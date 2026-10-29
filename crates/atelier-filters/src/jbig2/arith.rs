//! The MQ arithmetic decoder (T.88 annex E) and the integer and symbol-ID
//! decoding procedures built on it (annex A.2 and A.3).

/// Qe value, next index after MPS, next index after LPS, and whether an
/// LPS switches the MPS sense.
const QE: [(u32, u8, u8, bool); 47] = [
    (0x5601, 1, 1, true),
    (0x3401, 2, 6, false),
    (0x1801, 3, 9, false),
    (0x0AC1, 4, 12, false),
    (0x0521, 5, 29, false),
    (0x0221, 38, 33, false),
    (0x5601, 7, 6, true),
    (0x5401, 8, 14, false),
    (0x4801, 9, 14, false),
    (0x3801, 10, 14, false),
    (0x3001, 11, 17, false),
    (0x2401, 12, 18, false),
    (0x1C01, 13, 20, false),
    (0x1601, 29, 21, false),
    (0x5601, 15, 14, true),
    (0x5401, 16, 14, false),
    (0x5101, 17, 15, false),
    (0x4801, 18, 16, false),
    (0x3801, 19, 17, false),
    (0x3401, 20, 18, false),
    (0x3001, 21, 19, false),
    (0x2801, 22, 19, false),
    (0x2401, 23, 20, false),
    (0x2201, 24, 21, false),
    (0x1C01, 25, 22, false),
    (0x1801, 26, 23, false),
    (0x1601, 27, 24, false),
    (0x1401, 28, 25, false),
    (0x1201, 29, 26, false),
    (0x1101, 30, 27, false),
    (0x0AC1, 31, 28, false),
    (0x09C1, 32, 29, false),
    (0x08A1, 33, 30, false),
    (0x0521, 34, 31, false),
    (0x0441, 35, 32, false),
    (0x02A1, 36, 33, false),
    (0x0221, 37, 34, false),
    (0x0141, 38, 35, false),
    (0x0111, 39, 36, false),
    (0x0085, 40, 37, false),
    (0x0049, 41, 38, false),
    (0x0025, 42, 39, false),
    (0x0015, 43, 40, false),
    (0x0009, 44, 41, false),
    (0x0005, 45, 42, false),
    (0x0001, 45, 43, false),
    (0x5601, 46, 46, false),
];

/// One adaptive probability state per context: index into `QE` << 1 | MPS.
pub type Contexts = Vec<u8>;

pub fn contexts(n: usize) -> Contexts {
    vec![0; n]
}

pub struct ArithDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    chigh: u32,
    clow: u32,
    ct: i32,
    a: u32,
}

