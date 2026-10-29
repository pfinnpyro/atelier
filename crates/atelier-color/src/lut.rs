//! Colour lookup tables: a grid over the input channels, interpolated the
//! way Little CMS does (tetrahedral for three inputs, linear in the first
//! of four, multilinear otherwise).

#[derive(Debug, Clone, PartialEq)]
pub struct Clut {
    /// Grid points along each input, first input first.
    pub grid: Vec<usize>,
    pub outputs: usize,
    /// Values in 0..=1, the last input varying fastest.
    pub values: Vec<f32>,
}

impl Clut {
    /// Checks the table is the size its grid says.
    pub fn new(grid: Vec<usize>, outputs: usize, values: Vec<f32>) -> Option<Clut> {
        let points = grid
            .iter()
            .try_fold(1usize, |n, &g| if g < 2 { None } else { n.checked_mul(g) })?;
        (points.checked_mul(outputs)? == values.len() && outputs > 0).then_some(Clut {
            grid,
            outputs,
            values,
        })
    }

    fn strides(&self) -> Vec<usize> {
        let mut s = vec![self.outputs; self.grid.len()];
        for i in (0..self.grid.len().saturating_sub(1)).rev() {
            s[i] = s[i + 1] * self.grid[i + 1];
        }
        s
    }

    /// Looks up `input` (each 0..=1) into `out`.
    pub fn eval(&self, input: &[f32], out: &mut [f32]) {
        let n = self.grid.len();
        let strides = self.strides();
        // Cell origin and position within it, per input.
        let mut base = 0usize;
        let mut frac = [0f32; 16];
        let mut step = [0usize; 16];
        for i in 0..n.min(16) {
            let g = self.grid[i];
            let pos = input.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0) * (g - 1) as f32;
            let cell = (pos as usize).min(g - 2);
            base += cell * strides[i];
            frac[i] = pos - cell as f32;
            step[i] = strides[i];
        }
        let o = self.outputs;
        let v = &self.values;
        match n {
            3 => tetrahedral(
                v,
                base,
                [step[0], step[1], step[2]],
                [frac[0], frac[1], frac[2]],
                o,
                out,
            ),
            4 => {
                let mut lo = vec![0f32; o];
                let mut hi = vec![0f32; o];
                let s = [step[1], step[2], step[3]];
                let f = [frac[1], frac[2], frac[3]];
                tetrahedral(v, base, s, f, o, &mut lo);
                tetrahedral(v, base + step[0], s, f, o, &mut hi);
                for k in 0..o.min(out.len()) {
                    out[k] = lo[k] + (hi[k] - lo[k]) * frac[0];
                }
            }
            _ => {
                // Multilinear over the 2^n corners.
                for k in out.iter_mut() {
                    *k = 0.0;
                }
                for corner in 0..1usize << n {
                    let mut weight = 1.0;
                    let mut at = base;
                    for i in 0..n {
                        if corner >> i & 1 == 1 {
                            weight *= frac[i];
                            at += step[i];
                        } else {
                            weight *= 1.0 - frac[i];
                        }
                    }
                    if weight == 0.0 {
                        continue;
                    }
                    for k in 0..o.min(out.len()) {
                        out[k] += weight * v.get(at + k).copied().unwrap_or(0.0);
                    }
                }
            }
        }
    }
}

/// Tetrahedral interpolation in the cell at `base` (Little CMS's order of
/// cases, with x the first of the three inputs).
fn tetrahedral(v: &[f32], base: usize, s: [usize; 3], f: [f32; 3], o: usize, out: &mut [f32]) {
    let (rx, ry, rz) = (f[0], f[1], f[2]);
    let at = |dx: usize, dy: usize, dz: usize, k: usize| -> f32 {
        v.get(base + dx * s[0] + dy * s[1] + dz * s[2] + k)
            .copied()
            .unwrap_or(0.0)
    };
    for k in 0..o.min(out.len()) {
        let c0 = at(0, 0, 0, k);
        let (c1, c2, c3) = if rx >= ry && ry >= rz {
            (
                at(1, 0, 0, k) - c0,
                at(1, 1, 0, k) - at(1, 0, 0, k),
                at(1, 1, 1, k) - at(1, 1, 0, k),
            )
        } else if rx >= rz && rz >= ry {
            (
                at(1, 0, 0, k) - c0,
                at(1, 1, 1, k) - at(1, 0, 1, k),
                at(1, 0, 1, k) - at(1, 0, 0, k),
            )
        } else if rz >= rx && rx >= ry {
            (
                at(1, 0, 1, k) - at(0, 0, 1, k),
                at(1, 1, 1, k) - at(1, 0, 1, k),
                at(0, 0, 1, k) - c0,
            )
        } else if ry >= rx && rx >= rz {
            (
                at(1, 1, 0, k) - at(0, 1, 0, k),
                at(0, 1, 0, k) - c0,
                at(1, 1, 1, k) - at(1, 1, 0, k),
            )
        } else if ry >= rz && rz >= rx {
            (
                at(1, 1, 1, k) - at(0, 1, 1, k),
                at(0, 1, 0, k) - c0,
                at(0, 1, 1, k) - at(0, 1, 0, k),
            )
        } else {
            (
                at(1, 1, 1, k) - at(0, 1, 1, k),
                at(0, 1, 1, k) - at(0, 0, 1, k),
                at(0, 0, 1, k) - c0,
            )
        };
        out[k] = c0 + c1 * rx + c2 * ry + c3 * rz;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid sampling a linear function reproduces it anywhere.
    #[test]
    fn linear_functions_are_exact() {
        for n in 1..=5 {
            let grid = vec![3; n];
            let points = 3usize.pow(n as u32);
            let f = |x: &[f32]| {
                x.iter()
                    .enumerate()
                    .map(|(i, v)| v * (i + 1) as f32)
                    .sum::<f32>()
                    / 15.0
            };
            let mut values = Vec::new();
            for p in 0..points {
                let mut x = vec![0f32; n];
                let mut r = p;
                for i in (0..n).rev() {
                    x[i] = (r % 3) as f32 / 2.0;
                    r /= 3;
                }
                values.push(f(&x));
            }
            let clut = Clut::new(grid, 1, values).unwrap();
            let x: Vec<f32> = (0..n).map(|i| [0.1, 0.7, 0.33, 0.9, 0.5][i]).collect();
            let mut out = [0f32];
            clut.eval(&x, &mut out);
            assert!(
                (out[0] - f(&x)).abs() < 1e-5,
                "{n} inputs: {} vs {}",
                out[0],
                f(&x)
            );
        }
    }
}
