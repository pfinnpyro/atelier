//! ICC profiles (ICC.1:2010 and v2): the device-to-PCS direction, enough
//! to show any input, display, output or colour space profile on screen.
//!
//! Supported: gray TRC, RGB matrix/TRC, and the lut8, lut16 and lutAtoB
//! transforms with any number of inputs, to an XYZ or Lab connection space.

use crate::curve::{self, Curve};
use crate::lut::Clut;
use crate::{D50, lab_to_xyz, xyz_d50_to_srgb};

/// The device colour space a profile describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSpace {
    Gray,
    Rgb,
    Cmyk,
    Lab,
    /// Any other space, by channel count.
    Other(usize),
}

impl ProfileSpace {
    pub fn components(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb | Self::Lab => 3,
            Self::Cmyk => 4,
            Self::Other(n) => n,
        }
    }
}

/// Rendering intents (ICC.1 7.2.15), which pick the A2B table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

#[derive(Debug, Clone, PartialEq)]
enum Pcs {
    Xyz,
    Lab,
}

/// How LUT outputs encode the PCS.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Encoding {
    /// lut8: one byte, Lab L 0..100 and a, b -128..127.
    Eight,
    /// lut16: the legacy 16-bit encoding (Lab 100 at 0xFF00).
    Legacy16,
    /// lutAtoB: the v4 encoding (Lab 100 at 0xFFFF).
    V4,
}

#[derive(Debug, Clone, PartialEq)]
struct Lut {
    a_curves: Vec<Curve>,
    clut: Option<Clut>,
    m_curves: Vec<Curve>,
    /// 3x3 matrix and offsets, applied after the M curves.
    matrix: Option<([[f32; 3]; 3], [f32; 3])>,
    b_curves: Vec<Curve>,
    encoding: Encoding,
}

#[derive(Debug, Clone, PartialEq)]
enum Pipeline {
    Gray(Curve),
    Matrix([Curve; 3], [[f32; 3]; 3]),
    Lut(Box<Lut>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub space: ProfileSpace,
    pcs: Pcs,
    pipeline: Pipeline,
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(i..i + 2)?.try_into().ok()?))
}

fn s15(d: &[u8], i: usize) -> Option<f32> {
    Some(i32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?) as f32 / 65536.0)
}

fn xyz_tag(d: &[u8]) -> Option<[f32; 3]> {
    if d.get(..4)? != b"XYZ " {
        return None;
    }
    Some([s15(d, 8)?, s15(d, 12)?, s15(d, 16)?])
}

fn lut8(d: &[u8]) -> Option<Lut> {
    let (i, o, g) = (
        usize::from(*d.get(8)?),
        usize::from(*d.get(9)?),
        usize::from(*d.get(10)?),
    );
    if i == 0 || o == 0 || i > 15 || g < 2 {
        return None;
    }
    let mut at = 48;
    let table = |d: &[u8], at: usize| -> Option<Curve> {
        Some(Curve::Table(
            d.get(at..at + 256)?
                .iter()
                .map(|&v| f32::from(v) / 255.0)
                .collect(),
        ))
    };
    let a_curves = (0..i)
        .map(|k| table(d, at + 256 * k))
        .collect::<Option<Vec<_>>>()?;
    at += 256 * i;
    let points = g.checked_pow(i as u32)?.checked_mul(o)?;
    let values = d
        .get(at..at.checked_add(points)?)?
        .iter()
        .map(|&v| f32::from(v) / 255.0)
        .collect();
    at += points;
    let b_curves = (0..o)
        .map(|k| table(d, at + 256 * k))
        .collect::<Option<Vec<_>>>()?;
    Some(Lut {
        a_curves,
        clut: Some(Clut::new(vec![g; i], o, values)?),
        m_curves: Vec::new(),
        matrix: None,
        b_curves,
        encoding: Encoding::Eight,
    })
}

fn lut16(d: &[u8]) -> Option<Lut> {
    let (i, o, g) = (
        usize::from(*d.get(8)?),
        usize::from(*d.get(9)?),
        usize::from(*d.get(10)?),
    );
    let n = usize::from(u16_at(d, 48)?);
    let m = usize::from(u16_at(d, 50)?);
    if i == 0 || o == 0 || i > 15 || g < 2 || n < 2 || m < 2 {
        return None;
    }
    let words = |at: usize, count: usize| -> Option<Vec<f32>> {
        let bytes = d.get(at..at.checked_add(count.checked_mul(2)?)?)?;
        Some(
            bytes
                .chunks_exact(2)
                .map(|c| f32::from(u16::from_be_bytes([c[0], c[1]])) / 65535.0)
                .collect(),
        )
    };
    let mut at = 52;
    let a_curves = (0..i)
        .map(|k| words(at + 2 * n * k, n).map(Curve::Table))
        .collect::<Option<Vec<_>>>()?;
    at += 2 * n * i;
    let points = g.checked_pow(i as u32)?.checked_mul(o)?;
    let values = words(at, points)?;
    at += 2 * points;
    let b_curves = (0..o)
        .map(|k| words(at + 2 * m * k, m).map(Curve::Table))
        .collect::<Option<Vec<_>>>()?;
    Some(Lut {
        a_curves,
        clut: Some(Clut::new(vec![g; i], o, values)?),
        m_curves: Vec::new(),
        matrix: None,
        b_curves,
        encoding: Encoding::Legacy16,
    })
}

