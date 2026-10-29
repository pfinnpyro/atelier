//! Colour spaces (ISO 32000-2, 8.6): device, CIE-based, Indexed,
//! Separation, DeviceN and Pattern.

use std::sync::Arc;

use atelier_color::{CalGray, CalRgb, Intent, Profile, ProfileSpace};
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
    CalGray(CalGray),
    CalRgb(Arc<CalRgb>),
    /// An ICC profile (ICCBased, an output intent, or a JPEG 2000 image's
    /// own), with values in 0..=1.
    Icc(Arc<Profile>),
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
        /// The colorant names: one for Separation, one each for DeviceN.
        names: Vec<String>,
        /// Separation rather than DeviceN.
        separation: bool,
        alt: Box<ColorSpace>,
        tint: Option<Arc<Function>>,
    },
}

impl ColorSpace {
    /// The space in a few words, as the file names it: "DeviceCMYK",
    /// "Separation PANTONE 185 C (alternate DeviceCMYK)".
    pub fn describe(&self) -> String {
        match self {
            Self::Gray => "DeviceGray".into(),
            Self::Rgb => "DeviceRGB".into(),
            Self::Cmyk => "DeviceCMYK".into(),
            Self::Lab { .. } => "Lab".into(),
            Self::CalGray(_) => "CalGray".into(),
            Self::CalRgb(_) => "CalRGB".into(),
            Self::Icc(p) => format!(
                "ICCBased ({})",
                match p.components() {
                    1 => "grey",
                    3 => "RGB or Lab",
                    4 => "CMYK",
                    _ => "other",
                }
            ),
            Self::Indexed { base, hival, .. } => {
                format!(
                    "Indexed, {} colours of {}",
                    u16::from(*hival) + 1,
                    base.describe()
                )
            }
            Self::Pattern { .. } => "Pattern".into(),
            Self::Spot {
                names,
                separation,
                alt,
                ..
            } => {
                let kind = if *separation { "Separation" } else { "DeviceN" };
                format!("{kind} {} (alternate {})", names.join(", "), alt.describe())
            }
        }
    }

    /// The colorant names of a Separation or DeviceN space, one per
    /// component; empty for other spaces.
    pub fn colorants(&self) -> &[String] {
        match self {
            Self::Spot { names, .. } => names,
            _ => &[],
        }
    }

    /// The spot colour names this space paints with, if any: its
    /// colorants other than the process inks and None/All.
    pub fn spot_names(&self) -> Vec<String> {
        match self {
            Self::Spot { names, .. } => names
                .iter()
                .filter(|n| {
                    !matches!(
                        n.as_str(),
                        "None" | "All" | "Cyan" | "Magenta" | "Yellow" | "Black"
                    )
                })
                .cloned()
                .collect(),
            Self::Indexed { base, .. } => base.spot_names(),
            Self::Pattern { base: Some(b) } => b.spot_names(),
            _ => Vec::new(),
        }
    }

    /// For a spot colour, the alternate space and the tint mapped into it.
    pub fn alternate(&self, c: &[f32]) -> Option<(&ColorSpace, Vec<f32>)> {
        match self {
            Self::Spot {
                alt, tint: Some(f), ..
            } => {
                let tints: Vec<f32> = c.iter().map(|v| v.clamp(0.0, 1.0)).collect();
                Some((&**alt, f.eval(&tints)))
            }
            _ => None,
        }
    }

