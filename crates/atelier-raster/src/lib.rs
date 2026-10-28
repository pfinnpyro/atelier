//! Atelier's CPU rasterizer.
//!
//! Paths are flattened to line segments and scan converted with exact-area
//! anti-aliasing: each pixel's coverage is the area of the pixel inside the
//! shape, accumulated per scanline and resolved with a prefix sum. Strokes are
//! turned into polygons and filled. Clipping uses coverage masks, and all
//! drawing composites premultiplied RGBA.

mod geom;
mod image;
mod path;
mod pixmap;
mod raster;
mod stroke;

pub use geom::{Point, Transform};
pub use image::{ImageSampling, RgbaImage, draw_image};
pub use path::{Path, PathBuilder, Verb};
pub use pixmap::{Color, Mask, Pixmap};
pub use raster::{Coverage, FillRule, rasterize};
pub use stroke::{LineCap, LineJoin, StrokeStyle, stroke_to_path};

/// Fills `path` (already in device space) with a solid colour.
pub fn fill_path(
    pixmap: &mut Pixmap,
    path: &Path,
    rule: FillRule,
    color: Color,
    clip: Option<&Mask>,
) {
    let bounds = pixmap.bounds();
    let Some(coverage) = rasterize(path, rule, bounds) else {
        return;
    };
    pixmap.fill_coverage(&coverage, color, clip);
}
