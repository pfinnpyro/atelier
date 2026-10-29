//! Code-block decoding (T.800 annex D): the three coding passes over each
//! bit plane, with the MQ coder or, in bypass mode, raw bits.
//!
//! Coefficients come out doubled, so the reconstruction midpoint of the
//! lowest decoded plane stays an integer: a coefficient that becomes
//! significant in plane p is 3 << p, and refining it in plane q moves it by
//! 1 << q.

use super::codestream::style;
use super::mq::{Contexts, Mq};

// Per-coefficient flags: its own state in the low four bits, then the
// significance of its eight neighbours (in the bit order of `ZC`), then the
// signs of its four direct neighbours.
const SIG: u16 = 1;
const VISIT: u16 = 2;
const REFINED: u16 = 4;
const NEG: u16 = 8;
const N_SIG: u16 = 1 << 4;
const S_SIG: u16 = 1 << 5;
const W_SIG: u16 = 1 << 6;
const E_SIG: u16 = 1 << 7;
const NW_SIG: u16 = 1 << 8;
const NE_SIG: u16 = 1 << 9;
const SW_SIG: u16 = 1 << 10;
const SE_SIG: u16 = 1 << 11;
const N_NEG: u16 = 1 << 12;
const S_NEG: u16 = 1 << 13;
const W_NEG: u16 = 1 << 14;
const E_NEG: u16 = 1 << 15;
/// What the vertically causal mode hides from a stripe's last row.
const BELOW: u16 = S_SIG | SW_SIG | SE_SIG | S_NEG;

const CTX_RL: usize = 17;
const CTX_UNI: usize = 18;

/// Subband orientations.
pub const LL: u8 = 0;
pub const HL: u8 = 1;
pub const LH: u8 = 2;
pub const HH: u8 = 3;

/// Zero-coding contexts (table D.1) by orientation table (0: LL and LH,
/// 1: HL, 2: HH) and the significance of the eight neighbours: bits N S W E
/// NW NE SW SE from the lowest.
const ZC: [[u8; 256]; 3] = build_zc();

const fn build_zc() -> [[u8; 256]; 3] {
    let mut t = [[0u8; 256]; 3];
    let mut table = 0;
    while table < 3 {
        let mut p = 0;
        while p < 256 {
            let v0 = (p & 1) + ((p >> 1) & 1);
            let h0 = ((p >> 2) & 1) + ((p >> 3) & 1);
            let d = ((p >> 4) & 1) + ((p >> 5) & 1) + ((p >> 6) & 1) + ((p >> 7) & 1);
            let (h, v) = if table == 1 { (v0, h0) } else { (h0, v0) };
            t[table][p] = if table == 2 {
                let hv = h + v;
                match d {
                    0 => match hv {
                        0 => 0,
                        1 => 1,
                        _ => 2,
                    },
                    1 => match hv {
                        0 => 3,
                        1 => 4,
                        _ => 5,
                    },
                    2 => {
                        if hv == 0 {
                            6
                        } else {
                            7
                        }
                    }
                    _ => 8,
                }
            } else {
                match h {
                    0 => match v {
                        0 => match d {
                            0 => 0,
                            1 => 1,
                            _ => 2,
                        },
                        1 => 3,
                        _ => 4,
                    },
                    1 => {
                        if v > 0 {
                            7
                        } else if d > 0 {
                            6
                        } else {
                            5
                        }
                    }
                    _ => 8,
                }
            };
            p += 1;
        }
        table += 1;
    }
    t
}

/// Sign-coding context and XOR bit (table D.3), by the significance
/// (bits 0-3: N S W E) and signs (bits 4-7) of the direct neighbours.
const SC: [(u8, u8); 256] = build_sc();

/// One neighbour's contribution: 0 if insignificant, else its sign.
const fn contribution(i: usize, sig: usize, neg: usize) -> i32 {
    if (i >> sig) & 1 == 0 {
        0
    } else if (i >> neg) & 1 == 1 {
        -1
    } else {
        1
    }
}

const fn clamp1(v: i32) -> i32 {
    if v < -1 {
        -1
    } else if v > 1 {
        1
    } else {
        v
    }
}

const fn build_sc() -> [(u8, u8); 256] {
    let mut t = [(0u8, 0u8); 256];
    let mut i = 0;
    while i < 256 {
        let v = clamp1(contribution(i, 0, 4) + contribution(i, 1, 5));
        let h = clamp1(contribution(i, 2, 6) + contribution(i, 3, 7));
        t[i] = match (h, v) {
            (1, 1) => (13, 0),
            (1, 0) => (12, 0),
            (1, _) => (11, 0),
            (0, 1) => (10, 0),
            (0, 0) => (9, 0),
            (0, _) => (10, 1),
            (_, 1) => (11, 1),
            (_, 0) => (12, 1),
            _ => (13, 1),
        };
        i += 1;
    }
    t
}