/// A run of `count` curves starting at `at`, each padded to 4 bytes.
fn curves(d: &[u8], mut at: usize, count: usize) -> Option<Vec<Curve>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (c, len) = curve::parse(d.get(at..)?)?;
        out.push(c);
        at += len.div_ceil(4) * 4;
    }
    Some(out)
}

fn lut_atob(d: &[u8]) -> Option<Lut> {
    let (i, o) = (usize::from(*d.get(8)?), usize::from(*d.get(9)?));
    if i == 0 || o == 0 || i > 15 {
        return None;
    }
    let offset = |k: usize| u32_at(d, 12 + 4 * k).map(|v| v as usize);
    let (b, matrix_at, m, clut_at, a) =
        (offset(0)?, offset(1)?, offset(2)?, offset(3)?, offset(4)?);
    let b_curves = if b != 0 { curves(d, b, o)? } else { Vec::new() };
    let m_curves = if m != 0 { curves(d, m, o)? } else { Vec::new() };
    let a_curves = if a != 0 { curves(d, a, i)? } else { Vec::new() };
    let matrix = if matrix_at != 0 && o == 3 {
        let v = |k: usize| s15(d, matrix_at + 4 * k);
        Some((
            [
                [v(0)?, v(1)?, v(2)?],
                [v(3)?, v(4)?, v(5)?],
                [v(6)?, v(7)?, v(8)?],
            ],
            [v(9)?, v(10)?, v(11)?],
        ))
    } else {
        None
    };
    let clut = if clut_at != 0 {
        let grid: Vec<usize> = (0..i)
            .map(|k| d.get(clut_at + k).map(|&g| usize::from(g)))
            .collect::<Option<_>>()?;
        let precision = *d.get(clut_at + 16)?;
        let points = grid
            .iter()
            .try_fold(1usize, |n, &g| n.checked_mul(g))?
            .checked_mul(o)?;
        let start = clut_at + 20;
        let values: Vec<f32> = match precision {
            1 => d
                .get(start..start.checked_add(points)?)?
                .iter()
                .map(|&v| f32::from(v) / 255.0)
                .collect(),
            2 => d
                .get(start..start.checked_add(points.checked_mul(2)?)?)?
                .chunks_exact(2)
                .map(|c| f32::from(u16::from_be_bytes([c[0], c[1]])) / 65535.0)
                .collect(),
            _ => return None,
        };
        Some(Clut::new(grid, o, values)?)
    } else {
        // Without a CLUT, inputs pass straight to outputs.
        if i != o {
            return None;
        }
        None
    };
    Some(Lut {
        a_curves,
        clut,
        m_curves,
        matrix,
        b_curves,
        encoding: Encoding::V4,
    })
}

fn lut_tag(d: &[u8]) -> Option<Lut> {
    match d.get(..4)? {
        b"mft1" => lut8(d),
        b"mft2" => lut16(d),
        b"mAB " => lut_atob(d),
        _ => None,
    }
}

