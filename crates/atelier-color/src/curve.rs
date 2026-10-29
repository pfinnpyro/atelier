//! Tone curves: the ICC `curv` and `para` types.

#[derive(Debug, Clone, PartialEq)]
pub enum Curve {
    Identity,
    Gamma(f32),
    /// Samples spread evenly over 0..=1.
    Table(Vec<f32>),
    /// A parametric curve (ICC.1 table 65): type and up to seven values.
    Parametric(u16, [f32; 7]),
}

impl Curve {
    pub fn eval(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        let y = match self {
            Curve::Identity => x,
            Curve::Gamma(g) => x.powf(*g),
            Curve::Table(t) => match t.len() {
                0 => x,
                1 => t[0],
                n => {
                    let pos = x * (n - 1) as f32;
                    let i = (pos as usize).min(n - 2);
                    let f = pos - i as f32;
                    t[i] + (t[i + 1] - t[i]) * f
                }
            },
            Curve::Parametric(kind, p) => {
                let [g, a, b, c, d, e, f] = *p;
                let pow = |v: f32| if v <= 0.0 { 0.0 } else { v.powf(g) };
                match kind {
                    0 => pow(x),
                    1 => {
                        if a != 0.0 && x >= -b / a {
                            pow(a * x + b)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if a != 0.0 && x >= -b / a {
                            pow(a * x + b) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            pow(a * x + b)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            pow(a * x + b) + e
                        } else {
                            c * x + f
                        }
                    }
                }
            }
        };
        y.clamp(0.0, 1.0)
    }
}

fn s15(d: &[u8], at: usize) -> Option<f32> {
    let v = i32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?);
    Some(v as f32 / 65536.0)
}

/// Parses a `curv` or `para` element at the start of `d`. Returns the
/// curve and its length in bytes.
pub fn parse(d: &[u8]) -> Option<(Curve, usize)> {
    let u32_at =
        |i: usize| -> Option<u32> { Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?)) };
    match d.get(..4)? {
        b"curv" => {
            let n = u32_at(8)? as usize;
            let curve = match n {
                0 => Curve::Identity,
                1 => {
                    Curve::Gamma(f32::from(u16::from_be_bytes([*d.get(12)?, *d.get(13)?])) / 256.0)
                }
                _ => {
                    let bytes = d.get(12..12 + n.checked_mul(2)?)?;
                    Curve::Table(
                        bytes
                            .chunks_exact(2)
                            .map(|c| f32::from(u16::from_be_bytes([c[0], c[1]])) / 65535.0)
                            .collect(),
                    )
                }
            };
            Some((curve, 12 + 2 * n))
        }
        b"para" => {
            let kind = u16::from_be_bytes([*d.get(8)?, *d.get(9)?]);
            let count = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return None,
            };
            let mut p = [0f32; 7];
            for (i, v) in p.iter_mut().enumerate().take(count) {
                *v = s15(d, 12 + 4 * i)?;
            }
            Some((Curve::Parametric(kind, p), 12 + 4 * count))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_parametric_curve() {
        let c = Curve::Parametric(
            3,
            [
                2.4,
                1.0 / 1.055,
                0.055 / 1.055,
                1.0 / 12.92,
                0.04045,
                0.0,
                0.0,
            ],
        );
        assert!((c.eval(0.5) - 0.214).abs() < 1e-3);
        assert!((c.eval(0.02) - 0.02 / 12.92).abs() < 1e-6);
        assert_eq!(c.eval(1.0), 1.0);
    }

    #[test]
    fn tables_interpolate() {
        let c = Curve::Table(vec![0.0, 0.25, 1.0]);
        assert!((c.eval(0.25) - 0.125).abs() < 1e-6);
        assert!((c.eval(0.75) - 0.625).abs() < 1e-6);
    }
}
