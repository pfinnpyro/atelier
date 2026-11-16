//! What drawing one part of a document learns that the next part needs
//! too: its fonts, with their rasterized glyphs, and its decoded images.
//!
//! The viewer draws a page as many tiles, each a separate run over the
//! page's content and often on a different thread. Without this every
//! tile would parse the page's fonts and decode its images again. The
//! cache belongs to the [`Document`] and is dropped with it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use atelier_syntax::{Document, ObjRef};

use crate::font::PdfFont;
use crate::image::ImageInfo;

/// Decoded images kept per document, in bytes.
static IMAGE_BUDGET: AtomicUsize = AtomicUsize::new(256 << 20);

/// Sets how many bytes of decoded images each open document may keep
/// between draws. An image bigger than this is still kept while it is the
/// one being drawn.
pub fn set_image_cache_budget(bytes: usize) {
    IMAGE_BUDGET.store(bytes, Ordering::Relaxed);
}

/// Drops the document's decoded images and rasterized glyphs, keeping its
/// parsed fonts. For a document that will not be drawn for a while.
pub fn trim_cache(doc: &Document) {
    let cache = RenderCache::of(doc);
    lock(&cache.images).map.clear();
    for (font, _) in lock(&cache.fonts).values() {
        font.forget_glyphs();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// An image, keyed by its stream and, for a stencil mask, the colour it
/// was painted with.
pub(crate) type ImageKey = (ObjRef, Option<[u32; 4]>);

/// A decoded image (or that it cannot be decoded) with the warnings
/// decoding gave.
pub(crate) type Decoded = (Option<Arc<ImageInfo>>, Vec<String>);

#[derive(Default)]
struct Images {
    /// Each image and when it was last used. The cell is filled by the
    /// first thread to want the image; others wait for it.
    map: HashMap<ImageKey, (Arc<OnceLock<Decoded>>, u64)>,
    clock: u64,
}

/// A font with the warnings loading it gave.
type Loaded = (Arc<PdfFont>, Vec<String>);

#[derive(Default)]
pub(crate) struct RenderCache {
    fonts: Mutex<HashMap<ObjRef, Loaded>>,
    images: Mutex<Images>,
}

impl RenderCache {
    pub(crate) fn of(doc: &Document) -> Arc<RenderCache> {
        doc.memo("render-cache", 0, RenderCache::default)
    }

    /// The font object `r`, loaded once, with the warnings loading gave.
    pub(crate) fn font(
        &self,
        r: ObjRef,
        load: impl FnOnce(&mut dyn FnMut(String)) -> PdfFont,
    ) -> Loaded {
        if let Some(hit) = lock(&self.fonts).get(&r) {
            return hit.clone();
        }
        // Loaded outside the lock: two threads may both load a font the
        // first time, and the first to finish is kept.
        let mut warnings = Vec::new();
        let font = Arc::new(load(&mut |w| warnings.push(w)));
        lock(&self.fonts)
            .entry(r)
            .or_insert((font, warnings))
            .clone()
    }

    /// The image `key`, decoded once.
    pub(crate) fn image(&self, key: ImageKey, decode: impl FnOnce() -> Decoded) -> Decoded {
        let cell = {
            let mut images = lock(&self.images);
            images.clock += 1;
            let clock = images.clock;
            let entry = images.map.entry(key).or_default();
            entry.1 = clock;
            entry.0.clone()
        };
        let mut fresh = false;
        let decoded = cell
            .get_or_init(|| {
                fresh = true;
                decode()
            })
            .clone();
        if fresh {
            self.evict(key);
        }
        decoded
    }

    /// Drops the images used longest ago until the rest fit the budget,
    /// never `keep`.
    fn evict(&self, keep: ImageKey) {
        let budget = IMAGE_BUDGET.load(Ordering::Relaxed);
        let mut images = lock(&self.images);
        let size = |cell: &OnceLock<Decoded>| {
            cell.get()
                .and_then(|d| d.0.as_ref())
                .map_or(0, |i| i.image.data.len())
        };
        let mut total: usize = images.map.values().map(|(c, _)| size(c)).sum();
        if total <= budget {
            return;
        }
        let mut order: Vec<(u64, ImageKey, usize)> = images
            .map
            .iter()
            .filter(|(k, _)| **k != keep)
            .map(|(k, (c, used))| (*used, *k, size(c)))
            .collect();
        order.sort_unstable_by_key(|(used, _, _)| *used);
        for (_, key, bytes) in order {
            if total <= budget {
                break;
            }
            images.map.remove(&key);
            total -= bytes;
        }
    }
}
