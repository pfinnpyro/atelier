//! Packets and the numbers inside them (RFC 9580, sections 3 and 4).

use std::borrow::Cow;

use crate::{Result, malformed};

/// Packet type IDs.
pub(crate) mod tag {
    pub const PKESK: u8 = 1;
    pub const SIGNATURE: u8 = 2;
    pub const SKESK: u8 = 3;
    pub const ONE_PASS: u8 = 4;
    pub const SECRET_KEY: u8 = 5;
    pub const PUBLIC_KEY: u8 = 6;
    pub const SECRET_SUBKEY: u8 = 7;
    pub const COMPRESSED: u8 = 8;
    pub const MARKER: u8 = 10;
    pub const LITERAL: u8 = 11;
    pub const USER_ID: u8 = 13;
    pub const PUBLIC_SUBKEY: u8 = 14;
    pub const SEIPD: u8 = 18;
    pub const PADDING: u8 = 21;
}

/// Reads fields off the front of some bytes.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.data.len() {
            return malformed("a packet ends too soon");
        }
        let (head, rest) = self.data.split_at(n);
        self.data = rest;
        Ok(head)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.data)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A multiprecision integer: a count of bits, then that many, in
    /// whole bytes.
    pub fn mpi(&mut self) -> Result<&'a [u8]> {
        let bits = usize::from(self.u16()?);
        self.take(bits.div_ceil(8))
    }
}

/// Appends a multiprecision integer; zeros at the front are dropped.
pub(crate) fn write_mpi(out: &mut Vec<u8>, value: &[u8]) {
    let start = value.iter().position(|b| *b != 0).unwrap_or(value.len());
    let value = &value[start..];
    let bits = match value.first() {
        Some(first) => value.len() * 8 - first.leading_zeros() as usize,
        None => 0,
    };
    out.extend_from_slice(&(bits as u16).to_be_bytes());
    out.extend_from_slice(value);
}

/// Appends a packet with its header.
pub(crate) fn write_packet(out: &mut Vec<u8>, tag: u8, body: &[u8]) {
    out.push(0xc0 | tag);
    match body.len() {
        n @ 0..192 => out.push(n as u8),
        n @ 192..8384 => {
            let n = n - 192;
            out.push((n >> 8) as u8 + 192);
            out.push(n as u8);
        }
        n => {
            out.push(0xff);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(body);
}

pub(crate) struct Packet<'a> {
    pub tag: u8,
    pub body: Cow<'a, [u8]>,
}

/// Splits bytes into packets. A body sent in pieces (partial lengths) is
/// put back together.
pub(crate) fn packets(data: &[u8]) -> Result<Vec<Packet<'_>>> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let first = r.u8()?;
        if first & 0x80 == 0 {
            return malformed("this is not an OpenPGP packet");
        }
        if first & 0x40 == 0 {
            // The legacy header: the type in four bits, then how the
            // length is written.
            let len = match first & 3 {
                0 => usize::from(r.u8()?),
                1 => usize::from(r.u16()?),
                2 => r.u32()? as usize,
                _ => r.data.len(),
            };
            out.push(Packet {
                tag: (first >> 2) & 0x0f,
                body: Cow::Borrowed(r.take(len)?),
            });
            continue;
        }
        let tag = first & 0x3f;
        let mut pieces: Option<Vec<u8>> = None;
        loop {
            let b = r.u8()?;
            let (len, last) = match b {
                0..192 => (usize::from(b), true),
                192..224 => (
                    ((usize::from(b) - 192) << 8) + usize::from(r.u8()?) + 192,
                    true,
                ),
                224..255 => (1usize << (b & 0x1f), false),
                255 => (r.u32()? as usize, true),
            };
            let piece = r.take(len)?;
            match (&mut pieces, last) {
                (None, true) => {
                    out.push(Packet {
                        tag,
                        body: Cow::Borrowed(piece),
                    });
                    break;
                }
                (Some(all), true) => {
                    all.extend_from_slice(piece);
                    out.push(Packet {
                        tag,
                        body: Cow::Owned(std::mem::take(all)),
                    });
                    break;
                }
                (None, false) => pieces = Some(piece.to_vec()),
                (Some(all), false) => all.extend_from_slice(piece),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_of_every_form_round_trip() {
        for n in [0, 1, 191, 192, 193, 8383, 8384, 70_000] {
            let body: Vec<u8> = (0..n).map(|i| i as u8).collect();
            let mut bytes = Vec::new();
            write_packet(&mut bytes, tag::LITERAL, &body);
            write_packet(&mut bytes, tag::PADDING, b"xy");
            let found = packets(&bytes).unwrap();
            assert_eq!(found.len(), 2);
            assert_eq!(found[0].tag, tag::LITERAL);
            assert_eq!(found[0].body, body, "{n} bytes");
            assert_eq!(found[1].body.as_ref(), b"xy");
        }
    }

    #[test]
    fn partial_and_legacy_lengths_are_read() {
        // Two pieces of 2 and 1 bytes, then a final piece of 3.
        let bytes = [0xcb, 0xe1, 1, 2, 0xe0, 3, 3, 4, 5, 6];
        let found = packets(&bytes).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].body.as_ref(), [1, 2, 3, 4, 5, 6]);
        // A legacy user ID packet with a one-byte length.
        let found = packets(&[0xb4, 2, b'h', b'i']).unwrap();
        assert_eq!((found[0].tag, found[0].body.as_ref()), (13, &b"hi"[..]));
        assert!(packets(&[0x40, 0]).is_err());
        assert!(packets(&[0xcb, 5, 1]).is_err());
    }

    #[test]
    fn mpis_drop_leading_zeros() {
        let mut out = Vec::new();
        write_mpi(&mut out, &[0, 0, 1, 0xff]);
        assert_eq!(out, [0, 9, 1, 0xff]);
        let mut r = Reader::new(&out);
        assert_eq!(r.mpi().unwrap(), [1, 0xff]);
        let mut out = Vec::new();
        write_mpi(&mut out, &[4, 9]);
        assert_eq!(out, [0, 11, 4, 9]);
    }
}
