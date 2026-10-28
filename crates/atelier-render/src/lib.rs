//! Renders PDF pages to pixmaps with Atelier's own rasterizer.
//!
//! This phase covers paths, strokes, clipping, device and CIE colour, Form
//! XObjects and images without image codecs. Text, shadings, patterns,
//! transparency groups and annotations come in later phases.

mod colorspace;
mod image;
mod interp;

pub use colorspace::ColorSpace;
pub use interp::{RenderOptions, Rendered, page_transform, render_page};
