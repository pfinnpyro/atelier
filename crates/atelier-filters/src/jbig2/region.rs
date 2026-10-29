//! Bitmaps, generic region decoding (6.2) and generic refinement region
//! decoding (6.3).

use super::arith::ArithDecoder;
use crate::ccitt::{CcittParams, decode_mmr};

/// A bilevel image, one byte per pixel, 1 = black.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// How a region's pixels combine with what is under them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    Or,
    And,
    Xor,
    Xnor,
    Replace,
}

impl Combine {
    pub fn from_bits(v: u8) -> Self {
        match v {
            1 => Self::And,
            2 => Self::Xor,
            3 => Self::Xnor,
            4 => Self::Replace,
            _ => Self::Or,
        }
    }
}

impl Bitmap {
    pub fn new(width: usize, height: usize, value: u8) -> Self {
        Self {
            width,
            height,
            data: vec![value; width * height],
        }
    }

    /// The pixel at (x, y); 0 outside the bitmap.
    #[inline]
    pub fn get(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            0
        } else {
            self.data[y as usize * self.width + x as usize]
        }
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: u8) {
        self.data[y * self.width + x] = v;
    }

    /// Draws `src` with its top left corner at (x, y).
    pub fn compose(&mut self, src: &Bitmap, x: i64, y: i64, op: Combine) {
        for sy in 0..src.height {
            let dy = y + sy as i64;
            if dy < 0 || dy >= self.height as i64 {
                continue;
            }
            let x0 = (-x).max(0) as usize;
            let x1 = (self.width as i64 - x).clamp(0, src.width as i64) as usize;
            for sx in x0..x1 {
                let s = src.data[sy * src.width + sx];
                let d = &mut self.data[dy as usize * self.width + (x + sx as i64) as usize];
                *d = match op {
                    Combine::Or => *d | s,
                    Combine::And => *d & s,
                    Combine::Xor => *d ^ s,
                    Combine::Xnor => 1 ^ (*d ^ s),
                    Combine::Replace => s,
                };
            }
        }
    }

    /// The part of this bitmap at (x, y) of the given size; 0 outside.
    pub fn crop(&self, x: i64, y: i64, width: usize, height: usize) -> Bitmap {
        let mut out = Bitmap::new(width, height, 0);
        for cy in 0..height {
            for cx in 0..width {
                out.data[cy * width + cx] = self.get(x + cx as i64, y + cy as i64);
            }
        }
        out
    }
}

/// Nominal template pixels (x, y), without the adaptive ones.
const TEMPLATES: [&[(i64, i64)]; 4] = [
    &[
        (-1, -2),
        (0, -2),
        (1, -2),
        (-2, -1),
        (-1, -1),
        (0, -1),
        (1, -1),
        (2, -1),
        (-4, 0),
        (-3, 0),
        (-2, 0),
        (-1, 0),
    ],
    &[
        (-1, -2),
        (0, -2),
        (1, -2),
        (2, -2),
        (-2, -1),
        (-1, -1),
        (0, -1),
        (1, -1),
        (2, -1),
        (-3, 0),
        (-2, 0),
        (-1, 0),
    ],
    &[
        (-1, -2),
        (0, -2),
        (1, -2),
        (-2, -1),
        (-1, -1),
        (0, -1),
        (1, -1),
        (2, -1),
        (-2, 0),
        (-1, 0),
    ],
    &[
        (-3, -1),
        (-2, -1),
        (-1, -1),
        (0, -1),
        (1, -1),
        (-4, 0),
        (-3, 0),
        (-2, 0),
        (-1, 0),
    ],
];

/// The context used to decode the "line is like the one above" bit.
const SLTP_CONTEXTS: [usize; 4] = [0x9b25, 0x0795, 0x00e5, 0x0195];

/// Parameters of one generic region decode.
pub struct Generic<'a> {
    pub width: usize,
    pub height: usize,
    pub template: u8,
    pub tpgdon: bool,
    /// Adaptive template pixels: four for template 0, one otherwise.
    pub at: &'a [(i64, i64)],
    /// Pixels to leave at 0 without decoding (halftone grids).
    pub skip: Option<&'a Bitmap>,
}

