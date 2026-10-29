use std::collections::HashMap;

use crate::layout::{Layout, ViewRect};

/// Tile edge in device pixels.
pub const TILE_SIZE: usize = 1024;

/// One square of a page rendered at one scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey {
    pub page: usize,
    /// Device pixels per point, in thousandths, so equal scales compare equal.
    pub scale_milli: u32,
    pub tx: u32,
    pub ty: u32,
}

impl TileKey {
    pub fn scale(&self) -> f64 {
        f64::from(self.scale_milli) / 1000.0
    }

    /// The device-pixel rectangle `(x, y, w, h)` of the page this tile
    /// covers, clipped to a page of `page_px` device pixels.
    pub fn region(&self, page_px: (usize, usize)) -> (usize, usize, usize, usize) {
        let x = self.tx as usize * TILE_SIZE;
        let y = self.ty as usize * TILE_SIZE;
        let w = TILE_SIZE.min(page_px.0.saturating_sub(x)).max(1);
        let h = TILE_SIZE.min(page_px.1.saturating_sub(y)).max(1);
        (x, y, w, h)
    }

    /// Where the tile sits in page coordinates (logical pixels), given the
    /// display's device pixels per logical pixel.
    pub fn page_rect(&self, page_px: (usize, usize), pixels_per_point: f64) -> ViewRect {
        let (x, y, w, h) = self.region(page_px);
        let k = 1.0 / pixels_per_point;
        ViewRect::new(x as f64 * k, y as f64 * k, w as f64 * k, h as f64 * k)
    }
}

/// The render scale for a zoom on a display, rounded to the key's step.
pub fn scale_key(zoom: f64, pixels_per_point: f64) -> u32 {
    (zoom * pixels_per_point * 1000.0).round().clamp(1.0, 1e6) as u32
}

/// The page's size in device pixels at a key's scale, as the renderer
/// rounds it.
fn page_pixels(layout: &Layout, page: usize, scale: f64) -> (usize, usize) {
    let (w, h) = layout.geom(page).map(|g| g.size()).unwrap_or((0.0, 0.0));
    (
        (w * scale - 0.01).ceil().max(1.0) as usize,
        (h * scale - 0.01).ceil().max(1.0) as usize,
    )
}

/// The tiles needed to cover `view` (a rectangle in layout coordinates),
/// nearest the view's centre first, each with the page's size in device
/// pixels.
pub fn tiles_for_view(
    layout: &Layout,
    view: ViewRect,
    pixels_per_point: f64,
) -> Vec<(TileKey, (usize, usize))> {
    tiles_at_scale(layout, view, scale_key(layout.zoom(), pixels_per_point))
}

/// Device pixels per point, in thousandths, of the quick low-resolution
/// tiles shown while sharp ones render (36 dpi).
pub const PREVIEW_SCALE: u32 = 500;

/// Like [`tiles_for_view`], at a given scale (in thousandths) rather than
/// the layout's zoom; used for preview tiles.
pub fn tiles_at_scale(
    layout: &Layout,
    view: ViewRect,
    scale_milli: u32,
) -> Vec<(TileKey, (usize, usize))> {
    let scale_milli = scale_milli.max(1);
    let scale = f64::from(scale_milli) / 1000.0;
    // Logical pixels to device pixels at the key's (rounded) scale.
    let k = scale / layout.zoom();
    let mut out = Vec::new();
    for page in layout.visible_pages(view.y, view.bottom()) {
        let Some(rect) = layout.page_rect(page) else {
            continue;
        };
        let Some(vis) = rect.intersect(&view) else {
            continue;
        };
        let px = page_pixels(layout, page, scale);
        let tiles_x = px.0.div_ceil(TILE_SIZE);
        let tiles_y = px.1.div_ceil(TILE_SIZE);
        let t = TILE_SIZE as f64;
        let x0 = (((vis.x - rect.x) * k) / t).floor().max(0.0) as usize;
        let y0 = (((vis.y - rect.y) * k) / t).floor().max(0.0) as usize;
        let x1 = ((((vis.right() - rect.x) * k) / t).ceil() as usize).min(tiles_x);
        let y1 = ((((vis.bottom() - rect.y) * k) / t).ceil() as usize).min(tiles_y);
        for ty in y0..y1 {
            for tx in x0..x1 {
                let key = TileKey {
                    page,
                    scale_milli,
                    tx: tx as u32,
                    ty: ty as u32,
                };
                out.push((key, px));
            }
        }
    }
    let (cx, cy) = (view.x + view.w / 2.0, view.y + view.h / 2.0);
    let dist = |(key, px): &(TileKey, (usize, usize))| {
        let r = layout.page_rect(key.page).unwrap_or(view);
        let tr = key.page_rect(*px, k);
        let (x, y) = (r.x + tr.x + tr.w / 2.0, r.y + tr.y + tr.h / 2.0);
        (x - cx).powi(2) + (y - cy).powi(2)
    };
    out.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    out
}

struct Entry<T> {
    value: T,
    bytes: usize,
    used: u64,
}

