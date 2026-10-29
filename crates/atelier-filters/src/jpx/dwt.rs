//! Inverse discrete wavelet transforms (T.800 annex F): the reversible 5/3
//! filter on integers and the irreversible 9/7 filter on floats, both by
//! lifting with whole-sample symmetric extension at the edges.

/// One resolution level's place on its grid.
#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    pub fn width(&self) -> usize {
        (self.x1 - self.x0) as usize
    }

    pub fn height(&self) -> usize {
        (self.y1 - self.y0) as usize
    }
}

/// The four subbands one level of reconstruction combines.
pub struct Level<'a, T> {
    /// The resolution being rebuilt.
    pub rect: Rect,
    pub hl: &'a [T],
    pub lh: &'a [T],
    pub hh: &'a [T],
}

pub trait Sample: Copy + Default {
    /// Inverse 1-D transform of `len` samples spaced `step` apart in `v`
    /// (`count` such lines side by side, `line` apart), whose first sample
    /// sits at an even (low-pass) position when `even` is true.
    fn lift(v: &mut [Self], len: usize, even: bool, step: usize, count: usize, line: usize);
}

/// Index `i` of a line of `len` samples, mirrored at both ends.
#[inline]
fn mirror(i: isize, len: usize) -> usize {
    let n = len as isize;
    if n == 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut i = i.rem_euclid(period);
    if i >= n {
        i = period - i;
    }
    i as usize
}

/// Runs one lifting step: every sample at a position of parity `odd`
/// (relative to the line start) gets `f(itself, left, right)`.
#[inline]
fn step<T: Copy>(
    v: &mut [T],
    len: usize,
    first: usize,
    stride: usize,
    count: usize,
    line: usize,
    f: impl Fn(T, T, T) -> T,
) {
    let mut i = first;
    while i < len {
        let l = mirror(i as isize - 1, len);
        let r = mirror(i as isize + 1, len);
        for k in 0..count {
            let base = k * line;
            let (a, b, c) = (
                v[base + i * stride],
                v[base + l * stride],
                v[base + r * stride],
            );
            v[base + i * stride] = f(a, b, c);
        }
        i += 2;
    }
}

impl Sample for i32 {
    fn lift(v: &mut [i32], len: usize, even: bool, stride: usize, count: usize, line: usize) {
        // Positions with an even global index hold low-pass samples.
        let low = usize::from(!even);
        let high = 1 - low;
        if len == 1 {
            if !even {
                for k in 0..count {
                    v[k * line] /= 2;
                }
            }
            return;
        }
        step(v, len, low, stride, count, line, |x, a, b| {
            x - ((a + b + 2) >> 2)
        });
        step(v, len, high, stride, count, line, |x, a, b| {
            x + ((a + b) >> 1)
        });
    }
}

const ALPHA: f32 = -1.586_134_3;
const BETA: f32 = -0.052_980_118;
const GAMMA: f32 = 0.882_911_1;
const DELTA: f32 = 0.443_506_87;
const K: f32 = 1.230_174_1;

impl Sample for f32 {
    fn lift(v: &mut [f32], len: usize, even: bool, stride: usize, count: usize, line: usize) {
        let low = usize::from(!even);
        let high = 1 - low;
        if len == 1 {
            if !even {
                for k in 0..count {
                    v[k * line] /= 2.0;
                }
            }
            return;
        }
        for i in 0..len {
            let s = if i % 2 == low { K } else { 1.0 / K };
            for k in 0..count {
                v[k * line + i * stride] *= s;
            }
        }
        step(v, len, low, stride, count, line, |x, a, b| {
            x - DELTA * (a + b)
        });
        step(v, len, high, stride, count, line, |x, a, b| {
            x - GAMMA * (a + b)
        });
        step(v, len, low, stride, count, line, |x, a, b| {
            x - BETA * (a + b)
        });
        step(v, len, high, stride, count, line, |x, a, b| {
            x - ALPHA * (a + b)
        });
    }
}

/// Rebuilds one resolution from the one below (`ll`, sized for
/// `prev`) and the level's three high-pass subbands.
pub fn inverse_level<T: Sample>(ll: &[T], prev: Rect, level: &Level<T>) -> Vec<T> {
    let r = level.rect;
    let (w, h) = (r.width(), r.height());
    let mut out = vec![T::default(); w * h];
    if w == 0 || h == 0 {
        return out;
    }
    // Low-pass samples sit at even global positions; their indices within
    // the subbands start from the first such position.
    let low_x0 = r.x0.div_ceil(2);
    let high_x0 = r.x0 / 2;
    let low_y0 = r.y0.div_ceil(2);
    let high_y0 = r.y0 / 2;
    let low_w = (r.x1.div_ceil(2) - low_x0) as usize;
    let high_w = (r.x1 / 2 - high_x0) as usize;
    debug_assert_eq!(low_w, prev.width());
    for y in 0..h {
        let gy = r.y0 + y as u32;
        let (row_low, sy) = if gy % 2 == 0 {
            (true, (gy / 2 - low_y0) as usize)
        } else {
            (false, (gy / 2 - high_y0) as usize)
        };
        for x in 0..w {
            let gx = r.x0 + x as u32;
            let v = if gx % 2 == 0 {
                let sx = (gx / 2 - low_x0) as usize;
                if row_low {
                    ll.get(sy * low_w + sx)
                } else {
                    level.lh.get(sy * low_w + sx)
                }
            } else {
                let sx = (gx / 2 - high_x0) as usize;
                if row_low {
                    level.hl.get(sy * high_w + sx)
                } else {
                    level.hh.get(sy * high_w + sx)
                }
            };
            out[y * w + x] = v.copied().unwrap_or_default();
        }
    }
    // Rows, then columns.
    let even_x = r.x0 % 2 == 0;
    for y in 0..h {
        T::lift(&mut out[y * w..(y + 1) * w], w, even_x, 1, 1, 0);
    }
    T::lift(&mut out, h, r.y0 % 2 == 0, w, w, 1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_reflects_at_both_ends() {
        let got: Vec<usize> = (-3..8).map(|i| mirror(i, 5)).collect();
        assert_eq!(got, [3, 2, 1, 0, 1, 2, 3, 4, 3, 2, 1]);
    }

    #[test]
    fn reversible_one_level_round_trip() {
        // Forward 5/3 on a row starting at an even position, then inverse.
        let x = [10i32, 20, 30, 25, 5, 0, 100];
        let n = x.len();
        let mut y = x;
        // Forward lifting: high first, then low.
        for i in (1..n).step_by(2) {
            let a = x[mirror(i as isize - 1, n)];
            let b = x[mirror(i as isize + 1, n)];
            y[i] = x[i] - ((a + b) >> 1);
        }
        let yc = y;
        for i in (0..n).step_by(2) {
            let a = yc[mirror(i as isize - 1, n)];
            let b = yc[mirror(i as isize + 1, n)];
            y[i] = yc[i] + ((a + b + 2) >> 2);
        }
        i32::lift(&mut y, n, true, 1, 1, 0);
        assert_eq!(y, x);
    }
}