/// Decodes a generic region with the arithmetic decoder.
pub fn decode_generic(g: &Generic, d: &mut ArithDecoder, cx: &mut [u8]) -> Bitmap {
    let t = g.template.min(3) as usize;
    let mut pixels: Vec<(i64, i64)> = TEMPLATES[t].to_vec();
    let at_count = if t == 0 { 4 } else { 1 };
    // Adaptive pixels must lie in rows already decoded, or to the left.
    pixels.extend(g.at.iter().take(at_count).map(|&(x, y)| {
        if y > 0 || (y == 0 && x >= 0) {
            (-1, 0)
        } else {
            (x, y)
        }
    }));
    // Rows first, then columns: the order is ours, but it must not change
    // between the pixels of one decode.
    pixels.sort_by_key(|&(x, y)| (y, x));
    // Decode into a copy with a blank margin wide enough for every template
    // pixel, so reading neighbours needs no bounds checks.
    let left = pixels.iter().map(|p| (-p.0).max(0)).max().unwrap_or(0) as usize;
    let right = pixels.iter().map(|p| p.0.max(0)).max().unwrap_or(0) as usize;
    let top = pixels.iter().map(|p| (-p.1).max(0)).max().unwrap_or(0) as usize;
    let stride = left + g.width + right;
    let mut buf = vec![0u8; stride * (top + g.height)];
    let offsets: Vec<isize> = pixels
        .iter()
        .map(|&(x, y)| y as isize * stride as isize + x as isize)
        .collect();
    // The template's rows as (dy, first dx, last dx), when each row's
    // pixels are side by side (always so with the nominal adaptive pixels).
    let mut groups: Vec<(isize, isize, isize)> = Vec::new();
    let mut contiguous = true;
    for &(x, y) in &pixels {
        match groups.last_mut() {
            Some(g) if g.0 == y as isize => {
                contiguous &= x as isize == g.2 + 1;
                g.2 = x as isize;
            }
            _ => groups.push((y as isize, x as isize, x as isize)),
        }
    }
    let windows = (contiguous && groups.len() <= 4).then_some(groups);
    let mut ltp = 0u8;
    for y in 0..g.height {
        let row = (top + y) * stride + left;
        if g.tpgdon {
            ltp ^= d.bit(cx, SLTP_CONTEXTS[t]);
            if ltp == 1 {
                if y > 0 {
                    buf.copy_within(row - stride..row - stride + g.width, row);
                }
                continue;
            }
        }
        if let Some(groups) = &windows {
            // Each template row is a run of neighbouring pixels: keep its
            // bits in a register and slide it one pixel at a time.
            let mut regs = [0usize; 4];
            for (k, &(dy, lo, hi)) in groups.iter().enumerate() {
                let base = (row as isize + dy * stride as isize) as usize;
                for dx in lo..=hi {
                    regs[k] = (regs[k] << 1) | buf[(base as isize + dx) as usize] as usize;
                }
            }
            for x in 0..g.width {
                let skipped = g.skip.is_some_and(|s| s.get(x as i64, y as i64) == 1);
                if !skipped {
                    let mut context = 0usize;
                    for (k, &(_, lo, hi)) in groups.iter().enumerate() {
                        context = (context << (hi - lo + 1)) | regs[k];
                    }
                    buf[row + x] = d.bit(cx, context);
                }
                for (k, &(dy, lo, hi)) in groups.iter().enumerate() {
                    let next = (row as isize + dy * stride as isize + x as isize + 1 + hi) as usize;
                    let mask = (1usize << (hi - lo + 1)) - 1;
                    regs[k] = ((regs[k] << 1) | buf[next] as usize) & mask;
                }
            }
            continue;
        }
        for x in 0..g.width {
            if g.skip.is_some_and(|s| s.get(x as i64, y as i64) == 1) {
                continue;
            }
            let i = row + x;
            let mut context = 0usize;
            for &off in &offsets {
                context = (context << 1) | buf[(i as isize + off) as usize] as usize;
            }
            buf[i] = d.bit(cx, context);
        }
    }
    let mut out = Bitmap::new(g.width, g.height, 0);
    for y in 0..g.height {
        let row = (top + y) * stride + left;
        out.data[y * g.width..(y + 1) * g.width].copy_from_slice(&buf[row..row + g.width]);
    }
    out
}