/// Rendered tiles, dropping the least recently used past a memory budget.
pub struct TileCache<T> {
    entries: HashMap<TileKey, Entry<T>>,
    budget: usize,
    bytes: usize,
    clock: u64,
}

impl<T> TileCache<T> {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget: budget_bytes,
            bytes: 0,
            clock: 0,
        }
    }

    /// Starts a new frame; tiles touched in it are kept over older ones.
    pub fn next_frame(&mut self) {
        self.clock += 1;
    }

    pub fn contains(&self, key: &TileKey) -> bool {
        self.entries.contains_key(key)
    }

    /// The tile, marked as used this frame.
    pub fn get(&mut self, key: &TileKey) -> Option<&T> {
        let clock = self.clock;
        self.entries.get_mut(key).map(|e| {
            e.used = clock;
            &e.value
        })
    }

    pub fn insert(&mut self, key: TileKey, value: T, bytes: usize) {
        if let Some(old) = self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                used: self.clock,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.evict();
    }

    /// Drops the oldest tiles until the cache fits its budget, never one
    /// used in the current frame.
    fn evict(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut order: Vec<(u64, TileKey)> =
            self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable();
        for (used, key) in order {
            if self.bytes <= self.budget || used == self.clock {
                break;
            }
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.bytes;
            }
        }
    }

    /// Cached tiles of a page at any scale, for drawing something while the
    /// wanted scale renders.
    pub fn page_tiles(&self, page: usize) -> impl Iterator<Item = (&TileKey, &T)> {
        self.entries
            .iter()
            .filter(move |(k, _)| k.page == page)
            .map(|(k, e)| (k, &e.value))
    }

    /// Drops every tile of a page, for a page whose size or turn changed.
    pub fn remove_page(&mut self, page: usize) {
        let bytes = &mut self.bytes;
        self.entries.retain(|k, e| {
            let keep = k.page != page;
            if !keep {
                *bytes -= e.bytes;
            }
            keep
        });
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PageGeom;
    use atelier_syntax::Rect;

    fn layout(n: usize, zoom: f64) -> Layout {
        let g = PageGeom {
            crop: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 612.0,
                y1: 792.0,
            },
            rotate: 0,
        };
        Layout::new(vec![g; n], zoom)
    }

    #[test]
    fn covers_visible_area_only() {
        let l = layout(3, 2.0);
        let r0 = l.page_rect(0).unwrap();
        // A view over the top of page 0 at 2x: the page is 1224 x 1584
        // device pixels, so two columns and two rows of tiles.
        let view = ViewRect::new(0.0, 0.0, 2000.0, r0.y + 100.0);
        let tiles = tiles_for_view(&l, view, 1.0);
        let keys: Vec<_> = tiles.iter().map(|t| (t.0.page, t.0.tx, t.0.ty)).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&(0, 0, 0)) && keys.contains(&(0, 1, 0)));
        assert_eq!(tiles[0].1, (1224, 1584));
        let first = TileKey {
            tx: 0,
            ty: 0,
            ..tiles[0].0
        };
        assert_eq!(first.region((1224, 1584)), (0, 0, 1024, 1024));
        let edge = TileKey {
            tx: 1,
            ty: 1,
            ..tiles[0].0
        };
        assert_eq!(edge.region((1224, 1584)), (1024, 1024, 200, 560));
        // A high-density display renders more pixels for the same view.
        let hidpi = tiles_for_view(&l, view, 2.0);
        assert_eq!(hidpi[0].0.scale_milli, 4000);
        assert_eq!(hidpi[0].1, (2448, 3168));
        assert_eq!(hidpi.len(), 3);
        // Previews cover the same view in far fewer tiles.
        let preview = tiles_at_scale(&l, view, PREVIEW_SCALE);
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].1, (306, 396));
    }

    #[test]
    fn spans_pages() {
        let l = layout(3, 0.5);
        let (w, h) = l.size();
        let tiles = tiles_for_view(&l, ViewRect::new(0.0, 0.0, w, h), 1.0);
        let pages: Vec<_> = tiles.iter().map(|t| t.0.page).collect();
        assert_eq!(tiles.len(), 3);
        // The middle page is nearest the centre.
        assert_eq!(pages[0], 1);
    }

    #[test]
    fn cache_drops_least_recently_used() {
        let key = |tx| TileKey {
            page: 0,
            scale_milli: 1000,
            tx,
            ty: 0,
        };
        let mut c = TileCache::new(300);
        c.insert(key(0), "a", 100);
        c.next_frame();
        c.insert(key(1), "b", 100);
        c.next_frame();
        c.insert(key(2), "c", 100);
        c.next_frame();
        assert!(c.get(&key(0)).is_some());
        c.insert(key(3), "d", 100);
        assert_eq!(c.len(), 3);
        assert!(!c.contains(&key(1)));
        assert!(c.contains(&key(0)));
        assert_eq!(c.bytes(), 300);
        // Tiles used this frame stay even over budget.
        c.insert(key(4), "e", 1000);
        assert!(c.contains(&key(0)) && c.contains(&key(4)));
        assert_eq!(c.page_tiles(0).count(), c.len());
    }
}
