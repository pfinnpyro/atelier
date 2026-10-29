//! Blend modes (ISO 32000-2, 11.3.5).

/// How a source colour mixes with the backdrop under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// The mode for a PDF name such as `Multiply`; `None` if unknown.
    /// `Compatible` is an old name for `Normal`.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"Normal" | b"Compatible" => Self::Normal,
            b"Multiply" => Self::Multiply,
            b"Screen" => Self::Screen,
            b"Overlay" => Self::Overlay,
            b"Darken" => Self::Darken,
            b"Lighten" => Self::Lighten,
            b"ColorDodge" => Self::ColorDodge,
            b"ColorBurn" => Self::ColorBurn,
            b"HardLight" => Self::HardLight,
            b"SoftLight" => Self::SoftLight,
            b"Difference" => Self::Difference,
            b"Exclusion" => Self::Exclusion,
            b"Hue" => Self::Hue,
            b"Saturation" => Self::Saturation,
            b"Color" => Self::Color,
            b"Luminosity" => Self::Luminosity,
            _ => return None,
        })
    }

    /// The blended colour of straight-alpha backdrop `b` and source `s`.
    fn mix(self, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
        match self {
            Self::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
            Self::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
            Self::Color => set_lum(s, lum(b)),
            Self::Luminosity => set_lum(b, lum(s)),
            _ => [0, 1, 2].map(|i| self.separable(b[i], s[i])),
        }
    }

    fn separable(self, b: f32, s: f32) -> f32 {
        match self {
            Self::Normal => s,
            Self::Multiply => b * s,
            Self::Screen => b + s - b * s,
            Self::Overlay => Self::HardLight.separable(s, b),
            Self::Darken => b.min(s),
            Self::Lighten => b.max(s),
            Self::ColorDodge => {
                if b == 0.0 {
                    0.0
                } else if s >= 1.0 {
                    1.0
                } else {
                    (b / (1.0 - s)).min(1.0)
                }
            }
            Self::ColorBurn => {
                if b >= 1.0 {
                    1.0
                } else if s <= 0.0 {
                    0.0
                } else {
                    1.0 - ((1.0 - b) / s).min(1.0)
                }
            }
            Self::HardLight => {
                if s <= 0.5 {
                    b * 2.0 * s
                } else {
                    let t = 2.0 * s - 1.0;
                    b + t - b * t
                }
            }
            Self::SoftLight => {
                if s <= 0.5 {
                    b - (1.0 - 2.0 * s) * b * (1.0 - b)
                } else {
                    let d = if b <= 0.25 {
                        ((16.0 * b - 12.0) * b + 4.0) * b
                    } else {
                        b.sqrt()
                    };
                    b + (2.0 * s - 1.0) * (d - b)
                }
            }
            Self::Difference => (b - s).abs(),
            Self::Exclusion => b + s - 2.0 * b * s,
            Self::Hue | Self::Saturation | Self::Color | Self::Luminosity => s,
        }
    }

    /// Composites premultiplied source `s` over premultiplied backdrop `d`
    /// with this mode. Both in 0..=255.
    pub(crate) fn composite(self, d: [u8; 4], s: [u32; 4]) -> [u8; 4] {
        let a_s = s[3] as f32 / 255.0;
        let a_b = f32::from(d[3]) / 255.0;
        if a_s <= 0.0 {
            return d;
        }
        let straight = |v: f32, a: f32| if a > 0.0 { (v / a).min(1.0) } else { 0.0 };
        let cs = [0, 1, 2].map(|i| s[i] as f32 / 255.0);
        let cb = [0, 1, 2].map(|i| f32::from(d[i]) / 255.0);
        let mixed = self.mix(cb.map(|v| straight(v, a_b)), cs.map(|v| straight(v, a_s)));
        let mut out = [0u8; 4];
        for i in 0..3 {
            let v = cs[i] * (1.0 - a_b) + cb[i] * (1.0 - a_s) + a_s * a_b * mixed[i];
            out[i] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        out[3] = ((a_s + a_b - a_s * a_b).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        out
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut c = c;
    if n < 0.0 && l - n > 0.0 {
        c = c.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 && x - l > 0.0 {
        c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    c
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    if max > min {
        c.map(|v| (v - min) * s / (max - min))
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiply_over_opaque() {
        // 50% grey multiplied onto opaque red keeps only half the red.
        let out = BlendMode::Multiply.composite([255, 0, 0, 255], [128, 128, 128, 255]);
        assert_eq!(out, [128, 0, 0, 255]);
    }

    #[test]
    fn modes_over_nothing_are_the_source() {
        for mode in [BlendMode::Screen, BlendMode::Difference, BlendMode::Hue] {
            assert_eq!(
                mode.composite([0, 0, 0, 0], [10, 20, 30, 128]),
                [10, 20, 30, 128]
            );
        }
    }

    #[test]
    fn screen_and_difference() {
        let d = [100, 200, 0, 255];
        let s = [100, 100, 255, 255];
        assert_eq!(BlendMode::Screen.composite(d, s), [161, 222, 255, 255]);
        assert_eq!(BlendMode::Difference.composite(d, s), [0, 100, 255, 255]);
    }

    #[test]
    fn luminosity_keeps_the_backdrop_hue() {
        // Grey source over red: still red-ish, as light as the grey.
        let out = BlendMode::Luminosity.composite([255, 0, 0, 255], [200, 200, 200, 255]);
        assert!(out[0] > out[1] && out[1] == out[2], "{out:?}");
        let l = 0.3 * f32::from(out[0]) + 0.59 * f32::from(out[1]) + 0.11 * f32::from(out[2]);
        assert!((l - 200.0).abs() < 2.0, "{l}");
    }
}
