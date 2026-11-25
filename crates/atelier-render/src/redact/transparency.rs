use std::collections::HashMap;

use atelier_raster::BlendMode;
use atelier_syntax::{Dict, Document, Name, Object, Page, Result};

use crate::colorspace::{self, ColorSpace};

use super::{State, number, unsupported};

pub(super) fn page_group(doc: &Document, page: &Page) -> Result<Option<Dict>> {
    let Some(group) = page.dict.get(b"Group") else {
        return Ok(None);
    };
    let group = doc.resolve_dict(group)?;
    let subtype = doc.get_key(&group, b"S")?;
    if subtype
        .as_name()
        .is_none_or(|name| name.as_bytes() != b"Transparency")
    {
        return Err(unsupported("Unsupported page group subtype"));
    }

    let mut clean = Dict::new();
    clean.insert("Type", name("Group"));
    clean.insert("S", name("Transparency"));
    for (key, label) in [(b"I".as_slice(), "I"), (b"K".as_slice(), "K")] {
        if let Some(value) = group.get(key) {
            let value = doc
                .resolve(value)?
                .as_bool()
                .ok_or_else(|| unsupported("Invalid transparency group flag"))?;
            clean.insert(label, Object::Bool(value));
        }
    }
    if let Some(space) = group.get(b"CS") {
        let resolved = doc.resolve(space)?;
        // Literal device spaces select the blender's channels independently
        // of the default spaces used for individual source paints.
        let label = match resolved.as_name().map(Name::as_bytes) {
            Some(b"DeviceGray") => "DeviceGray",
            Some(b"DeviceRGB") => "DeviceRGB",
            Some(b"DeviceCMYK") => "DeviceCMYK",
            _ => match colorspace::parse(doc, &resolved, &page.resources) {
                Some(ColorSpace::Gray) => "DeviceGray",
                Some(ColorSpace::Cmyk) => "DeviceCMYK",
                Some(
                    ColorSpace::Rgb
                    | ColorSpace::CalGray(_)
                    | ColorSpace::CalRgb(_)
                    | ColorSpace::Lab { .. },
                ) => "DeviceRGB",
                Some(ColorSpace::Icc(profile)) if matches!(profile.components(), 1 | 3 | 4) => {
                    "DeviceRGB"
                }
                _ => return Err(unsupported("Unsupported page blending color space")),
            },
        };
        clean.insert("CS", name(label));
    }
    Ok(Some(clean))
}

pub(super) fn user_unit(doc: &Document, page: &Page) -> Result<Option<f64>> {
    page.dict
        .get(b"UserUnit")
        .map(|unit| {
            let unit = number(&doc.resolve(unit)?)?;
            if unit > 0.0 && unit <= 75_000.0 {
                Ok(unit)
            } else {
                Err(unsupported("Invalid page user unit"))
            }
        })
        .transpose()
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Effects {
    fill_alpha: u64,
    stroke_alpha: u64,
    blend: Name,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            fill_alpha: 1.0_f64.to_bits(),
            stroke_alpha: 1.0_f64.to_bits(),
            blend: Name::from("Normal"),
        }
    }
}

impl Effects {
    pub fn apply(
        state: &mut State,
        doc: &Document,
        resources: &Dict,
        args: &[Object],
    ) -> Result<()> {
        let [Object::Name(key)] = args else {
            return Err(unsupported("Invalid graphics state operator"));
        };
        let entries = doc.get_key(resources, b"ExtGState")?;
        let entries = entries
            .as_dict()
            .ok_or_else(|| unsupported("Invalid graphics state resources"))?;
        let source = doc.get_key(entries, key.as_bytes())?;
        let source = source
            .as_dict()
            .ok_or_else(|| unsupported("Invalid graphics state dictionary"))?;
        for (key, value) in source.iter() {
            let value = doc.resolve(value)?;
            match key.as_bytes() {
                b"ca" => state.effects.fill_alpha = alpha(&value)?,
                b"CA" => state.effects.stroke_alpha = alpha(&value)?,
                b"BM" => state.effects.blend = blend(&value)?,
                b"LW" => state.width = number(&value)?.abs(),
                b"LC" | b"LJ" => {
                    let style = number(&value)?;
                    if ![0.0, 1.0, 2.0].contains(&style) {
                        return Err(unsupported("Invalid extended stroke style"));
                    }
                    if key.as_bytes() == b"LC" {
                        state.cap = style;
                    } else {
                        state.join = style;
                    }
                }
                b"ML" => state.miter = number(&value)?.max(1.0),
                // These values leave the default PDF behavior in place.
                b"Type" if value == name("ExtGState") => {}
                b"SMask" if value == name("None") => {}
                b"AIS" | b"OP" | b"op" | b"SA" if value == Object::Bool(false) => {}
                b"TK" if value == Object::Bool(true) => {}
                b"OPM" if value.as_i64() == Some(0) => {}
                _ => {
                    return Err(unsupported(format!(
                        "Unsupported graphics state entry /{}",
                        key.as_str_lossy()
                    )));
                }
            }
        }
        Ok(())
    }

    fn dictionary(&self) -> Dict {
        let mut dict = Dict::new();
        dict.insert("Type", name("ExtGState"));
        dict.insert("ca", Object::Real(f64::from_bits(self.fill_alpha)));
        dict.insert("CA", Object::Real(f64::from_bits(self.stroke_alpha)));
        dict.insert("BM", Object::Name(self.blend.clone()));
        dict
    }
}

#[derive(Default)]
pub(super) struct UsedEffects {
    ids: HashMap<Effects, usize>,
    resources: Dict,
}

impl UsedEffects {
    pub fn emit(&mut self, effect: &Effects, out: &mut String) {
        if *effect == Effects::default() {
            return;
        }
        let next = self.ids.len();
        let id = self.ids.entry(effect.clone()).or_insert_with(|| {
            self.resources.insert(
                Name::from(format!("G{next}").as_str()),
                Object::Dict(effect.dictionary()),
            );
            next
        });
        use std::fmt::Write;
        writeln!(out, "/G{id} gs").unwrap();
    }

    pub fn write(self, resources: &mut Dict) {
        if !self.resources.is_empty() {
            resources.insert("ExtGState", Object::Dict(self.resources));
        }
    }
}

fn name(value: &str) -> Object {
    Object::Name(Name::from(value))
}

fn alpha(value: &Object) -> Result<u64> {
    Ok(number(value)?.clamp(0.0, 1.0).to_bits())
}

fn blend(value: &Object) -> Result<Name> {
    let list = value
        .as_array()
        .unwrap_or_else(|| std::slice::from_ref(value));
    let name = list
        .iter()
        .filter_map(Object::as_name)
        .find(|n| BlendMode::from_name(n.as_bytes()).is_some())
        .ok_or_else(|| unsupported("Unknown transparency blend mode"))?;
    Ok(
        if BlendMode::from_name(name.as_bytes()) == Some(BlendMode::Normal) {
            Name::from("Normal")
        } else {
            name.clone()
        },
    )
}
