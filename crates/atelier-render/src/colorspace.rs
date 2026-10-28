//! Colour spaces (ISO 32000-2, 8.6): device, CIE-based, Indexed,
//! Separation, DeviceN and Pattern.

use std::sync::Arc;

use atelier_raster::Color;
use atelier_syntax::{Dict, Document, Object};

use crate::function::Function;

#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpace {
    Gray,
    Rgb,
    Cmyk,
    /// CIE L*a*b* with its white point.
    Lab {
        white: [f32; 3],
    },
    Indexed {
        base: Box<ColorSpace>,
        hival: u8,
        lookup: Vec<u8>,
    },
    /// Painting with a pattern. Uncoloured tiling patterns take their
    /// colour from `base`.
    Pattern {
        base: Option<Box<ColorSpace>>,
    },
    /// Separation or DeviceN: tints mapped by `tint` into `alt`. Without a
    /// usable tint transform, full tint is drawn as black.
    Spot {
        components: usize,
        none: bool,
        alt: Box<ColorSpace>,
        tint: Option<Arc<Function>>,
    },
}

impl ColorSpace {
    pub fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Indexed { .. } => 1,
            Self::Rgb | Self::Lab { .. } => 3,
            Self::Cmyk => 4,
            Self::Pattern { .. } => 1,
            Self::Spot { components, .. } => *components,
        }
    }

    /// The colour a space starts with after `cs`/`CS` (8.6.8).
    pub fn initial(&self) -> Vec<f32> {
        match self {
            Self::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Self::Spot { components, .. } => vec![1.0; *components],
            other => vec![0.0; other.components()],
        }
    }

    /// The default Decode range of one component for image samples.
    pub fn decode_range(&self, bits: u32) -> (f32, f32) {
        match self {
            Self::Indexed { .. } => (0.0, ((1u32 << bits) - 1) as f32),
            Self::Lab { .. } => (0.0, 100.0),
            _ => (0.0, 1.0),
        }
    }

    /// Converts components to sRGB. `None` means "paint nothing".
    pub fn to_rgb(&self, c: &[f32]) -> Option<Color> {
        let at = |i: usize| c.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        Some(match self {
            Self::Gray => Color::rgb(at(0), at(0), at(0)),
            Self::Rgb => Color::rgb(at(0), at(1), at(2)),
            Self::Cmyk => cmyk_to_rgb(at(0), at(1), at(2), at(3)),
            Self::Lab { white } => lab_to_rgb(
                c.first().copied().unwrap_or(0.0),
                c.get(1).copied().unwrap_or(0.0),
                c.get(2).copied().unwrap_or(0.0),
                *white,
            ),
            Self::Indexed {
                base,
                hival,
                lookup,
            } => {
                let index = (c.first().copied().unwrap_or(0.0).round().max(0.0) as usize)
                    .min(usize::from(*hival));
                let n = base.components();
                let raw = lookup.get(index * n..(index + 1) * n)?;
                let comps: Vec<f32> = match base.as_ref() {
                    // Lab lookup bytes map onto its ranges.
                    ColorSpace::Lab { .. } => vec![
                        f32::from(raw[0]) * 100.0 / 255.0,
                        f32::from(raw[1]) - 128.0,
                        f32::from(raw[2]) - 128.0,
                    ],
                    _ => raw.iter().map(|&b| f32::from(b) / 255.0).collect(),
                };
                return base.to_rgb(&comps);
            }
            Self::Pattern { .. } => return None,
            Self::Spot { none: true, .. } => return None,
            Self::Spot {
                alt, tint: Some(f), ..
            } => {
                let tints: Vec<f32> = c.iter().map(|v| v.clamp(0.0, 1.0)).collect();
                return alt.to_rgb(&f.eval(&tints));
            }
            Self::Spot { .. } => {
                let tint = c.iter().fold(0.0f32, |m, v| m.max(v.clamp(0.0, 1.0)));
                Color::rgb(1.0 - tint, 1.0 - tint, 1.0 - tint)
            }
        })
    }
}

