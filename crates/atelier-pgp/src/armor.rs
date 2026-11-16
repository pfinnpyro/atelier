//! ASCII armor (RFC 9580, section 6): OpenPGP data as lines of base64
//! between a header and a tail line, for sending as text.

use crate::{Result, malformed};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Wraps `data` in armor of the kind named, such as `PUBLIC KEY BLOCK`.
/// No checksum line is written; the standard no longer asks for one.
pub fn armor(kind: &str, data: &[u8]) -> String {
    let mut text = String::with_capacity(data.len() * 4 / 3 + 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                text.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                text.push('=');
            }
        }
    }
    let mut out = format!("-----BEGIN PGP {kind}-----\n\n");
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let (line, tail) = rest.split_at(rest.len().min(64));
        out.push_str(line);
        out.push('\n');
        rest = tail;
    }
    out.push_str(&format!("-----END PGP {kind}-----\n"));
    out
}

/// The binary data of `data` if it is armored, or `data` itself when it
/// does not start with an armor header line.
pub fn dearmor(data: &[u8]) -> Result<Vec<u8>> {
    let start = data
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(data.len());
    if !data[start..].starts_with(b"-----BEGIN PGP ") {
        return Ok(data.to_vec());
    }
    let Ok(text) = std::str::from_utf8(&data[start..]) else {
        return malformed("armor that is not text");
    };
    let mut lines = text.lines().skip(1);
    // Armor headers run to the first blank line.
    for line in lines.by_ref() {
        if line.trim().is_empty() {
            break;
        }
        if !line.contains(": ") {
            return malformed("armor without a blank line after its headers");
        }
    }
    let mut sextets = Vec::new();
    let mut ended = false;
    for line in lines {
        let line = line.trim();
        if line.starts_with("-----END PGP ") {
            ended = true;
            break;
        }
        // A line starting with `=` is the old checksum, which is ignored.
        if line.starts_with('=') && line.len() == 5 {
            continue;
        }
        for c in line.bytes().filter(|c| *c != b'=') {
            match ALPHABET.iter().position(|a| *a == c) {
                Some(v) => sextets.push(v as u8),
                None => return malformed("armor with a character that is not base64"),
            }
        }
    }
    if !ended || sextets.len() % 4 == 1 {
        return malformed("armor that is cut short");
    }
    let mut out = Vec::with_capacity(sextets.len() * 3 / 4);
    for group in sextets.chunks(4) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, v)| n | u32::from(*v) << (18 - 6 * i));
        out.extend_from_slice(&n.to_be_bytes()[1..group.len()]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_length_round_trips() {
        for n in 0..200 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 5) as u8).collect();
            let text = armor("MESSAGE", &data);
            assert!(text.starts_with("-----BEGIN PGP MESSAGE-----\n\n"));
            assert!(text.ends_with("-----END PGP MESSAGE-----\n"));
            assert!(text.lines().all(|l| l.len() <= 64));
            assert_eq!(dearmor(text.as_bytes()).unwrap(), data, "{n} bytes");
        }
    }

    #[test]
    fn headers_checksums_and_line_endings_are_taken_in_stride() {
        let text = "\r\n-----BEGIN PGP MESSAGE-----\r\nComment: hello\r\n\r\naGVsbG8=\r\n=abcd\r\n\
                    -----END PGP MESSAGE-----\r\n";
        assert_eq!(dearmor(text.as_bytes()).unwrap(), b"hello");
        // Binary data passes through.
        assert_eq!(dearmor(&[0xc6, 1, 2]).unwrap(), [0xc6, 1, 2]);
        assert!(dearmor(b"-----BEGIN PGP MESSAGE-----\n\naGVsbG8=\n").is_err());
        assert!(
            dearmor(b"-----BEGIN PGP MESSAGE-----\n\naGV$bG8=\n-----END PGP MESSAGE-----").is_err()
        );
    }
}