fn initial_contexts() -> Contexts {
    let mut c = [0u8; 32];
    c[0] = 4 << 1;
    c[CTX_RL] = 3 << 1;
    c[CTX_UNI] = 46 << 1;
    c
}

/// Raw (bypass) bits, with a zero bit stuffed after each 0xFF byte.
struct Raw<'a> {
    data: &'a [u8],
    pos: usize,
    c: u32,
    ct: u32,
}

impl Raw<'_> {
    fn bit(&mut self) -> u8 {
        if self.ct == 0 {
            let next = |i: usize| u32::from(self.data.get(i).copied().unwrap_or(0xff));
            if self.c == 0xff {
                if next(self.pos) > 0x8f {
                    self.c = 0xff;
                    self.ct = 8;
                } else {
                    self.c = next(self.pos);
                    self.pos += 1;
                    self.ct = 7;
                }
            } else {
                self.c = next(self.pos);
                self.pos += 1;
                self.ct = 8;
            }
        }
        self.ct -= 1;
        ((self.c >> self.ct) & 1) as u8
    }
}

trait Bits {
    fn bit(&mut self, cx: &mut Contexts, i: usize) -> u8;
}

impl Bits for Mq<'_> {
    #[inline]
    fn bit(&mut self, cx: &mut Contexts, i: usize) -> u8 {
        Mq::bit(self, cx, i)
    }
}

impl Bits for Raw<'_> {
    #[inline]
    fn bit(&mut self, _: &mut Contexts, _: usize) -> u8 {
        Raw::bit(self)
    }
}

/// One terminated piece of a code-block's data and how many passes it holds.
pub struct Segment<'a> {
    pub data: &'a [u8],
    pub passes: u32,
}

/// Reused buffers for decoding code-blocks.
#[derive(Default)]
pub struct Block {
    w: usize,
    h: usize,
    stride: usize,
    flags: Vec<u16>,
    /// The decoded coefficients, row by row.
    pub data: Vec<i32>,
}

/// The flags of a coefficient as its context sees them.
#[inline]
fn visible(f: u16, vsc_last: bool) -> u16 {
    if vsc_last { f & !BELOW } else { f }
}

#[inline]
fn neighbours(f: u16) -> usize {
    usize::from((f >> 4) & 0xff)
}

impl Block {
    #[inline]
    fn decode_sign<B: Bits>(b: &mut B, cx: &mut Contexts, f: u16, raw: bool) -> bool {
        if raw {
            b.bit(cx, 0) == 1
        } else {
            let (ctx, xor) = SC[usize::from((f >> 4) & 0xf | (f >> 8) & 0xf0)];
            (b.bit(cx, usize::from(ctx)) ^ xor) == 1
        }
    }

    /// Marks coefficient `i` significant and tells its neighbours.
    #[inline]
    fn make_significant(&mut self, i: usize, d: usize, neg: bool, plane: u32) {
        let s = self.stride;
        let f = &mut self.flags;
        f[i] |= SIG | if neg { NEG } else { 0 };
        f[i - s] |= S_SIG | if neg { S_NEG } else { 0 };
        f[i + s] |= N_SIG | if neg { N_NEG } else { 0 };
        f[i - 1] |= E_SIG | if neg { E_NEG } else { 0 };
        f[i + 1] |= W_SIG | if neg { W_NEG } else { 0 };
        f[i - s - 1] |= SE_SIG;
        f[i - s + 1] |= SW_SIG;
        f[i + s - 1] |= NE_SIG;
        f[i + s + 1] |= NW_SIG;
        let v = 3i32 << plane;
        self.data[d] = if neg { -v } else { v };
    }

    fn significance<B: Bits>(
        &mut self,
        b: &mut B,
        cx: &mut Contexts,
        zc: &[u8; 256],
        plane: u32,
        vsc: bool,
        raw: bool,
    ) {
        const NEIGHBOURS: u16 = 0xff << 4;
        for y0 in (0..self.h).step_by(4) {
            let y1 = (y0 + 4).min(self.h);
            for x in 0..self.w {
                for y in y0..y1 {
                    let i = (y + 1) * self.stride + x + 1;
                    let f = visible(self.flags[i], vsc && y == y0 + 3);
                    if f & SIG != 0 || f & NEIGHBOURS == 0 {
                        continue;
                    }
                    if b.bit(cx, usize::from(zc[neighbours(f)])) == 1 {
                        let neg = Self::decode_sign(b, cx, f, raw);
                        self.make_significant(i, y * self.w + x, neg, plane);
                    }
                    self.flags[i] |= VISIT;
                }
            }
        }
    }

