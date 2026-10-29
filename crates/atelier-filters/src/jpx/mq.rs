//! The MQ arithmetic decoder (T.800 annex C), specialised for the 19
//! contexts of code-block decoding. It is the same coder JBIG2 uses; this
//! copy keeps C in one register and its tables sized so lookups need no
//! bounds checks.

/// Qe values, next states after an MPS and an LPS, and whether an LPS
/// switches the MPS sense (table C.2). Padded to 64 entries.
const QE: [u32; 64] = pad([
    0x5601, 0x3401, 0x1801, 0x0AC1, 0x0521, 0x0221, 0x5601, 0x5401, 0x4801, 0x3801, 0x3001, 0x2401,
    0x1C01, 0x1601, 0x5601, 0x5401, 0x5101, 0x4801, 0x3801, 0x3401, 0x3001, 0x2801, 0x2401, 0x2201,
    0x1C01, 0x1801, 0x1601, 0x1401, 0x1201, 0x1101, 0x0AC1, 0x09C1, 0x08A1, 0x0521, 0x0441, 0x02A1,
    0x0221, 0x0141, 0x0111, 0x0085, 0x0049, 0x0025, 0x0015, 0x0009, 0x0005, 0x0001, 0x5601,
]);
const NMPS: [u32; 64] = pad([
    1, 2, 3, 4, 5, 38, 7, 8, 9, 10, 11, 12, 13, 29, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 45, 46,
]);
const NLPS: [u32; 64] = pad([
    1, 6, 9, 12, 29, 33, 6, 14, 14, 14, 17, 18, 20, 21, 14, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22,
    23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 46,
]);
const SWITCH: [u32; 64] = pad([
    1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);

const fn pad(v: [u32; 47]) -> [u32; 64] {
    let mut out = [v[46]; 64];
    let mut i = 0;
    while i < 47 {
        out[i] = v[i];
        i += 1;
    }
    out
}

/// Context states: index into the tables << 1 | MPS. 32 slots, of which
/// code-block decoding uses 19.
pub type Contexts = [u8; 32];

pub struct Mq<'a> {
    data: &'a [u8],
    pos: usize,
    c: u32,
    a: u32,
    ct: u32,
}

impl<'a> Mq<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = Self {
            data,
            pos: 0,
            c: u32::from(data.first().copied().unwrap_or(0xff)) << 16,
            a: 0,
            ct: 0,
        };
        d.byte_in();
        d.c <<= 7;
        d.ct -= 7;
        d.a = 0x8000;
        d
    }

    #[inline]
    fn byte(&self, i: usize) -> u32 {
        u32::from(self.data.get(i).copied().unwrap_or(0xff))
    }

    #[inline]
    fn byte_in(&mut self) {
        if self.byte(self.pos) == 0xff {
            if self.byte(self.pos + 1) > 0x8f {
                self.c = self.c.wrapping_add(0xff00);
                self.ct = 8;
            } else {
                self.pos += 1;
                self.c = self.c.wrapping_add(self.byte(self.pos) << 9);
                self.ct = 7;
            }
        } else {
            self.pos += 1;
            self.c = self.c.wrapping_add(self.byte(self.pos) << 8);
            self.ct = 8;
        }
    }

    /// Decodes one bit with the state in `cx[i]`.
    #[inline]
    pub fn bit(&mut self, cx: &mut Contexts, i: usize) -> u8 {
        let slot = &mut cx[i & 31];
        let state = u32::from(*slot);
        let index = (state >> 1) as usize & 63;
        let mut mps = state & 1;
        let qe = QE[index];
        self.a -= qe;
        let (d, next);
        if (self.c >> 16) < qe {
            // LPS path, with the conditional exchange.
            if self.a < qe {
                d = mps;
                next = NMPS[index];
            } else {
                d = mps ^ 1;
                mps ^= SWITCH[index];
                next = NLPS[index];
            }
            self.a = qe;
        } else {
            self.c -= qe << 16;
            if self.a & 0x8000 != 0 {
                return mps as u8;
            }
            if self.a < qe {
                d = mps ^ 1;
                mps ^= SWITCH[index];
                next = NLPS[index];
            } else {
                d = mps;
                next = NMPS[index];
            }
        }
        *slot = (next << 1 | mps) as u8;
        loop {
            if self.ct == 0 {
                self.byte_in();
            }
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.a & 0x8000 != 0 {
                break;
            }
        }
        d as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test sequence from T.88 annex H.2 (the same MQ coder).
    #[test]
    fn annex_h2_sequence() {
        let encoded: [u8; 30] = [
            0x84, 0xC7, 0x3B, 0xFC, 0xE1, 0xA1, 0x43, 0x04, 0x02, 0x20, 0x00, 0x00, 0x41, 0x0D,
            0xBB, 0x86, 0xF4, 0x31, 0x7F, 0xFF, 0x88, 0xFF, 0x37, 0x47, 0x1A, 0xDB, 0x6A, 0xDF,
            0xFF, 0xAC,
        ];
        let expected: [u8; 32] = [
            0x00, 0x02, 0x00, 0x51, 0x00, 0x00, 0x00, 0xC0, 0x03, 0x52, 0x87, 0x2A, 0xAA, 0xAA,
            0xAA, 0xAA, 0x82, 0xC0, 0x20, 0x00, 0xFC, 0xD7, 0x9E, 0xF6, 0xBF, 0x7F, 0xED, 0x90,
            0x4F, 0x46, 0xA3, 0xBF,
        ];
        let mut d = Mq::new(&encoded);
        let mut cx = [0u8; 32];
        for (i, &byte) in expected.iter().enumerate() {
            let mut v = 0u8;
            for _ in 0..8 {
                v = (v << 1) | d.bit(&mut cx, 0);
            }
            assert_eq!(v, byte, "byte {i}");
        }
    }
}
