//! Pattern dictionaries (6.7) and halftone regions (6.6).

use super::arith::{ArithDecoder, contexts};
use super::region::{Bitmap, Combine, Generic, decode_generic, decode_generic_mmr};

pub struct PatternDict {
    pub mmr: bool,
    pub template: u8,
    pub width: usize,
    pub height: usize,
    pub gray_max: usize,
}

/// Decodes a pattern dictionary's patterns.
pub fn decode_patterns(p: &PatternDict, data: &[u8]) -> Result<Vec<Bitmap>, String> {
    let count = p.gray_max.checked_add(1).ok_or("too many patterns")?;
    let total = count.checked_mul(p.width).ok_or("patterns too large")?;
    if total.saturating_mul(p.height) > 1 << 26 || p.width == 0 || p.height == 0 {
        return Err("patterns too large".into());
    }
    let collective = if p.mmr {
        decode_generic_mmr(total, p.height, data).0
    } else {
        let at = [(-(p.width as i64), 0), (-3, -1), (2, -2), (-2, -2)];
        let mut d = ArithDecoder::new(data);
        let mut cx = contexts(1 << 16);
        decode_generic(
            &Generic {
                width: total,
                height: p.height,
                template: p.template,
                tpgdon: false,
                at: &at,
                skip: None,
            },
            &mut d,
            &mut cx,
        )
    };
    Ok((0..count)
        .map(|i| collective.crop((i * p.width) as i64, 0, p.width, p.height))
        .collect())
}

pub struct Halftone<'a> {
    pub width: usize,
    pub height: usize,
    pub mmr: bool,
    pub template: u8,
    pub enable_skip: bool,
    pub combine: Combine,
    pub default_pixel: u8,
    pub grid_width: usize,
    pub grid_height: usize,
    pub grid_x: i64,
    pub grid_y: i64,
    pub step_x: i64,
    pub step_y: i64,
    pub patterns: &'a [Bitmap],
}

/// Decodes a halftone region (6.6.5).
pub fn decode_halftone(h: &Halftone, data: &[u8]) -> Result<Bitmap, String> {
    let mut out = Bitmap::new(h.width, h.height, h.default_pixel);
    let first = h.patterns.first().ok_or("no patterns")?;
    let (pw, ph) = (first.width as i64, first.height as i64);
    let (gw, gh) = (h.grid_width, h.grid_height);
    if gw.saturating_mul(gh) > 1 << 26 {
        return Err("halftone grid too large".into());
    }
    let position = |m: usize, n: usize| -> (i64, i64) {
        let (m, n) = (m as i64, n as i64);
        (
            (h.grid_x + m * h.step_y + n * h.step_x) >> 8,
            (h.grid_y + m * h.step_x - n * h.step_y) >> 8,
        )
    };
    // Cells whose pattern would land wholly outside the region are skipped.
    let skip = h.enable_skip.then(|| {
        let mut s = Bitmap::new(gw, gh, 0);
        for m in 0..gh {
            for n in 0..gw {
                let (x, y) = position(m, n);
                if x + pw <= 0 || x >= h.width as i64 || y + ph <= 0 || y >= h.height as i64 {
                    s.set(n, m, 1);
                }
            }
        }
        s
    });
    // Gray-scale image decoding (annex C.5): bit planes, Gray coded.
    let mut bits = 0u32;
    while (1usize << bits) < h.patterns.len() {
        bits += 1;
    }
    let mut values = vec![0usize; gw * gh];
    let mut d = ArithDecoder::new(data);
    let mut cx = contexts(1 << 16);
    let mut offset = 0usize;
    let at = [
        (if h.template <= 1 { 3 } else { 2 }, -1),
        (-3, -1),
        (2, -2),
        (-2, -2),
    ];
    let mut previous: Option<Bitmap> = None;
    for j in (0..bits).rev() {
        let mut plane = if h.mmr {
            let (b, used) = decode_generic_mmr(gw, gh, data.get(offset..).unwrap_or(&[]));
            offset += used;
            b
        } else {
            decode_generic(
                &Generic {
                    width: gw,
                    height: gh,
                    template: h.template,
                    tpgdon: false,
                    at: &at,
                    skip: skip.as_ref(),
                },
                &mut d,
                &mut cx,
            )
        };
        if let Some(prev) = &previous {
            for (p, q) in plane.data.iter_mut().zip(&prev.data) {
                *p ^= q;
            }
        }
        for (v, &b) in values.iter_mut().zip(&plane.data) {
            *v |= usize::from(b) << j;
        }
        previous = Some(plane);
    }
    for m in 0..gh {
        for n in 0..gw {
            if skip
                .as_ref()
                .is_some_and(|s| s.get(n as i64, m as i64) == 1)
            {
                continue;
            }
            let (x, y) = position(m, n);
            let index = values[m * gw + n].min(h.patterns.len() - 1);
            out.compose(&h.patterns[index], x, y, h.combine);
        }
    }
    Ok(out)
}
