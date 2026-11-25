use atelier_raster::Color;
use atelier_syntax::{Dict, Document, Object, Result};

use crate::colorspace::{self, ColorSpace};

use super::{line, number, unsupported};

/// Resolve only the paint that is actually applied. The destination contains
/// no source color-space names, lookup tables, or ICC profile metadata.
#[derive(Clone)]
pub(super) struct Paint {
    space: Arc<ColorSpace>,
    pub command: String,
    pub rgb: Color,
}

impl Paint {
    pub fn initial(space: ColorSpace, stroke: bool) -> Result<Self> {
        let values: Vec<_> = space.initial().into_iter().map(f64::from).collect();
        Self::new(Arc::new(space), &values, stroke)
    }

    fn new(space: Arc<ColorSpace>, values: &[f64], stroke: bool) -> Result<Self> {
        if matches!(&*space, ColorSpace::Spot { tint: None, .. }) {
            return Err(unsupported("Color space has no usable tint transform"));
        }
        let samples: Vec<_> = values.iter().map(|&n| n as f32).collect();
        let rgb = space
            .to_rgb(&samples)
            .filter(|c| [c.r, c.g, c.b].into_iter().all(f32::is_finite))
            .ok_or_else(|| unsupported("Unsupported non-solid color"))?;
        let command = match (&*space, stroke) {
            (ColorSpace::Gray, false) => line(b"g", values),
            (ColorSpace::Gray, true) => line(b"G", values),
            (ColorSpace::Rgb, false) => line(b"rg", values),
            (ColorSpace::Rgb, true) => line(b"RG", values),
            (ColorSpace::Cmyk, false) => line(b"k", values),
            (ColorSpace::Cmyk, true) => line(b"K", values),
            _ => line(
                if stroke { b"RG" } else { b"rg" },
                &[rgb.r, rgb.g, rgb.b].map(f64::from),
            ),
        };
        Ok(Self {
            space,
            command,
            rgb,
        })
    }

    pub fn device(doc: &Document, resources: &Dict, op: &[u8], args: &[Object]) -> Result<Self> {
        let (components, stroke) = match op {
            b"g" => (1, false),
            b"G" => (1, true),
            b"rg" => (3, false),
            b"RG" => (3, true),
            b"k" => (4, false),
            b"K" => (4, true),
            _ => return Err(unsupported("Invalid device color operator")),
        };
        Self::from_operands(
            Arc::new(colorspace::device(doc, resources, components)),
            args,
            stroke,
        )
    }

    pub fn named(doc: &Document, resources: &Dict, args: &[Object], stroke: bool) -> Result<Self> {
        let [Object::Name(_)] = args else {
            return Err(unsupported("Invalid color-space operator"));
        };
        let space = colorspace::parse(doc, &args[0], resources)
            .ok_or_else(|| unsupported("Unresolved page color space"))?;
        Self::initial(space, stroke)
    }

    pub fn operands(&self, args: &[Object], stroke: bool) -> Result<Self> {
        Self::from_operands(self.space.clone(), args, stroke)
    }

    fn from_operands(space: Arc<ColorSpace>, args: &[Object], stroke: bool) -> Result<Self> {
        if args.len() != space.components() {
            return Err(unsupported("Wrong number of color components"));
        }
        let values = args.iter().map(number).collect::<Result<Vec<_>>>()?;
        Self::new(space, &values, stroke)
    }
}
use std::sync::Arc;
