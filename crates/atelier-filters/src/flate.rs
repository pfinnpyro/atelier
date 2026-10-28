//! DEFLATE (RFC 1951) and the zlib wrapper (RFC 1950), written from scratch.
//!
//! Huffman codes up to [`FAST_BITS`] long are decoded with a single table lookup;
//! longer codes fall back to a canonical bit-by-bit walk.

use crate::{FilterError, Result};

const FILTER: &str = "FlateDecode";
const FAST_BITS: u32 = 10;
const MAX_BITS: usize = 15;

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decodes a FlateDecode stream: zlib framed data, or raw DEFLATE when the zlib
/// header is missing (some producers write that). The Adler-32 checksum is not
/// enforced, matching what other viewers do with slightly damaged files.
pub fn decode_flate(data: &[u8]) -> Result<Vec<u8>> {
    let body = match data {
        [cmf, flg, ..] if cmf & 0x0f == 8 && (u16::from(*cmf) << 8 | u16::from(*flg)) % 31 == 0 => {
            let skip = if flg & 0x20 != 0 { 6 } else { 2 };
            data.get(skip..).unwrap_or(&[])
        }
        _ => data,
    };
    inflate(body)
}

/// Decodes raw DEFLATE data.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len().saturating_mul(4));
    match inflate_into(data, &mut out) {
        Ok(()) => Ok(out),
        Err(reason) => Err(FilterError::new(FILTER, reason, out)),
    }
}

fn inflate_into(data: &[u8], out: &mut Vec<u8>) -> std::result::Result<(), &'static str> {
    let mut br = BitReader::new(data);
    let mut fixed: Option<(Huffman, Huffman)> = None;
    loop {
        let last = br.bits(1)? == 1;
        match br.bits(2)? {
            0 => stored_block(&mut br, out)?,
            1 => {
                let (lit, dist) = fixed.get_or_insert_with(fixed_tables);
                codes(&mut br, out, lit, dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut br)?;
                codes(&mut br, out, &lit, &dist)?;
            }
            _ => return Err("invalid block type"),
        }
        if last {
            return Ok(());
        }
    }
}

fn stored_block(br: &mut BitReader, out: &mut Vec<u8>) -> std::result::Result<(), &'static str> {
    br.align_to_byte();
    let len = br.bits(16)?;
    let nlen = br.bits(16)?;
    if len != !nlen & 0xffff {
        return Err("stored block length check failed");
    }
    out.reserve(len as usize);
    for _ in 0..len {
        out.push(br.bits(8)? as u8);
    }
    Ok(())
}

fn codes(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
) -> std::result::Result<(), &'static str> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let i = usize::from(sym - 257);
                let len = usize::from(LEN_BASE[i]) + br.bits(u32::from(LEN_EXTRA[i]))? as usize;
                let d = usize::from(dist.decode(br)?);
                if d >= 30 {
                    return Err("invalid distance symbol");
                }
                let distance =
                    usize::from(DIST_BASE[d]) + br.bits(u32::from(DIST_EXTRA[d]))? as usize;
                if distance > out.len() {
                    return Err("distance reaches before start of output");
                }
                let start = out.len() - distance;
                if distance >= len {
                    out.extend_from_within(start..start + len);
                } else {
                    // Overlapping copy: the run repeats bytes it is producing.
                    out.reserve(len);
                    for k in 0..len {
                        let b = out[start + k];
                        out.push(b);
                    }
                }
            }
            _ => return Err("invalid literal/length symbol"),
        }
    }
}

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    let lit = Huffman::new(&lengths).expect("fixed literal table is valid");
    let dist = Huffman::new(&[5u8; 30]).expect("fixed distance table is valid");
    (lit, dist)
}

fn dynamic_tables(br: &mut BitReader) -> std::result::Result<(Huffman, Huffman), &'static str> {
    let nlen = br.bits(5)? as usize + 257;
    let ndist = br.bits(5)? as usize + 1;
    let ncode = br.bits(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err("too many length or distance codes");
    }

    let mut cl_lengths = [0u8; 19];
    for &slot in &CODE_LENGTH_ORDER[..ncode] {
        cl_lengths[slot] = br.bits(3)? as u8;
    }
    let cl = Huffman::new(&cl_lengths)?;

    let mut lengths = [0u8; 286 + 30];
    let total = nlen + ndist;
    let mut i = 0;
    while i < total {
        let sym = cl.decode(br)?;
        let (value, repeat) = match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
                continue;
            }
            16 => {
                if i == 0 {
                    return Err("repeat with no previous length");
                }
                (lengths[i - 1], 3 + br.bits(2)? as usize)
            }
            17 => (0, 3 + br.bits(3)? as usize),
            18 => (0, 11 + br.bits(7)? as usize),
            _ => return Err("invalid code length symbol"),
        };
        if i + repeat > total {
            return Err("code lengths overflow the table");
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }

    if lengths[256] == 0 {
        return Err("no end-of-block code");
    }
    let lit = Huffman::new(&lengths[..nlen])?;
    let dist = Huffman::new(&lengths[nlen..total])?;
    Ok((lit, dist))
}