/// Decodes an MMR-coded generic region. Returns the bitmap and how many
/// bytes of `data` it used.
pub fn decode_generic_mmr(width: usize, height: usize, data: &[u8]) -> (Bitmap, usize) {
    let params = CcittParams {
        k: -1,
        columns: width,
        rows: height,
        end_of_block: true,
        black_is_1: true,
        ..CcittParams::default()
    };
    let (packed, used) = decode_mmr(data, &params);
    let row_bytes = width.div_ceil(8);
    let mut out = Bitmap::new(width, height, 0);
    for y in 0..height {
        for x in 0..width {
            let byte = packed.get(y * row_bytes + x / 8).copied().unwrap_or(0);
            out.data[y * width + x] = (byte >> (7 - x % 8)) & 1;
        }
    }
    (out, used)
}

/// Template pixels for refinement: (pixels in the region being decoded,
/// pixels in the reference bitmap).
type Points = &'static [(i64, i64)];

const REFINEMENT: [(Points, Points); 2] = [
    (
        &[(0, -1), (1, -1), (-1, 0)],
        &[
            (0, -1),
            (1, -1),
            (-1, 0),
            (0, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ],
    ),
    (
        &[(-1, -1), (0, -1), (1, -1), (-1, 0)],
        &[(0, -1), (-1, 0), (0, 0), (1, 0), (0, 1), (1, 1)],
    ),
];

const REFINEMENT_SLTP: [usize; 2] = [0x0020, 0x0008];

pub struct Refinement<'a> {
    pub width: usize,
    pub height: usize,
    pub template: u8,
    pub reference: &'a Bitmap,
    pub dx: i64,
    pub dy: i64,
    pub tpgron: bool,
    /// Two adaptive pixels for template 0: one in the region, one in the
    /// reference.
    pub at: &'a [(i64, i64)],
}

/// Decodes a generic refinement region (6.3.5).
pub fn decode_refinement(r: &Refinement, d: &mut ArithDecoder, cx: &mut [u8]) -> Bitmap {
    let t = (r.template & 1) as usize;
    let mut coding: Vec<(i64, i64)> = REFINEMENT[t].0.to_vec();
    let mut reference: Vec<(i64, i64)> = REFINEMENT[t].1.to_vec();
    if t == 0 {
        if let Some(&a) = r.at.first() {
            coding.push(a);
        }
        if let Some(&a) = r.at.get(1) {
            reference.push(a);
        }
    }
    let mut out = Bitmap::new(r.width, r.height, 0);
    let mut ltp = 0u8;
    for y in 0..r.height as i64 {
        if r.tpgron {
            ltp ^= d.bit(cx, REFINEMENT_SLTP[t]);
        }
        for x in 0..r.width as i64 {
            let (rx, ry) = (x - r.dx, y - r.dy);
            if ltp == 1 {
                // Typical prediction: a pixel whose reference neighbourhood
                // is uniform copies it.
                let v = r.reference.get(rx, ry);
                let uniform =
                    (-1..=1).all(|j| (-1..=1).all(|i| r.reference.get(rx + i, ry + j) == v));
                if uniform {
                    if v != 0 {
                        out.set(x as usize, y as usize, 1);
                    }
                    continue;
                }
            }
            let mut context = 0usize;
            for &(dx, dy) in &coding {
                context = (context << 1) | out.get(x + dx, y + dy) as usize;
            }
            for &(dx, dy) in &reference {
                context = (context << 1) | r.reference.get(rx + dx, ry + dy) as usize;
            }
            if d.bit(cx, context) != 0 {
                out.set(x as usize, y as usize, 1);
            }
        }
    }
    out
}
