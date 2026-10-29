//! Colour picking: what painted a point of a page, and the spot colours a
//! file uses.

use std::collections::BTreeMap;

use atelier_raster::Color;
use atelier_syntax::{Dict, Document, ObjRef, Object, XrefEntry};

use crate::colorspace::{self, ColorSpace};

/// What kind of drawing painted a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaintKind {
    Fill,
    Stroke,
    Text,
    Image,
    Shading,
}

impl PaintKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fill => "Filled shape",
            Self::Stroke => "Line",
            Self::Text => "Text",
            Self::Image => "Image",
            Self::Shading => "Gradient",
        }
    }
}

/// The paint that last changed a picked point.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub kind: PaintKind,
    /// The colour space, as described by [`ColorSpace::describe`].
    pub space: String,
    /// The colour's components in that space, 0 to 1 (tints for spot
    /// colours). Empty for images and gradients, whose colour varies.
    pub components: Vec<f32>,
    /// Spot colour names it paints with.
    pub spots: Vec<String>,
    /// For Separation and DeviceN: the name of each component.
    pub colorants: Vec<String>,
    /// For a spot colour: its alternate space and the components there.
    pub alternate: Option<(String, Vec<f32>)>,
}

/// A picked point: its colour as drawn, and what painted it, if anything
/// did.
#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    /// Red, green and blue as shown, over the white page.
    pub rgb: [u8; 3],
    pub paint: Option<Paint>,
}

/// A spot colour the file uses.
#[derive(Debug, Clone, PartialEq)]
pub struct SpotColor {
    pub name: String,
    /// The space that defines it: "Separation … (alternate DeviceCMYK)".
    pub space: String,
    /// How it looks at full tint.
    pub rgb: Option<Color>,
}

/// Objects searched at most when listing spot colours.
const MAX_OBJECTS: usize = 200_000;
/// How deep into an object spot colour spaces are looked for.
const MAX_DEPTH: usize = 6;

/// Every Separation and DeviceN colorant named anywhere in the file,
/// sorted by name. Each is listed once, with the first space found for it.
pub fn spot_colors(doc: &Document) -> Vec<SpotColor> {
    let mut found: BTreeMap<String, SpotColor> = BTreeMap::new();
    let empty = Dict::default();
    for num in doc.object_numbers().into_iter().take(MAX_OBJECTS) {
        let generation = match doc.xref_entry(num) {
            Some(XrefEntry::Offset { generation, .. }) => generation,
            _ => 0,
        };
        let Ok(obj) = doc.get(ObjRef { num, generation }) else {
            continue;
        };
        let obj = match obj {
            Object::Stream(s) => Object::Dict(s.dict),
            o => o,
        };
        walk(doc, &obj, &empty, 0, &mut found);
    }
    found.into_values().collect()
}

fn walk(
    doc: &Document,
    obj: &Object,
    resources: &Dict,
    depth: usize,
    found: &mut BTreeMap<String, SpotColor>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match obj {
        Object::Array(items) => {
            let family = items.first().and_then(Object::as_name);
            if family.is_some_and(|f| f.as_bytes() == b"Separation" || f.as_bytes() == b"DeviceN") {
                if let Some(space) = colorspace::parse(doc, obj, resources) {
                    add(&space, found);
                }
                return;
            }
            for item in items {
                walk(doc, item, resources, depth + 1, found);
            }
        }
        Object::Dict(d) => {
            for (_, v) in d.iter() {
                walk(doc, v, resources, depth + 1, found);
            }
        }
        _ => {}
    }
}

fn add(space: &ColorSpace, found: &mut BTreeMap<String, SpotColor>) {
    let spots = space.spot_names();
    let n = space.components();
    for (i, name) in space.colorants().iter().enumerate() {
        if !spots.contains(name) || found.contains_key(name) {
            continue;
        }
        // Full tint of this colorant alone.
        let mut tints = vec![0.0; n];
        if let Some(t) = tints.get_mut(i) {
            *t = 1.0;
        }
        found.insert(
            name.clone(),
            SpotColor {
                name: name.clone(),
                space: space.describe(),
                rgb: space.to_rgb(&tints),
            },
        );
    }
}
