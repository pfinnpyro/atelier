//! Colour management for rendering: ICC profiles (ICC.1, versions 2 and 4)
//! and the CIE-based PDF colour spaces, converted to sRGB.
//!
//! Everything goes through the ICC profile connection space: XYZ relative
//! to D50. Other white points are adapted with the Bradford transform.

mod curve;
mod icc;
mod lut;

pub use icc::{Intent, Profile, ProfileSpace};

/// The D50 white point of the profile connection space.
pub const D50: [f32; 3] = [0.9642, 1.0, 0.8249];
/// The D65 white point of sRGB.
pub const D65: [f32; 3] = [0.9505, 1.0, 1.089];

type Matrix = [[f32; 3]; 3];

fn mul(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn mul_m(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn invert(m: &Matrix) -> Option<Matrix> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |a: usize, b: usize, c: usize, d: usize| m[a][b] * m[c][d];
    Some([
        [
            (c(1, 1, 2, 2) - c(1, 2, 2, 1)) / det,
            (c(0, 2, 2, 1) - c(0, 1, 2, 2)) / det,
            (c(0, 1, 1, 2) - c(0, 2, 1, 1)) / det,
        ],
        [
            (c(1, 2, 2, 0) - c(1, 0, 2, 2)) / det,
            (c(0, 0, 2, 2) - c(0, 2, 2, 0)) / det,
            (c(0, 2, 1, 0) - c(0, 0, 1, 2)) / det,
        ],
        [
            (c(1, 0, 2, 1) - c(1, 1, 2, 0)) / det,
            (c(0, 1, 2, 0) - c(0, 0, 2, 1)) / det,
            (c(0, 0, 1, 1) - c(0, 1, 1, 0)) / det,
        ],
    ])
}

const BRADFORD: Matrix = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// The Bradford chromatic adaptation from one white point to another.
pub fn adaptation(from: [f32; 3], to: [f32; 3]) -> [[f32; 3]; 3] {
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (Some(inv), true) = (invert(&BRADFORD), from[1] > 0.0) else {
        return identity;
    };
    let s = mul(&BRADFORD, from);
    let d = mul(&BRADFORD, to);
    if s.iter().any(|v| v.abs() < 1e-9) {
        return identity;
    }
    let scale = [
        [d[0] / s[0], 0.0, 0.0],
        [0.0, d[1] / s[1], 0.0],
        [0.0, 0.0, d[2] / s[2]],
    ];
    mul_m(&inv, &mul_m(&scale, &BRADFORD))
}

/// XYZ relative to D50 to linear sRGB (the sRGB primaries, Bradford-adapted
/// to D50, inverted).
const XYZ_D50_TO_LINEAR_SRGB: Matrix = [
    [3.133_856, -1.616_866_7, -0.490_614_6],
    [-0.978_768_4, 1.916_141_5, 0.033_454],
    [0.071_945_3, -0.228_991_4, 1.405_242_7],
];

/// The sRGB transfer function, from linear light.
pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// XYZ relative to D50 to sRGB components in 0..=1.
pub fn xyz_d50_to_srgb(xyz: [f32; 3]) -> [f32; 3] {
    let l = mul(&XYZ_D50_TO_LINEAR_SRGB, xyz);
    [srgb_encode(l[0]), srgb_encode(l[1]), srgb_encode(l[2])]
}

/// XYZ relative to `white` to sRGB.
pub fn xyz_to_srgb(xyz: [f32; 3], white: [f32; 3]) -> [f32; 3] {
    xyz_d50_to_srgb(mul(&adaptation(white, D50), xyz))
}

/// CIE L*a*b* relative to `white` to XYZ relative to the same white.
pub fn lab_to_xyz(l: f32, a: f32, b: f32, white: [f32; 3]) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let g = |t: f32| {
        if t > 6.0 / 29.0 {
            t * t * t
        } else {
            3.0 * (6.0f32 / 29.0).powi(2) * (t - 4.0 / 29.0)
        }
    };
    [white[0] * g(fx), white[1] * g(fy), white[2] * g(fz)]
}