    fn refinement<B: Bits>(&mut self, b: &mut B, cx: &mut Contexts, plane: u32, vsc: bool) {
        let half = 1i32 << plane;
        for y0 in (0..self.h).step_by(4) {
            let y1 = (y0 + 4).min(self.h);
            for x in 0..self.w {
                for y in y0..y1 {
                    let i = (y + 1) * self.stride + x + 1;
                    let f = self.flags[i];
                    if f & (SIG | VISIT) != SIG {
                        continue;
                    }
                    let ctx = if f & REFINED != 0 {
                        16
                    } else if neighbours(visible(f, vsc && y == y0 + 3)) != 0 {
                        15
                    } else {
                        14
                    };
                    let bit = b.bit(cx, ctx);
                    let d = &mut self.data[y * self.w + x];
                    let step = if bit == 1 { half } else { -half };
                    *d += if *d < 0 { -step } else { step };
                    self.flags[i] |= REFINED;
                }
            }
        }
    }

    fn cleanup<B: Bits>(
        &mut self,
        b: &mut B,
        cx: &mut Contexts,
        zc: &[u8; 256],
        plane: u32,
        vsc: bool,
    ) {
        let s = self.stride;
        for y0 in (0..self.h).step_by(4) {
            let y1 = (y0 + 4).min(self.h);
            for x in 0..self.w {
                let col = (y0 + 1) * s + x + 1;
                let mut start = y0;
                if y1 - y0 == 4 {
                    let f = &self.flags;
                    // Run-length mode needs four untouched coefficients with
                    // no significant neighbours.
                    let quiet = f[col] | f[col + s] | f[col + 2 * s] | visible(f[col + 3 * s], vsc);
                    if quiet == 0 {
                        if b.bit(cx, CTX_RL) == 0 {
                            continue;
                        }
                        let k =
                            usize::from(b.bit(cx, CTX_UNI)) << 1 | usize::from(b.bit(cx, CTX_UNI));
                        let i = col + k * s;
                        let f = visible(self.flags[i], vsc && k == 3);
                        let neg = Self::decode_sign(b, cx, f, false);
                        self.make_significant(i, (y0 + k) * self.w + x, neg, plane);
                        start = y0 + k + 1;
                    }
                }
                for y in start..y1 {
                    let i = (y + 1) * s + x + 1;
                    let f = visible(self.flags[i], vsc && y == y0 + 3);
                    if f & (SIG | VISIT) != 0 {
                        continue;
                    }
                    if b.bit(cx, usize::from(zc[neighbours(f)])) == 1 {
                        let neg = Self::decode_sign(b, cx, f, false);
                        self.make_significant(i, y * self.w + x, neg, plane);
                    }
                }
                for y in y0..y1 {
                    self.flags[(y + 1) * s + x + 1] &= !VISIT;
                }
            }
        }
    }

    /// Decodes a `w` x `h` code-block from its segments. `planes` is the
    /// number of magnitude bit planes left after the missing top ones.
    pub fn decode(
        &mut self,
        w: usize,
        h: usize,
        orient: u8,
        cb_style: u8,
        planes: u32,
        segments: &[Segment],
    ) {
        self.w = w;
        self.h = h;
        self.stride = w + 2;
        self.flags.clear();
        self.flags.resize(self.stride * (h + 2), 0);
        self.data.clear();
        self.data.resize(w * h, 0);
        // Doubled coefficients must fit an i32.
        let planes = planes.min(30);
        if planes == 0 {
            return;
        }
        let zc = &ZC[match orient {
            HL => 1,
            HH => 2,
            _ => 0,
        }];
        let vsc = cb_style & style::VSC != 0;
        let bypass = cb_style & style::BYPASS != 0;
        let mut cx = initial_contexts();
        let mut pass = 0u32;
        for seg in segments {
            if seg.passes == 0 {
                continue;
            }
            let mut mq = Mq::new(seg.data);
            let mut raw = Raw {
                data: seg.data,
                pos: 0,
                c: 0,
                ct: 0,
            };
            for _ in 0..seg.passes {
                // Pass 0 is the top plane's cleanup; then significance,
                // refinement and cleanup for each lower plane.
                let (kind, depth) = if pass == 0 {
                    (2, 0)
                } else {
                    ((pass - 1) % 3, pass.div_ceil(3))
                };
                if depth >= planes {
                    return;
                }
                let plane = planes - 1 - depth;
                let is_raw = bypass && pass >= 10 && kind != 2;
                match (kind, is_raw) {
                    (0, false) => self.significance(&mut mq, &mut cx, zc, plane, vsc, false),
                    (0, true) => self.significance(&mut raw, &mut cx, zc, plane, vsc, true),
                    (1, false) => self.refinement(&mut mq, &mut cx, plane, vsc),
                    (1, true) => self.refinement(&mut raw, &mut cx, plane, vsc),
                    _ => {
                        self.cleanup(&mut mq, &mut cx, zc, plane, vsc);
                        if cb_style & style::SEGSYM != 0 {
                            for _ in 0..4 {
                                mq.bit(&mut cx, CTX_UNI);
                            }
                        }
                    }
                }
                if cb_style & style::RESET != 0 {
                    cx = initial_contexts();
                }
                pass += 1;
            }
        }
    }
}
