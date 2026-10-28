//! Renders PDF pages to pixmaps with Atelier's own rasterizer.
//!
//! Covers paths, strokes, clipping, device and CIE colour, Form XObjects,
//! images (including JPEG) and text with embedded or substituted fonts.
//! Shadings, patterns, transparency groups and annotations come in later
//! phases.

mod colorspace;
mod font;
mod image;
mod interp;
mod text;

pub use colorspace::ColorSpace;
pub use interp::{
    RenderOptions, Rendered, extract_text, page_transform, render_page, render_region,
};
pub use text::{PageText, TextGlyph};