/// CIE L*a*b* relative to `white` to sRGB.
pub fn lab_to_srgb(l: f32, a: f32, b: f32, white: [f32; 3]) -> [f32; 3] {
    xyz_to_srgb(lab_to_xyz(l, a, b, white), white)
}

/// A CalRGB colour space (ISO 32000-2, 8.6.5.3).
#[derive(Debug, Clone, PartialEq)]
pub struct CalRgb {
    pub white: [f32; 3],
    pub gamma: [f32; 3],
    /// Columns are the X Y Z of the A, B and C components.
    pub matrix: [f32; 9],
}

impl CalRgb {
    pub fn to_srgb(&self, c: [f32; 3]) -> [f32; 3] {
        let a = c.map(|v| v.clamp(0.0, 1.0));
        let p = [
            a[0].powf(self.gamma[0]),
            a[1].powf(self.gamma[1]),
            a[2].powf(self.gamma[2]),
        ];
        let m = &self.matrix;
        let xyz = [
            m[0] * p[0] + m[3] * p[1] + m[6] * p[2],
            m[1] * p[0] + m[4] * p[1] + m[7] * p[2],
            m[2] * p[0] + m[5] * p[1] + m[8] * p[2],
        ];
        xyz_to_srgb(xyz, self.white)
    }
}

/// A CalGray colour space (ISO 32000-2, 8.6.5.2).
#[derive(Debug, Clone, PartialEq)]
pub struct CalGray {
    pub white: [f32; 3],
    pub gamma: f32,
}

impl CalGray {
    pub fn to_srgb(&self, a: f32) -> [f32; 3] {
        let y = a.clamp(0.0, 1.0).powf(self.gamma);
        // The white point's chromaticity at luminance y: neutral after
        // adaptation, so every channel is the same.
        let v = srgb_encode(y);
        [v, v, v]
    }
}

/// sRGB values of the SWOP process inks and their overprints, which
/// unmanaged DeviceCMYK is shown as (the published values for process
/// cyan #00AEEF, magenta #EC008C, yellow #FFF200, black #231F20 and the
/// two-ink overprints). Indexed by C M Y bits, high to low.
const INKS: [[f32; 3]; 8] = [
    [255.0, 255.0, 255.0],
    [255.0, 242.0, 0.0],
    [236.0, 0.0, 140.0],
    [237.0, 28.0, 36.0],
    [0.0, 174.0, 239.0],
    [0.0, 166.0, 81.0],
    [46.0, 49.0, 146.0],
    [45.0, 42.0, 45.0],
];
const BLACK_INK: [f32; 3] = [35.0, 31.0, 32.0];

/// DeviceCMYK as a press would print it, without a profile: interpolates
/// between the process inks and their overprints, with black ink darkening
/// whatever is under it. Close to what viewers show with a SWOP profile.
pub fn cmyk_to_srgb(c: f32, m: f32, y: f32, k: f32) -> [f32; 3] {
    let (c, m, y, k) = (
        c.clamp(0.0, 1.0),
        m.clamp(0.0, 1.0),
        y.clamp(0.0, 1.0),
        k.clamp(0.0, 1.0),
    );
    let mut rgb = [0f32; 3];
    for (i, ink) in INKS.iter().enumerate() {
        let w = if i & 4 != 0 { c } else { 1.0 - c }
            * if i & 2 != 0 { m } else { 1.0 - m }
            * if i & 1 != 0 { y } else { 1.0 - y };
        if w == 0.0 {
            continue;
        }
        for ch in 0..3 {
            rgb[ch] += w * ink[ch];
        }
    }
    let mut out = [0f32; 3];
    for ch in 0..3 {
        let under = rgb[ch] / 255.0;
        // Black ink over a colour: the colour times black's own tint.
        out[ch] = under * (1.0 - k + k * BLACK_INK[ch] / 255.0);
    }
    out
}

