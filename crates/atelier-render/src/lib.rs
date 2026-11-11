//! Renders PDF pages to pixmaps with Atelier's own rasterizer.
//!
//! Covers paths, strokes, clipping, device and CIE colour, Form XObjects,
//! images, text with embedded or substituted fonts, shadings, patterns and
//! common transparency features. Unsupported or malformed content is
//! reported through render warnings where possible.

mod colorspace;
mod font;
mod function;
mod image;
mod interp;
mod lines;
mod pattern;
mod probe;
mod shading;
mod text;

pub use colorspace::ColorSpace;
pub use font::TextFont;
pub use interp::{
    RenderOptions, Rendered, extract_graphics, extract_text, page_transform, probe_color,
    render_page, render_region,
};
pub use lines::{PageGraphics, PageLine};
pub use probe::{Paint, PaintKind, Probed, SpotColor, spot_colors};
pub use text::{PageText, TextGlyph};
