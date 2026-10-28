//! What the viewer shows and where, kept apart from the window so it can be
//! tested: where each page sits at a zoom level, which tiles cover the
//! visible area, a tile cache, text selection and search.
//!
//! Layout coordinates are logical pixels (the window's points) with the
//! origin at the top left of the whole document. Page coordinates are the
//! same units with the origin at the top left of one page. User space is
//! the PDF's own coordinates (points, y up).

mod layout;
mod search;
mod select;
mod tiles;

pub use layout::{Layout, PageGeom, ViewRect};
pub use search::{SearchMatch, find_in_page};
pub use select::{Caret, Selection, hit_test};
pub use tiles::{
    PREVIEW_SCALE, TILE_SIZE, TileCache, TileKey, scale_key, tiles_at_scale, tiles_for_view,
};
