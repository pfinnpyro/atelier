//! A small DEFLATE compressor: greedy LZ77 with hash chains and the fixed
//! Huffman code. Used for PNG output now and for writing PDFs later. It favours
//! simplicity over ratio; dynamic Huffman blocks can come later.

const WINDOW: usize = 32 * 1024;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const HASH_BITS: u32 = 15;
const MAX_CHAIN: usize = 48;

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

/// Compresses `data` into a zlib stream (what FlateDecode and PNG expect).
pub fn encode_flate(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.out.extend_from_slice(&[0x78, 0x9c]);
    w.bits(1, 1); // final block
    w.bits(1, 2); // fixed Huffman

    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; WINDOW];
    let hash = |i: usize| -> usize {
        let v = u32::from(data[i]) << 16 | u32::from(data[i + 1]) << 8 | u32::from(data[i + 2]);
        (v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
    };
    let insert = |i: usize, head: &mut [usize], prev: &mut [usize]| {
        if i + MIN_MATCH <= data.len() {
            let h = hash(i);
            prev[i % WINDOW] = head[h];
            head[h] = i;
        }
    };

    let mut i = 0;
    while i < data.len() {
        let (mut best_len, mut best_dist) = (0, 0);
        if i + MIN_MATCH <= data.len() {
            let mut candidate = head[hash(i)];
            let max_len = MAX_MATCH.min(data.len() - i);
            let mut chain = 0;
            while candidate != usize::MAX && i - candidate <= WINDOW && chain < MAX_CHAIN {
                let len = data[candidate..]
                    .iter()
                    .zip(&data[i..i + max_len])
                    .take_while(|(a, b)| a == b)
                    .count();
                if len > best_len {
                    best_len = len;
                    best_dist = i - candidate;
                    if len == max_len {
                        break;
                    }
                }
                let next = prev[candidate % WINDOW];
                if next == usize::MAX || next >= candidate {
                    break;
                }
                candidate = next;
                chain += 1;
            }
        }
        if best_len >= MIN_MATCH {
            w.length(best_len);
            w.distance(best_dist);
            for k in i..i + best_len {
                insert(k, &mut head, &mut prev);
            }
            i += best_len;
        } else {
            w.literal(u16::from(data[i]));
            insert(i, &mut head, &mut prev);
            i += 1;
        }
    }
    w.literal(256);
    w.flush();
    w.out.extend_from_slice(&adler32(data).to_be_bytes());
    w.out
}

/// The Adler-32 checksum used by zlib.
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    buf: u64,
    count: u32,
}

impl BitWriter {
    /// Writes `n` bits of `v`, least significant first.
    fn bits(&mut self, v: u32, n: u32) {
        self.buf |= u64::from(v) << self.count;
        self.count += n;
        while self.count >= 8 {
            self.out.push(self.buf as u8);
            self.buf >>= 8;
            self.count -= 8;
        }
    }

    /// Writes a Huffman code, which DEFLATE stores most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        self.bits(code.reverse_bits() >> (32 - len), len);
    }

    fn literal(&mut self, sym: u16) {
        let sym = u32::from(sym);
        match sym {
            0..=143 => self.code(0x30 + sym, 8),
            144..=255 => self.code(0x190 + sym - 144, 9),
            256..=279 => self.code(sym - 256, 7),
            _ => self.code(0xc0 + sym - 280, 8),
        }
    }

    fn length(&mut self, len: usize) {
        let i = LEN_BASE
            .iter()
            .rposition(|&b| usize::from(b) <= len)
            .unwrap_or(0);
        self.literal(257 + i as u16);
        self.bits(
            (len - usize::from(LEN_BASE[i])) as u32,
            u32::from(LEN_EXTRA[i]),
        );
    }

    fn distance(&mut self, dist: usize) {
        let i = DIST_BASE
            .iter()
            .rposition(|&b| usize::from(b) <= dist)
            .unwrap_or(0);
        self.code(i as u32, 5);
        self.bits(
            (dist - usize::from(DIST_BASE[i])) as u32,
            u32::from(DIST_EXTRA[i]),
        );
    }

    fn flush(&mut self) {
        if self.count > 0 {
            self.out.push(self.buf as u8);
            self.buf = 0;
            self.count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_flate;

    fn sample(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if state >> 29 < 5 {
                out.extend_from_slice(b"0 0 612 792 re f 1 0 0 RG ");
            } else {
                out.push((state >> 16) as u8);
            }
        }
        out.truncate(len);
        out
    }

    #[test]
    fn round_trips_through_our_inflate_and_miniz() {
        for (len, seed) in [
            (0, 1),
            (1, 2),
            (2, 3),
            (1000, 4),
            (100_000, 5),
            (300_000, 6),
        ] {
            let data = sample(len, seed);
            let packed = encode_flate(&data);
            assert_eq!(decode_flate(&packed).unwrap(), data);
            assert_eq!(
                miniz_oxide::inflate::decompress_to_vec_zlib(&packed).unwrap(),
                data
            );
        }
    }

    #[test]
    fn compresses_repetitive_data() {
        let data = vec![255u8; 1_000_000];
        assert!(encode_flate(&data).len() < 10_000);
    }

    #[test]
    fn adler_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