/// A colour conversion sampled on a grid, to convert many pixels quickly:
/// images in profile-based spaces interpolate in this instead of running
/// the full conversion for every pixel.
#[derive(Debug, Clone)]
pub struct Sampled {
    clut: lut::Clut,
}

impl Sampled {
    /// Samples `f` (taking `inputs` values in 0..=1) at `points` grid
    /// positions along each input.
    pub fn new(inputs: usize, points: usize, f: impl Fn(&[f32]) -> [f32; 3]) -> Option<Sampled> {
        if inputs == 0 || inputs > 8 || points < 2 {
            return None;
        }
        let total = points.checked_pow(inputs as u32)?;
        let mut values = Vec::with_capacity(total * 3);
        let mut x = vec![0f32; inputs];
        for p in 0..total {
            let mut r = p;
            for i in (0..inputs).rev() {
                x[i] = (r % points) as f32 / (points - 1) as f32;
                r /= points;
            }
            values.extend_from_slice(&f(&x));
        }
        Some(Sampled {
            clut: lut::Clut::new(vec![points; inputs], 3, values)?,
        })
    }

    pub fn eval(&self, input: &[f32]) -> [f32; 3] {
        let mut out = [0f32; 3];
        self.clut.eval(input, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_black_map_to_srgb_extremes() {
        let w = xyz_d50_to_srgb(D50);
        assert!(w.iter().all(|&v| (v - 1.0).abs() < 2e-3), "{w:?}");
        assert_eq!(xyz_d50_to_srgb([0.0; 3]), [0.0; 3]);
        // D65 white adapts to D50 white.
        let a = mul(&adaptation(D65, D50), D65);
        assert!(
            a.iter().zip(D50).all(|(x, y)| (x - y).abs() < 1e-3),
            "{a:?}"
        );
        let lab = lab_to_srgb(100.0, 0.0, 0.0, D65);
        assert!(lab.iter().all(|&v| v > 0.998), "{lab:?}");
    }

    #[test]
    fn process_inks() {
        let to8 = |v: [f32; 3]| v.map(|x| (x * 255.0).round() as u8);
        assert_eq!(to8(cmyk_to_srgb(0.0, 0.0, 0.0, 0.0)), [255, 255, 255]);
        assert_eq!(to8(cmyk_to_srgb(1.0, 0.0, 0.0, 0.0)), [0, 174, 239]);
        assert_eq!(to8(cmyk_to_srgb(0.0, 0.0, 0.0, 1.0)), [35, 31, 32]);
        let rich = to8(cmyk_to_srgb(0.6, 0.4, 0.4, 1.0));
        assert!(rich.iter().all(|&v| v < 30), "{rich:?}");
    }

    #[test]
    fn sampled_grids_follow_the_function() {
        let f = |x: &[f32]| [x[0] * x[1], x[2], 1.0 - x[0]];
        let s = Sampled::new(3, 17, f).unwrap();
        let v = s.eval(&[0.3, 0.6, 0.9]);
        let e = f(&[0.3, 0.6, 0.9]);
        assert!(v.iter().zip(e).all(|(a, b)| (a - b).abs() < 2e-3), "{v:?}");
    }

    #[test]
    fn calrgb_with_srgb_parameters_is_close_to_srgb() {
        // sRGB primaries (D65) with gamma 2.2: mid grey stays neutral.
        let cal = CalRgb {
            white: D65,
            gamma: [2.2; 3],
            matrix: [
                0.4124, 0.2126, 0.0193, 0.3576, 0.7152, 0.1192, 0.1805, 0.0722, 0.9505,
            ],
        };
        let g = cal.to_srgb([0.5; 3]);
        assert!(
            (g[0] - g[1]).abs() < 0.01 && (g[1] - g[2]).abs() < 0.01,
            "{g:?}"
        );
        let red = cal.to_srgb([1.0, 0.0, 0.0]);
        assert!(red[0] > 0.99 && red[1] < 0.02 && red[2] < 0.02, "{red:?}");
    }
}
