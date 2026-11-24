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
const QUICK_DPI: f64 = 120.0;

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
    read_at_resolution(engine, doc, page, OCR_DPI, None, false)
}

/// A faster pass for the interactive viewer. The previous page's orientation
/// only changes which direction is tried first. Weak results get the full pass.
pub fn read_page_with_hint(
    engine: &Engine,
    doc: &Document,
    page: &Page,
    previous: Option<u16>,
) -> Result<Option<PageReading>, String> {
    match read_at_resolution(engine, doc, page, QUICK_DPI, previous, true)? {
        Some(reading) => Ok(Some(reading)),
        None => read_at_resolution(engine, doc, page, OCR_DPI, None, false),
    }
}

/// Checks the direction of scanned text on a small raster for early display.
pub fn preview_orientation(doc: &Document, page: &Page) -> Result<Option<u16>, String> {
    let (gray, width, height, _) = scan_image(doc, page, 96.0)?;
    Ok(upright_from_image(&gray, width, height).map(|turn| (page.rotate + turn) % 360))
}

fn scan_image(
    doc: &Document,
    page: &Page,
    dpi: f64,
) -> Result<(Vec<u8>, usize, usize, f64), String> {
    let long = page.crop_box.width().max(page.crop_box.height()).max(1.0);
    let scale = (dpi / 72.0).min(OCR_MAX_PX / long);
    let options = RenderOptions {
        scale,
        annotations: false,
        ..RenderOptions::default()
    };
    let pixmap = render_page(doc, page, &options)
        .map_err(|e| e.to_string())?
        .pixmap;
    Ok((
        gray_of_rgba(pixmap.data()),
        pixmap.width(),
        pixmap.height(),
        scale,
    ))
}

fn read_at_resolution(
    engine: &Engine,
    doc: &Document,
    page: &Page,
    dpi: f64,
    previous: Option<u16>,
    quick: bool,
) -> Result<Option<PageReading>, String> {
    let (gray, w, h, scale) = scan_image(doc, page, dpi)?;
    let expected = previous.map(|rotation| (rotation % 360 + 360 - page.rotate) % 360);
    let reading = if quick {
        let mut candidate = expected
            .map(|turn| engine.read_likely_upright(&gray, w, h, turn))
            .transpose()?
            .flatten();
        if !candidate
            .as_ref()
            .is_some_and(|(_, letters)| *letters >= 96)
        {
            let axis = upright_from_image(&gray, w, h);
            if axis != expected || candidate.is_none() {
                candidate = None;
                if let Some(axis) = axis {
                    for turn in [axis, (axis + 180) % 360] {
                        if expected != Some(turn) {
                            candidate = engine
                                .read_likely_upright(&gray, w, h, turn)?
                                .filter(|(_, letters)| turn == axis || *letters >= 96);
                            if candidate.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        candidate.map(|(reading, _)| reading)
    } else {
        engine.read_any_way_up(&gray, w, h, upright_from_image(&gray, w, h))?
    };
    let Some(reading) = reading else {
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