impl<'a> ArithDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = Self {
            data,
            pos: 0,
            chigh: u32::from(data.first().copied().unwrap_or(0xff)),
            clow: 0,
            ct: 0,
            a: 0,
        };
        d.byte_in();
        d.chigh = ((d.chigh << 7) & 0xffff) | ((d.clow >> 9) & 0x7f);
        d.clow = (d.clow << 7) & 0xffff;
        d.ct -= 7;
        d.a = 0x8000;
        d
    }

    fn byte(&self, i: usize) -> u32 {
        u32::from(self.data.get(i).copied().unwrap_or(0xff))
    }

    fn byte_in(&mut self) {
        if self.byte(self.pos) == 0xff {
            if self.byte(self.pos + 1) > 0x8f {
                self.clow += 0xff00;
                self.ct = 8;
            } else {
                self.pos += 1;
                self.clow += self.byte(self.pos) << 9;
                self.ct = 7;
            }
        } else {
            self.pos += 1;
            self.clow += if self.pos < self.data.len() {
                self.byte(self.pos) << 8
            } else {
                0xff00
            };
            self.ct = 8;
        }
        if self.clow > 0xffff {
            self.chigh += self.clow >> 16;
            self.clow &= 0xffff;
        }
    }

    /// Decodes one bit with the state in `cx[i]`.
    pub fn bit(&mut self, cx: &mut [u8], i: usize) -> u8 {
        let state = cx[i];
        let mut index = (state >> 1) as usize;
        let mut mps = state & 1;
        let (qe, nmps, nlps, switch) = QE[index.min(46)];
        self.a -= qe;
        let d;
        if self.chigh < qe {
            // Conditional exchange on the LPS path.
            if self.a < qe {
                self.a = qe;
                d = mps;
                index = nmps as usize;
            } else {
                self.a = qe;
                d = 1 ^ mps;
                if switch {
                    mps = d;
                }
                index = nlps as usize;
            }
        } else {
            self.chigh -= qe;
            if self.a & 0x8000 != 0 {
                return mps;
            }
            if self.a < qe {
                d = 1 ^ mps;
                if switch {
                    mps = d;
                }
                index = nlps as usize;
            } else {
                d = mps;
                index = nmps as usize;
            }
        }
        loop {
            if self.ct == 0 {
                self.byte_in();
            }
            self.a <<= 1;
            self.chigh = ((self.chigh << 1) & 0xffff) | ((self.clow >> 15) & 1);
            self.clow = (self.clow << 1) & 0xffff;
            self.ct -= 1;
            if self.a & 0x8000 != 0 {
                break;
            }
        }
        cx[i] = ((index as u8) << 1) | mps;
        d
    }

    /// The integer decoding procedure (A.2); `None` is out-of-band.
    pub fn int(&mut self, cx: &mut [u8]) -> Option<i64> {
        let mut prev = 1usize;
        let mut read = |d: &mut Self, n: u32| -> i64 {
            let mut v = 0i64;
            for _ in 0..n {
                let bit = d.bit(cx, prev);
                prev = if prev < 256 {
                    (prev << 1) | bit as usize
                } else {
                    (((prev << 1) | bit as usize) & 511) | 256
                };
                v = (v << 1) | i64::from(bit);
            }
            v
        };
        let sign = read(self, 1);
        let value = if read(self, 1) == 0 {
            read(self, 2)
        } else if read(self, 1) == 0 {
            read(self, 4) + 4
        } else if read(self, 1) == 0 {
            read(self, 6) + 20
        } else if read(self, 1) == 0 {
            read(self, 8) + 84
        } else if read(self, 1) == 0 {
            read(self, 12) + 340
        } else {
            read(self, 32) + 4436
        };
        match (sign, value) {
            (0, v) => Some(v),
            (_, 0) => None,
            (_, v) => Some(-v),
        }
    }

    /// The symbol ID decoding procedure (A.3) for `len`-bit codes.
    pub fn id(&mut self, cx: &mut [u8], len: u32) -> usize {
        let mut prev = 1usize;
        for _ in 0..len {
            let bit = self.bit(cx, prev);
            prev = (prev << 1) | bit as usize;
        }
        prev - (1 << len)
    }
}

/// The context sets one decoding run shares (one per IAx procedure, plus the
/// generic and refinement statistics).
pub struct IntContexts {
    pub iadh: Contexts,
    pub iadw: Contexts,
    pub iaex: Contexts,
    pub iaai: Contexts,
    pub iadt: Contexts,
    pub iafs: Contexts,
    pub iads: Contexts,
    pub iait: Contexts,
    pub iari: Contexts,
    pub iardw: Contexts,
    pub iardh: Contexts,
    pub iardx: Contexts,
    pub iardy: Contexts,
    pub iaid: Contexts,
    pub gb: Contexts,
    pub gr: Contexts,
}

impl IntContexts {
    pub fn new(id_len: u32) -> Self {
        let c = || contexts(512);
        Self {
            iadh: c(),
            iadw: c(),
            iaex: c(),
            iaai: c(),
            iadt: c(),
            iafs: c(),
            iads: c(),
            iait: c(),
            iari: c(),
            iardw: c(),
            iardh: c(),
            iardx: c(),
            iardy: c(),
            iaid: contexts(1 << (id_len.min(24) + 1)),
            gb: contexts(1 << 16),
            gr: contexts(1 << 13),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test sequence from T.88 annex H.2: 256 encoded bytes and the
    /// bits they decode to with a single context.
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
        let mut d = ArithDecoder::new(&encoded);
        let mut cx = contexts(1);
        for (i, &byte) in expected.iter().enumerate() {
            let mut v = 0u8;
            for _ in 0..8 {
                v = (v << 1) | d.bit(&mut cx, 0);
            }
            assert_eq!(v, byte, "byte {i}");
        }
    }
}