/// Canonical Huffman decoding table.
struct Huffman {
    /// Number of codes of each length.
    counts: [u16; MAX_BITS + 1],
    /// Symbols ordered by code length, then by symbol value.
    symbols: Vec<u16>,
    /// Indexed by the next FAST_BITS input bits (LSB first). Entry is
    /// `symbol << 4 | length`, or 0 when the code is longer than FAST_BITS.
    fast: Box<[u16; 1 << FAST_BITS]>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> std::result::Result<Self, &'static str> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &len in lengths {
            counts[usize::from(len)] += 1;
        }
        counts[0] = 0;

        let mut left: i32 = 1;
        for &count in &counts[1..] {
            left = (left << 1) - i32::from(count);
            if left < 0 {
                return Err("over-subscribed Huffman code");
            }
        }

        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; usize::from(offsets[MAX_BITS + 1])];
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let slot = &mut offsets[usize::from(len)];
                symbols[usize::from(*slot)] = sym as u16;
                *slot += 1;
            }
        }

        let mut fast = Box::new([0u16; 1 << FAST_BITS]);
        let mut code: u32 = 0;
        let mut index = 0usize;
        for len in 1..=MAX_BITS as u32 {
            for _ in 0..counts[len as usize] {
                if len <= FAST_BITS {
                    let reversed = code.reverse_bits() >> (32 - len);
                    let entry = (symbols[index] << 4) | len as u16;
                    let mut k = reversed;
                    while k < (1 << FAST_BITS) {
                        fast[k as usize] = entry;
                        k += 1 << len;
                    }
                }
                code += 1;
                index += 1;
            }
            code <<= 1;
        }

        Ok(Self {
            counts,
            symbols,
            fast,
        })
    }

    #[inline]
    fn decode(&self, br: &mut BitReader) -> std::result::Result<u16, &'static str> {
        let (peeked, available) = br.peek(FAST_BITS);
        let entry = self.fast[peeked as usize];
        if entry != 0 {
            let len = u32::from(entry & 0xf);
            if len <= available {
                br.consume(len);
                return Ok(entry >> 4);
            }
        }
        self.decode_slow(br)
    }

    fn decode_slow(&self, br: &mut BitReader) -> std::result::Result<u16, &'static str> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_BITS {
            code |= br.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("invalid Huffman code")
    }
}

/// LSB-first bit reader over a byte slice.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    #[inline]
    fn refill(&mut self) {
        while self.count <= 56 {
            let Some(&b) = self.data.get(self.pos) else {
                return;
            };
            self.buf |= u64::from(b) << self.count;
            self.count += 8;
            self.pos += 1;
        }
    }

    /// Returns up to `n` upcoming bits (zero padded) and how many are real.
    #[inline]
    fn peek(&mut self, n: u32) -> (u32, u32) {
        if self.count < n {
            self.refill();
        }
        ((self.buf & ((1 << n) - 1)) as u32, self.count.min(n))
    }

    #[inline]
    fn consume(&mut self, n: u32) {
        self.buf >>= n;
        self.count -= n;
    }

    #[inline]
    fn bits(&mut self, n: u32) -> std::result::Result<u32, &'static str> {
        if n == 0 {
            return Ok(0);
        }
        if self.count < n {
            self.refill();
            if self.count < n {
                return Err("unexpected end of data");
            }
        }
        let v = (self.buf & ((1 << n) - 1)) as u32;
        self.consume(n);
        Ok(v)
    }

    fn align_to_byte(&mut self) {
        let drop = self.count % 8;
        self.consume(drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::{compress_to_vec, compress_to_vec_zlib};

    fn sample(len: usize, seed: u32) -> Vec<u8> {
        // Mix of repetitive text and pseudo-random bytes so every block type and
        // long and short matches get exercised.
        let mut state = seed;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if state >> 28 < 10 {
                out.extend_from_slice(b"BT /F1 12 Tf 72 712 Td (Hello, Atelier) Tj ET\n");
            } else {
                out.push((state >> 16) as u8);
            }
        }
        out.truncate(len);
        out
    }

    #[test]
    fn round_trips_every_compression_level() {
        for level in 0..=10u8 {
            for (len, seed) in [(0, 1), (1, 2), (300, 3), (70_000, 4), (500_000, 5)] {
                let data = sample(len, seed);
                let compressed = compress_to_vec_zlib(&data, level);
                assert_eq!(
                    decode_flate(&compressed).unwrap(),
                    data,
                    "level {level} len {len}"
                );
            }
        }
    }

    #[test]
    fn accepts_raw_deflate_without_zlib_header() {
        let data = sample(10_000, 9);
        assert_eq!(decode_flate(&compress_to_vec(&data, 6)).unwrap(), data);
    }

    #[test]
    fn long_runs_use_overlapping_copies() {
        let data = vec![b'a'; 100_000];
        assert_eq!(decode_flate(&compress_to_vec_zlib(&data, 9)).unwrap(), data);
    }

    #[test]
    fn truncated_stream_returns_partial_output() {
        let data = sample(200_000, 11);
        let compressed = compress_to_vec_zlib(&data, 6);
        let err = decode_flate(&compressed[..compressed.len() / 2]).unwrap_err();
        assert!(!err.partial.is_empty());
        assert_eq!(err.partial, data[..err.partial.len()]);
    }

    #[test]
    fn garbage_does_not_panic() {
        let mut state = 7u32;
        for len in 0..2000 {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 24) as u8
                })
                .collect();
            let _ = decode_flate(&bytes);
        }
    }
}