/// A simple, profile-free CMYK conversion.
pub fn cmyk_to_rgb(c: f32, m: f32, y: f32, k: f32) -> Color {
    Color::rgb(
        (1.0 - c) * (1.0 - k),
        (1.0 - m) * (1.0 - k),
        (1.0 - y) * (1.0 - k),
    )
}

fn lab_to_rgb(l: f32, a: f32, b: f32, white: [f32; 3]) -> Color {
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
    let (x, y, z) = (white[0] * g(fx), white[1] * g(fy), white[2] * g(fz));
    // XYZ (D65) to linear sRGB, then gamma.
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let gg = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let bb = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    let gamma = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.003_130_8 {
            12.92 * v
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    Color::rgb(gamma(r), gamma(gg), gamma(bb))
}

/// Parses a colour space object, looking names up in `resources`.
pub fn parse(doc: &Document, obj: &Object, resources: &Dict) -> Option<ColorSpace> {
    parse_depth(doc, obj, resources, 0)
}

fn parse_depth(doc: &Document, obj: &Object, resources: &Dict, depth: usize) -> Option<ColorSpace> {
    if depth > 8 {
        return None;
    }
    let obj = doc.resolve(obj).ok()?;
    match &obj {
        Object::Name(n) => match n.as_bytes() {
            b"DeviceGray" | b"G" | b"CalGray" => Some(ColorSpace::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(ColorSpace::Rgb),
            b"DeviceCMYK" | b"CMYK" | b"CalCMYK" => Some(ColorSpace::Cmyk),
            b"Pattern" => Some(ColorSpace::Pattern { base: None }),
            _ => {
                let named = doc.get_key(resources, b"ColorSpace").ok()?;
                let entry = named.as_dict()?.get(n.as_bytes())?.clone();
                parse_depth(doc, &entry, resources, depth + 1)
            }
        },
        Object::Array(items) => {
            let family = doc.resolve(items.first()?).ok()?;
            match family.as_name()?.as_bytes() {
                b"DeviceGray" | b"CalGray" | b"G" => Some(ColorSpace::Gray),
                b"DeviceRGB" | b"CalRGB" | b"RGB" => Some(ColorSpace::Rgb),
                b"DeviceCMYK" | b"CMYK" => Some(ColorSpace::Cmyk),
                b"ICCBased" => {
                    let stream = doc.resolve(items.get(1)?).ok()?;
                    let dict = stream.as_dict()?;
                    if let Some(alt) = dict.get(b"Alternate") {
                        if let Some(cs) = parse_depth(doc, alt, resources, depth + 1) {
                            return Some(cs);
                        }
                    }
                    match doc.get_key(dict, b"N").ok()?.as_i64()? {
                        1 => Some(ColorSpace::Gray),
                        4 => Some(ColorSpace::Cmyk),
                        _ => Some(ColorSpace::Rgb),
                    }
                }
                b"Lab" => {
                    let params = doc.resolve(items.get(1)?).ok()?;
                    let white = params
                        .as_dict()
                        .and_then(|d| d.get(b"WhitePoint"))
                        .and_then(Object::as_array)
                        .map(|w| {
                            let v =
                                |i: usize| w.get(i).and_then(Object::as_f64).unwrap_or(1.0) as f32;
                            [v(0), v(1), v(2)]
                        })
                        .unwrap_or([0.9505, 1.0, 1.089]);
                    Some(ColorSpace::Lab { white })
                }
                b"Indexed" | b"I" => {
                    let base = parse_depth(doc, items.get(1)?, resources, depth + 1)?;
                    let hival = doc.resolve(items.get(2)?).ok()?.as_i64()?.clamp(0, 255) as u8;
                    let lookup = match doc.resolve(items.get(3)?).ok()? {
                        Object::String(s) => s.bytes,
                        Object::Stream(s) => doc.decode_stream(&s).ok()?.data,
                        _ => return None,
                    };
                    Some(ColorSpace::Indexed {
                        base: Box::new(base),
                        hival,
                        lookup,
                    })
                }
                b"Pattern" => Some(ColorSpace::Pattern {
                    base: items
                        .get(1)
                        .and_then(|b| parse_depth(doc, b, resources, depth + 1))
                        .map(Box::new),
                }),
                b"Separation" | b"DeviceN" => {
                    let separation = family.as_name()?.as_bytes() == b"Separation";
                    let names = doc.resolve(items.get(1)?).ok()?;
                    let (components, none) = if separation {
                        (1, names.as_name().is_some_and(|n| *n == b"None"))
                    } else {
                        let list = names.as_array().unwrap_or(&[]);
                        (
                            list.len().max(1),
                            !list.is_empty()
                                && list
                                    .iter()
                                    .all(|x| x.as_name().is_some_and(|n| *n == b"None")),
                        )
                    };
                    let alt = items
                        .get(2)
                        .and_then(|a| parse_depth(doc, a, resources, depth + 1))
                        .unwrap_or(ColorSpace::Gray);
                    let tint = items
                        .get(3)
                        .and_then(|f| Function::parse(doc, f))
                        .filter(|f| f.inputs() >= components)
                        .map(Arc::new);
                    Some(ColorSpace::Spot {
                        components,
                        none,
                        alt: Box::new(alt),
                        tint,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmyk_black_and_white() {
        assert_eq!(
            ColorSpace::Cmyk.to_rgb(&[0.0, 0.0, 0.0, 1.0]),
            Some(Color::BLACK)
        );
        assert_eq!(
            ColorSpace::Cmyk.to_rgb(&[0.0, 0.0, 0.0, 0.0]),
            Some(Color::WHITE)
        );
    }

    #[test]
    fn indexed_looks_up_base_colour() {
        let cs = ColorSpace::Indexed {
            base: Box::new(ColorSpace::Rgb),
            hival: 1,
            lookup: vec![255, 0, 0, 0, 0, 255],
        };
        assert_eq!(cs.to_rgb(&[1.0]), Some(Color::rgb(0.0, 0.0, 1.0)));
        assert_eq!(cs.to_rgb(&[7.0]), Some(Color::rgb(0.0, 0.0, 1.0)));
    }

    #[test]
    fn separation_uses_its_tint_transform() {
        let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
                    2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
                    trailer << /Root 1 0 R >>\n%%EOF\n";
        let doc = Document::open(pdf.to_vec()).unwrap();
        let obj = atelier_syntax::Parser::new(
            b"[/Separation /Orange /DeviceCMYK \
               << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0.5 1 0] /N 1 >>]",
        )
        .parse_object()
        .unwrap();
        let cs = parse(&doc, &obj, &Dict::new()).unwrap();
        assert_eq!(cs.to_rgb(&[0.0]), Some(Color::WHITE));
        assert_eq!(cs.to_rgb(&[1.0]), Some(Color::rgb(1.0, 0.5, 0.0)));
        // DeviceN with a PostScript tint transform is covered by the
        // function tests; /None paints nothing.
        let none = atelier_syntax::Parser::new(b"[/Separation /None /DeviceGray {}]")
            .parse_object()
            .unwrap();
        assert_eq!(
            parse(&doc, &none, &Dict::new()).unwrap().to_rgb(&[1.0]),
            None
        );
    }

    #[test]
    fn lab_white_is_white() {
        let c = ColorSpace::Lab {
            white: [0.9505, 1.0, 1.089],
        }
        .to_rgb(&[100.0, 0.0, 0.0])
        .unwrap();
        assert!(c.r > 0.98 && c.g > 0.98 && c.b > 0.98, "{c:?}");
    }
}
