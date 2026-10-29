//! Renders PDF pages to pixmaps with Atelier's own rasterizer.
//!
//! Covers paths, strokes, clipping, device and CIE colour, Form XObjects,
//! images (including JPEG) and text with embedded or substituted fonts.
//! Shadings, patterns, transparency groups and annotations come in later
//! phases.

mod colorspace;
mod font;
mod function;
mod image;
mod interp;
mod pattern;
mod probe;
mod shading;
mod text;

pub use colorspace::ColorSpace;
pub use font::TextFont;
pub use interp::{
    RenderOptions, Rendered, extract_text, page_transform, probe_color, render_page, render_region,
};
pub use probe::{Paint, PaintKind, Probed, SpotColor, spot_colors};
pub use text::{PageText, TextGlyph};
