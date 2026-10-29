//! OCR for a PDF page: draws it, reads it any way up, and turns what was
//! read into page text in the page's own coordinates.

use atelier_raster::Point;
use atelier_render::{PageText, RenderOptions, TextGlyph, page_transform, render_page};
use atelier_syntax::{Document, Page};
use atelier_view::{gray_of_rgba, upright_from_image};

use crate::{Engine, unturn_point};

/// Pages are read at about this resolution...
const OCR_DPI: f64 = 200.0;
/// ...and no bigger than this on their long side, in pixels.
const OCR_MAX_PX: f64 = 3000.0;

/// What OCR made of a page.
#[derive(Debug, Clone)]
pub struct PageReading {
    /// The /Rotate that shows the page upright.
    pub rotate: u16,
    /// The words found, as invisible text in the page's user space.
    pub text: PageText,
}

/// Reads a page with OCR. None when the page could not be drawn or has no
/// readable text any way up.
pub fn read_page(
    engine: &Engine,
    doc: &Document,
    page: &Page,
) -> Result<Option<PageReading>, String> {
    let long = page.crop_box.width().max(page.crop_box.height()).max(1.0);
    let scale = (OCR_DPI / 72.0).min(OCR_MAX_PX / long);
    let options = RenderOptions {
        scale,
        annotations: false,
        ..RenderOptions::default()
    };
    let r = render_page(doc, page, &options).map_err(|e| e.to_string())?;
    let (w, h) = (r.pixmap.width(), r.pixmap.height());
    let gray = gray_of_rgba(r.pixmap.data());
    drop(r);
    // The quick guess from the shape of the text goes first, since it is
    // usually right and then only one way up needs reading.
    let hint = upright_from_image(&gray, w, h);
    let Some(reading) = engine.read_any_way_up(&gray, w, h, hint)? else {
        return Ok(None);
    };
    // Device pixels back to user space.
    let (to_device, _, _) = page_transform(page, scale);
    let Some(to_user) = to_device.invert() else {
        return Ok(None);
    };
    let (fw, fh) = (w as f64, h as f64);
    let user = |x: f32, y: f32| {
        let (x, y) = unturn_point(f64::from(x), f64::from(y), fw, fh, reading.turn);
        to_user.apply(Point::new(x, y))
    };
    let mut glyphs = Vec::new();
    for line in &reading.lines {
        for ch in line {
            let [x0, y0, x1, y1] = ch.rect;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let quad = [user(x0, y1), user(x1, y1), user(x1, y0), user(x0, y0)];
            glyphs.push(TextGlyph {
                text: ch.c.to_string(),
                quad,
                baseline: (quad[0], quad[1]),
                size: f64::from(y1 - y0) / scale,
                font: "OCR".into(),
                invisible: true,
            });
        }
    }
    Ok(Some(PageReading {
        rotate: (page.rotate + reading.turn) % 360,
        text: PageText { glyphs },
    }))
}