    pub fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Indexed { .. } | Self::CalGray(_) => 1,
            Self::Rgb | Self::Lab { .. } | Self::CalRgb(_) => 3,
            Self::Icc(p) => p.components(),
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
            Self::Lab { white } => srgb(atelier_color::lab_to_srgb(
                c.first().copied().unwrap_or(0.0),
                c.get(1).copied().unwrap_or(0.0),
                c.get(2).copied().unwrap_or(0.0),
                *white,
            )),
            Self::CalGray(cal) => srgb(cal.to_srgb(at(0))),
            Self::CalRgb(cal) => srgb(cal.to_srgb([at(0), at(1), at(2)])),
            Self::Icc(p) => {
                let mut v = [0f32; 16];
                for (i, slot) in v.iter_mut().enumerate().take(p.components().min(16)) {
                    *slot = at(i);
                }
                srgb(p.to_srgb(&v[..p.components().min(16)]))
            }
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

/// Unmanaged DeviceCMYK, shown the way a press prints it.
pub fn cmyk_to_rgb(c: f32, m: f32, y: f32, k: f32) -> Color {
    srgb(atelier_color::cmyk_to_srgb(c, m, y, k))
}

fn srgb(c: [f32; 3]) -> Color {
    Color::rgb(c[0], c[1], c[2])
}

/// The ICC profile in an ICCBased stream, parsed once per document.
fn icc_profile(doc: &Document, obj: &Object) -> Option<Arc<Profile>> {
    let parse = || -> Option<Arc<Profile>> {
        let Object::Stream(stream) = doc.resolve(obj).ok()? else {
            return None;
        };
        let data = doc.decode_stream(&stream).ok()?.data;
        Profile::parse(&data, Intent::RelativeColorimetric).map(Arc::new)
    };
    match obj {
        Object::Reference(r) => {
            let key = u64::from(r.num) << 16 | u64::from(r.generation);
            (*doc.memo("icc-profile", key, parse)).clone()
        }
        _ => parse(),
    }
}

/// The CMYK profile of the document's output intent (PDF/X and the like),
/// which DeviceCMYK colours are shown through.
fn output_intent_cmyk(doc: &Document) -> Option<Arc<Profile>> {
    let find = || -> Option<Arc<Profile>> {
        let catalog = doc.catalog().ok()?;
        let intents = doc.get_key(&catalog, b"OutputIntents").ok()?;
        intents.as_array()?.iter().find_map(|i| {
            let intent = doc.resolve(i).ok()?;
            let profile = icc_profile(doc, intent.as_dict()?.get(b"DestOutputProfile")?)?;
            (profile.space == ProfileSpace::Cmyk).then_some(profile)
        })
    };
    (*doc.memo("output-intent-cmyk", 0, find)).clone()
}

/// The space a device colour space name stands for: the resources'
/// DefaultGray, DefaultRGB or DefaultCMYK when given (8.6.5.6), and for
/// CMYK otherwise the output intent's profile.
pub fn device(doc: &Document, resources: &Dict, components: usize) -> ColorSpace {
    let (name, plain): (&[u8], ColorSpace) = match components {
        1 => (b"DefaultGray", ColorSpace::Gray),
        4 => (b"DefaultCMYK", ColorSpace::Cmyk),
        _ => (b"DefaultRGB", ColorSpace::Rgb),
    };
    let named = doc
        .get_key(resources, b"ColorSpace")
        .ok()
        .and_then(|d| d.as_dict().and_then(|d| d.get(name)).cloned());
    if let Some(entry) = named {
        if let Some(cs) = parse_depth(doc, &entry, resources, 1, false) {
            if cs.components() == components && !matches!(cs, ColorSpace::Pattern { .. }) {
                return cs;
            }
        }
    }
    if components == 4 {
        if let Some(p) = output_intent_cmyk(doc) {
            return ColorSpace::Icc(p);
        }
    }
    plain
}

impl ColorSpace {
    /// Whether converting a colour costs enough that images should sample
    /// the conversion on a grid.
    pub fn is_costly(&self) -> bool {
        match self {
            Self::Icc(_) | Self::CalRgb(_) | Self::Lab { .. } | Self::Cmyk => true,
            Self::Spot { tint: Some(_), .. } => true,
            Self::Indexed { .. } => false,
            _ => false,
        }
    }
}

fn white_point(d: &Dict) -> [f32; 3] {
    let arr = |key: &[u8], n: usize| -> Option<Vec<f32>> {
        let a = d.get(key)?.as_array()?;
        (a.len() >= n).then(|| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
    };
    arr(b"WhitePoint", 3)
        .filter(|w| w[1] > 0.0)
        .map_or(atelier_color::D65, |w| [w[0], w[1], w[2]])
}

/// Parses a colour space object, looking names up in `resources`.
pub fn parse(doc: &Document, obj: &Object, resources: &Dict) -> Option<ColorSpace> {
    parse_depth(doc, obj, resources, 0, true)
}

/// `defaults` is false while resolving a Default space itself, which must
/// not refer back to the device space it replaces.
fn parse_depth(
    doc: &Document,
    obj: &Object,
    resources: &Dict,
    depth: usize,
    defaults: bool,
) -> Option<ColorSpace> {
    if depth > 8 {
        return None;
    }
    let obj = doc.resolve(obj).ok()?;
    let device = |n: usize, plain: ColorSpace| {
        if defaults {
            device(doc, resources, n)
        } else {
            plain
        }
    };
    match &obj {
        Object::Name(n) => match n.as_bytes() {
            b"DeviceGray" | b"G" | b"CalGray" => Some(device(1, ColorSpace::Gray)),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(device(3, ColorSpace::Rgb)),
            b"DeviceCMYK" | b"CMYK" | b"CalCMYK" => Some(device(4, ColorSpace::Cmyk)),
            b"Pattern" => Some(ColorSpace::Pattern { base: None }),
            _ => {
                let named = doc.get_key(resources, b"ColorSpace").ok()?;
                let entry = named.as_dict()?.get(n.as_bytes())?.clone();
                parse_depth(doc, &entry, resources, depth + 1, defaults)
            }
        },
        Object::Array(items) => {
            let family = doc.resolve(items.first()?).ok()?;
            match family.as_name()?.as_bytes() {
                b"DeviceGray" | b"G" => Some(device(1, ColorSpace::Gray)),
                b"DeviceRGB" | b"RGB" => Some(device(3, ColorSpace::Rgb)),
                b"DeviceCMYK" | b"CMYK" => Some(device(4, ColorSpace::Cmyk)),
                b"CalGray" => {
                    let params = doc.resolve(items.get(1)?).ok()?;
                    let d = params.as_dict()?;
                    let gamma = d.get(b"Gamma").and_then(Object::as_f64).unwrap_or(1.0) as f32;
                    Some(ColorSpace::CalGray(CalGray {
                        white: white_point(d),
                        gamma: if gamma > 0.0 { gamma } else { 1.0 },
                    }))
                }
                b"CalRGB" => {
                    let params = doc.resolve(items.get(1)?).ok()?;
                    let d = params.as_dict()?;
                    let floats = |key: &[u8], n: usize| -> Option<Vec<f32>> {
                        let a = d.get(key)?.as_array()?;
                        (a.len() >= n)
                            .then(|| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
                    };
                    let gamma = floats(b"Gamma", 3).unwrap_or(vec![1.0; 3]);
                    let matrix = floats(b"Matrix", 9)
                        .unwrap_or(vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
                    Some(ColorSpace::CalRgb(Arc::new(CalRgb {
                        white: white_point(d),
                        gamma: [gamma[0], gamma[1], gamma[2]]
                            .map(|g| if g > 0.0 { g } else { 1.0 }),
                        matrix: std::array::from_fn(|i| matrix[i]),
                    })))
                }
                b"ICCBased" => {
                    let stream_obj = items.get(1)?;
                    let stream = doc.resolve(stream_obj).ok()?;
                    let dict = stream.as_dict()?;
                    let n = doc
                        .get_key(dict, b"N")
                        .ok()
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if let Some(profile) = icc_profile(doc, stream_obj) {
                        if profile.components() as i64 == n || n == 0 {
                            return Some(if profile.space == ProfileSpace::Lab {
                                ColorSpace::Lab {
                                    white: atelier_color::D50,
                                }
                            } else {
                                ColorSpace::Icc(profile)
                            });
                        }
                    }
                    if let Some(alt) = dict.get(b"Alternate") {
                        if let Some(cs) = parse_depth(doc, alt, resources, depth + 1, defaults) {
                            return Some(cs);
                        }
                    }
                    match n {
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
                    let base = parse_depth(doc, items.get(1)?, resources, depth + 1, defaults)?;
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
                        .and_then(|b| parse_depth(doc, b, resources, depth + 1, defaults))
                        .map(Box::new),
                }),
                b"Separation" | b"DeviceN" => {
                    let separation = family.as_name()?.as_bytes() == b"Separation";
                    let names = doc.resolve(items.get(1)?).ok()?;
                    let colorants: Vec<String> = if separation {
                        names
                            .as_name()
                            .map(|n| n.as_str_lossy().into_owned())
                            .into_iter()
                            .collect()
                    } else {
                        names
                            .as_array()
                            .unwrap_or(&[])
                            .iter()
                            .filter_map(|n| n.as_name().map(|n| n.as_str_lossy().into_owned()))
                            .collect()
                    };
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
                        .and_then(|a| parse_depth(doc, a, resources, depth + 1, defaults))
                        .unwrap_or(ColorSpace::Gray);
                    let tint = items
                        .get(3)
                        .and_then(|f| Function::parse(doc, f))
                        .filter(|f| f.inputs() >= components)
                        .map(Arc::new);
                    Some(ColorSpace::Spot {
                        components,
                        none,
                        names: colorants,
                        separation,
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
            Some(Color::rgb(35.0 / 255.0, 31.0 / 255.0, 32.0 / 255.0))
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
        assert_eq!(cs.to_rgb(&[1.0]), Some(cmyk_to_rgb(0.0, 0.5, 1.0, 0.0)));
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