impl Profile {
    /// Parses a profile; `None` when it is damaged or uses something
    /// unsupported, in which case callers fall back to an alternate space.
    pub fn parse(data: &[u8], intent: Intent) -> Option<Profile> {
        if data.len() < 132 || data.get(36..40)? != b"acsp" {
            return None;
        }
        let space = match data.get(16..20)? {
            b"GRAY" => ProfileSpace::Gray,
            b"RGB " => ProfileSpace::Rgb,
            b"CMYK" => ProfileSpace::Cmyk,
            b"Lab " => ProfileSpace::Lab,
            b"CMY " | b"YCbr" | b"Yxy " | b"HSV " | b"HLS " | b"XYZ " | b"Luv " => {
                ProfileSpace::Other(3)
            }
            [b'2'..=b'9' | b'A'..=b'F', b'C', b'L', b'R'] => {
                let c = data[16];
                ProfileSpace::Other(usize::from(if c <= b'9' {
                    c - b'0'
                } else {
                    c - b'A' + 10
                }))
            }
            _ => return None,
        };
        let pcs = match data.get(20..24)? {
            b"XYZ " => Pcs::Xyz,
            b"Lab " => Pcs::Lab,
            _ => return None,
        };
        let count = u32_at(data, 128)? as usize;
        let mut tags = Vec::new();
        for k in 0..count.min(1024) {
            let at = 132 + 12 * k;
            let sig: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
            let off = u32_at(data, at + 4)? as usize;
            let len = u32_at(data, at + 8)? as usize;
            if let Some(body) = data.get(off..off.saturating_add(len)) {
                tags.push((sig, body));
            }
        }
        let tag = |sig: &[u8; 4]| tags.iter().find(|t| &t.0 == sig).map(|t| t.1);
        // A2B tables, preferring the intent's own.
        let order: [&[u8; 4]; 3] = match intent {
            Intent::Perceptual => [b"A2B0", b"A2B1", b"A2B2"],
            Intent::RelativeColorimetric | Intent::AbsoluteColorimetric => {
                [b"A2B1", b"A2B0", b"A2B2"]
            }
            Intent::Saturation => [b"A2B2", b"A2B0", b"A2B1"],
        };
        let lut = order.iter().find_map(|s| tag(s).and_then(lut_tag));
        let pipeline = if let Some(lut) = lut {
            if lut.a_curves.len() > space.components() {
                return None;
            }
            Pipeline::Lut(Box::new(lut))
        } else {
            match space {
                ProfileSpace::Gray => Pipeline::Gray(curve::parse(tag(b"kTRC")?)?.0),
                ProfileSpace::Rgb => {
                    let trc = |s: &[u8; 4]| curve::parse(tag(s)?).map(|c| c.0);
                    let col = |s: &[u8; 4]| tag(s).and_then(xyz_tag);
                    let (r, g, b) = (col(b"rXYZ")?, col(b"gXYZ")?, col(b"bXYZ")?);
                    Pipeline::Matrix(
                        [trc(b"rTRC")?, trc(b"gTRC")?, trc(b"bTRC")?],
                        [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]],
                    )
                }
                _ => return None,
            }
        };
        Some(Profile {
            space,
            pcs,
            pipeline,
        })
    }

    pub fn components(&self) -> usize {
        self.space.components()
    }

    /// The colour of device values `input` (each 0..=1; Lab profiles take
    /// L/100 and (a+128)/255) as XYZ relative to D50.
    pub fn to_xyz(&self, input: &[f32]) -> [f32; 3] {
        let at = |i: usize| input.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match &self.pipeline {
            Pipeline::Gray(trc) => {
                let y = trc.eval(at(0));
                [D50[0] * y, y, D50[2] * y]
            }
            Pipeline::Matrix(trc, m) => {
                let v = [trc[0].eval(at(0)), trc[1].eval(at(1)), trc[2].eval(at(2))];
                [
                    m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
                    m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
                    m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
                ]
            }
            Pipeline::Lut(lut) => self.lut_to_xyz(lut, input),
        }
    }

    fn lut_to_xyz(&self, lut: &Lut, input: &[f32]) -> [f32; 3] {
        let n = self.components();
        let mut v = [0f32; 16];
        for (k, slot) in v.iter_mut().enumerate().take(n.min(16)) {
            let x = input.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            *slot = lut.a_curves.get(k).map_or(x, |c| c.eval(x));
        }
        let mut out = [0f32; 16];
        match &lut.clut {
            Some(clut) => clut.eval(&v[..n.min(16)], &mut out),
            None => out = v,
        }
        let mut p = [out[0], out[1], out[2]];
        if lut.encoding == Encoding::V4 {
            for (k, c) in lut.m_curves.iter().enumerate().take(3) {
                p[k] = c.eval(p[k]);
            }
            if let Some((m, off)) = &lut.matrix {
                let q = p;
                for k in 0..3 {
                    p[k] =
                        (m[k][0] * q[0] + m[k][1] * q[1] + m[k][2] * q[2] + off[k]).clamp(0.0, 1.0);
                }
            }
        }
        for (k, c) in lut.b_curves.iter().enumerate().take(3) {
            p[k] = c.eval(p[k]);
        }
        match self.pcs {
            Pcs::Lab => {
                let scale = match lut.encoding {
                    Encoding::Legacy16 => 65535.0 / 65280.0,
                    _ => 1.0,
                };
                let l = p[0] * scale * 100.0;
                let a = p[1] * scale * 255.0 - 128.0;
                let b = p[2] * scale * 255.0 - 128.0;
                lab_to_xyz(l, a, b, D50)
            }
            // u1Fixed15: 1.0 is 0x8000.
            Pcs::Xyz => p.map(|v| v * 65535.0 / 32768.0),
        }
    }

    /// The colour of device values `input` in sRGB, each 0..=1.
    pub fn to_srgb(&self, input: &[f32]) -> [f32; 3] {
        xyz_d50_to_srgb(self.to_xyz(input))
    }
}
