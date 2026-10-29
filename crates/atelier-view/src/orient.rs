//! Auto-orient: which way up a page should be shown, judged by which way
//! its text runs. Scans with an OCR text layer count too, since that text
//! runs the way the scan does.

use atelier_render::{PageText, RenderOptions, extract_text, render_page};
use atelier_syntax::{Document, Page};

/// Fewer glyphs than this say too little about a page.
const MIN_GLYPHS: usize = 12;
/// The leading direction must hold this share of the glyphs.
const MIN_SHARE: f64 = 0.6;

/// The /Rotate (clockwise degrees) that makes most of the page's text read
/// left to right, or None when the page has too little text to tell.
pub fn upright_rotation(text: &PageText) -> Option<u16> {
    // Glyphs whose baseline runs right, up, left and down in user space.
    let mut counts = [0usize; 4];
    for g in &text.glyphs {
        if g.text.trim().is_empty() {
            continue;
        }
        let (a, b) = g.baseline;
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        if dx == 0.0 && dy == 0.0 {
            continue;
        }
        let quarter = (dy.atan2(dx) / std::f64::consts::FRAC_PI_2).round() as i64;
        counts[quarter.rem_euclid(4) as usize] += 1;
    }
    let total: usize = counts.iter().sum();
    let (best, &n) = counts.iter().enumerate().max_by_key(|(_, n)| **n)?;
    if total < MIN_GLYPHS || (n as f64) < MIN_SHARE * total as f64 {
        return None;
    }
    // Text running up reads left to right once the page turns a quarter
    // clockwise, and so on round.
    Some(best as u16 * 90)
}

/// A page image's text lines must stand out this much more one way than
/// the other to tell which way they run.
const MIN_AXIS_LEAD: f64 = 1.5;
/// Letters rising above the line must outnumber those hanging below it
/// (or the other way round) by this much to tell up from down.
const MIN_UPRIGHT_LEAD: f64 = 1.5;
/// Rising and hanging letters needed to judge a page image.
const MIN_TELLING_LETTERS: usize = 20;
/// Text is looked for up to this many degrees off square, in steps of
/// `SKEW_STEP`.
const MAX_SKEW: f64 = 12.0;
const SKEW_STEP: f64 = 0.25;

/// A black and white page image: `true` is ink. Row 0 is the top.
struct Ink {
    width: usize,
    height: usize,
    data: Vec<bool>,
}

impl Ink {
    /// Ink found by comparing each pixel with the average brightness
    /// around it, so a photo's uneven light and shadows do not read as
    /// ink. `gray` is one byte per pixel, 0 black to 255 white.
    fn from_gray(gray: &[u8], width: usize, height: usize) -> Self {
        // Summed brightness of every rectangle from the top left.
        let stride = width + 1;
        let mut sum = vec![0u64; stride * (height + 1)];
        for y in 0..height {
            let mut row = 0u64;
            for x in 0..width {
                row += u64::from(gray[y * width + x]);
                sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
            }
        }
        let r = (width.max(height) / 60).max(4);
        let mut data = vec![false; width * height];
        for y in 0..height {
            let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(height));
            for x in 0..width {
                let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(width));
                let total = sum[y1 * stride + x1] + sum[y0 * stride + x0]
                    - sum[y0 * stride + x1]
                    - sum[y1 * stride + x0];
                let n = ((y1 - y0) * (x1 - x0)) as u64;
                let g = u64::from(gray[y * width + x]) * n;
                // Clearly darker than its surroundings: by 15%, and by a
                // margin that grain in a dark photo does not reach.
                data[y * width + x] = g * 100 < total * 85 && g + 40 * n < total;
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// The connected patches of ink (touching sideways or corner to
    /// corner), as boxes.
    fn blobs(&self) -> Vec<Blob> {
        let (w, h) = (self.width, self.height);
        let mut seen = vec![false; w * h];
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for start in 0..w * h {
            if !self.data[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            stack.push(start);
            let mut b = Blob {
                left: w,
                top: h,
                right: 0,
                bottom: 0,
            };
            while let Some(i) = stack.pop() {
                let (x, y) = (i % w, i / w);
                b.left = b.left.min(x);
                b.right = b.right.max(x);
                b.top = b.top.min(y);
                b.bottom = b.bottom.max(y);
                for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        let j = ny * w + nx;
                        if self.data[j] && !seen[j] {
                            seen[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
            out.push(b);
        }
        out
    }
}

/// A patch of ink's box, edges included, in pixels.
#[derive(Debug, Clone, Copy)]
struct Blob {
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
}

/// A letter-sized patch of ink, in coordinates where text lines run
/// across: its middle across, and its top and bottom.
#[derive(Debug, Clone, Copy)]
struct Mark {
    x: f64,
    top: f64,
    bottom: f64,
}

/// The patches that could be letters (or bits of words): not specks, not
/// pictures, rules or the page's edge.
fn letters(blobs: &[Blob], width: usize, height: usize) -> Vec<Blob> {
    let max_h = (width.max(height) / 25).max(4);
    blobs
        .iter()
        .copied()
        .filter(|b| {
            let (w, h) = (b.right - b.left + 1, b.bottom - b.top + 1);
            (2..=max_h).contains(&w.min(h).max(2)) && h >= 3 && h <= max_h && w <= max_h * 6
        })
        .collect()
}

/// The letters with the page turned a quarter clockwise `quarters` times
/// (0 or 1), in the new page's coordinates.
fn marks(letters: &[Blob], height: usize, quarters: u8) -> Vec<Mark> {
    letters
        .iter()
        .map(|b| {
            if quarters == 0 {
                Mark {
                    x: (b.left + b.right) as f64 / 2.0,
                    top: b.top as f64,
                    bottom: b.bottom as f64 + 1.0,
                }
            } else {
                // (x, y) moves to (height - y, x).
                Mark {
                    x: height as f64 - (b.top + b.bottom) as f64 / 2.0,
                    top: b.left as f64,
                    bottom: b.right as f64 + 1.0,
                }
            }
        })
        .collect()
}

/// Ink per row with lines sloping by `slope` (rise per pixel across),
/// straightened out; each letter counts its width over its height.
fn profile(marks: &[Mark], slope: f64, centre: f64) -> (Vec<f64>, f64) {
    let shift = |m: &Mark| (m.x - centre) * slope;
    let lowest = marks
        .iter()
        .map(|m| m.top - shift(m))
        .fold(f64::INFINITY, f64::min);
    let highest = marks
        .iter()
        .map(|m| m.bottom - shift(m))
        .fold(f64::NEG_INFINITY, f64::max);
    if !lowest.is_finite() || !highest.is_finite() {
        return (Vec::new(), 0.0);
    }
    let mut rows = vec![0.0; (highest - lowest).ceil() as usize + 2];
    for m in marks {
        let (t, b) = (m.top - shift(m) - lowest, m.bottom - shift(m) - lowest);
        for row in rows
            .iter_mut()
            .take(b.round() as usize)
            .skip(t.round() as usize)
        {
            *row += 1.0;
        }
    }
    (rows, lowest)
}

/// How sharply ink comes and goes along a profile: high across text
/// lines, where rows of letters alternate with blank gaps.
fn banding(profile: &[f64]) -> f64 {
    let total: f64 = profile.iter().map(|p| p * p).sum();
    if total == 0.0 {
        return 0.0;
    }
    let change: f64 = profile.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    change / total
}

/// The slope at which text lines show most clearly, and how clearly.
fn straightest(marks: &[Mark]) -> (f64, f64) {
    let centre = marks.iter().map(|m| m.x).sum::<f64>() / marks.len().max(1) as f64;
    let steps = (MAX_SKEW / SKEW_STEP) as i32;
    (-steps..=steps)
        .map(|i| {
            let slope = (f64::from(i) * SKEW_STEP).to_radians().tan();
            (banding(&profile(marks, slope, centre).0), slope)
        })
        .fold((0.0, 0.0), |best, c| if c.0 > best.0 { c } else { best })
}

/// Letters rising above their line's middle band (ascenders, capitals)
/// and hanging below it (descenders), counted over every line.
fn rising_and_hanging(marks: &[Mark], slope: f64) -> (usize, usize) {
    let centre = marks.iter().map(|m| m.x).sum::<f64>() / marks.len().max(1) as f64;
    let (rows, lowest) = profile(marks, slope, centre);
    let peak = rows.iter().copied().fold(0.0, f64::max);
    // Lines are the runs of rows with letters in them.
    let mut lines: Vec<(f64, f64)> = Vec::new();
    let mut y = 0;
    while y < rows.len() {
        if rows[y] <= peak * 0.05 {
            y += 1;
            continue;
        }
        let start = y;
        while y < rows.len() && rows[y] > peak * 0.05 {
            y += 1;
        }
        lines.push((start as f64 + lowest, y as f64 + lowest));
    }
    let mut members: Vec<Vec<(f64, f64)>> = vec![Vec::new(); lines.len()];
    for m in marks {
        let shift = (m.x - centre) * slope;
        let (t, b) = (m.top - shift, m.bottom - shift);
        let mid = (t + b) / 2.0;
        if let Some(i) = lines.iter().position(|&(a, z)| a <= mid && mid < z) {
            members[i].push((t, b));
        }
    }
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (mut rising, mut hanging) = (0, 0);
    for line in members.into_iter().filter(|l| l.len() >= 5) {
        // Most letters sit between the baseline and the x-height.
        let x_line = median(line.iter().map(|m| m.0).collect());
        let base = median(line.iter().map(|m| m.1).collect());
        let x_height = base - x_line;
        if x_height < 2.0 {
            continue;
        }
        let reach = x_height * 0.2;
        rising += line.iter().filter(|m| m.0 < x_line - reach).count();
        hanging += line.iter().filter(|m| m.1 > base + reach).count();
    }
    (rising, hanging)
}

/// Which way up a page image reads, from the shape of its text: the
/// clockwise turn (0, 90, 180 or 270) that makes it upright, or None when
/// it cannot tell. For scans and photos of pages with no text layer.
/// Written for scripts whose letters rise above the line more than they
/// hang below it, such as Latin, Greek and Cyrillic.
pub fn upright_from_image(gray: &[u8], width: usize, height: usize) -> Option<u16> {
    if width < 32 || height < 32 || gray.len() < width * height {
        return None;
    }
    let ink = Ink::from_gray(gray, width, height);
    let letters = letters(&ink.blobs(), width, height);
    let across = marks(&letters, height, 0);
    let down = marks(&letters, height, 1);
    let (a, a_slope) = straightest(&across);
    let (d, d_slope) = straightest(&down);
    let (base, marks, slope) = if a >= d * MIN_AXIS_LEAD {
        (0, across, a_slope)
    } else if d >= a * MIN_AXIS_LEAD {
        // Lines run up or down the page: judged with it turned clockwise.
        (90, down, d_slope)
    } else {
        return None;
    };
    let (rising, hanging) = rising_and_hanging(&marks, slope);
    if rising + hanging < MIN_TELLING_LETTERS {
        return None;
    }
    if rising as f64 >= hanging as f64 * MIN_UPRIGHT_LEAD {
        Some(base)
    } else if hanging as f64 >= rising as f64 * MIN_UPRIGHT_LEAD {
        Some(base + 180)
    } else {
        None
    }
}

/// A rendered page (premultiplied RGBA) as grey, for
/// [`upright_from_image`].
pub fn gray_of_rgba(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .map(|p| {
            let (r, g, b, a) = (
                u32::from(p[0]),
                u32::from(p[1]),
                u32::from(p[2]),
                u32::from(p[3]),
            );
            // Composite over white, then weigh the channels as the eye does.
            let w = 255 - a;
            ((r + w) * 299 + (g + w) * 587 + (b + w) * 114) as f64 / 1000.0
        })
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// What auto-orient went by for a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// The page's text: its fonts, or a scan's OCR layer.
    Text,
    /// The look of the page image, for scans with no text layer.
    Image,
    /// Words read from the page image with OCR.
    Ocr,
}

/// Page images are judged at about this resolution.
const JUDGE_DPI: f64 = 150.0;
/// And no bigger than this on their long side, in pixels.
const JUDGE_MAX_PX: f64 = 2200.0;

/// The /Rotate that shows a page upright and what that was judged from,
/// or None when neither its text nor its image says.
pub fn upright_page(doc: &Document, page: &Page) -> Option<(u16, Basis)> {
    if let Some(r) = extract_text(doc, page)
        .ok()
        .and_then(|t| upright_rotation(&t))
    {
        return Some((r, Basis::Text));
    }
    let long = page.crop_box.width().max(page.crop_box.height()).max(1.0);
    let options = RenderOptions {
        scale: (JUDGE_DPI / 72.0).min(JUDGE_MAX_PX / long),
        annotations: false,
        ..RenderOptions::default()
    };
    let r = render_page(doc, page, &options).ok()?;
    let (w, h) = (r.pixmap.width(), r.pixmap.height());
    let turn = upright_from_image(&gray_of_rgba(r.pixmap.data()), w, h)?;
    // The image was drawn with the page's own turn already applied.
    Some(((page.rotate + turn) % 360, Basis::Image))
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_raster::Point;
    use atelier_render::TextGlyph;

    fn page(dir: (f64, f64), n: usize) -> PageText {
        let glyphs = (0..n)
            .map(|i| {
                let a = Point::new(dir.0 * i as f64 * 6.0, dir.1 * i as f64 * 6.0);
                let b = Point::new(a.x + dir.0 * 5.0, a.y + dir.1 * 5.0);
                TextGlyph {
                    text: "a".into(),
                    quad: [a, b, b, a],
                    baseline: (a, b),
                    size: 10.0,
                    font: "F".into(),
                    invisible: false,
                }
            })
            .collect();
        PageText { glyphs }
    }

    #[test]
    fn turns_text_to_read_left_to_right() {
        assert_eq!(upright_rotation(&page((1.0, 0.0), 40)), Some(0));
        assert_eq!(upright_rotation(&page((0.0, 1.0), 40)), Some(90));
        assert_eq!(upright_rotation(&page((-1.0, 0.0), 40)), Some(180));
        assert_eq!(upright_rotation(&page((0.0, -1.0), 40)), Some(270));
    }

    #[test]
    fn too_little_or_mixed_text_says_nothing() {
        assert_eq!(upright_rotation(&page((0.0, 1.0), 5)), None);
        let mut mixed = page((1.0, 0.0), 20);
        mixed.glyphs.extend(page((0.0, 1.0), 20).glyphs);
        assert_eq!(upright_rotation(&mixed), None);
        assert_eq!(upright_rotation(&PageText::default()), None);
    }

    /// A grey page of fake text: lines of letter-sized blocks, some rising
    /// above the line and fewer hanging below, sloping by `skew` degrees
    /// and then turned `quarters` times clockwise.
    fn fake_page(skew: f64, quarters: usize) -> (Vec<u8>, usize, usize) {
        let (w, h) = (900usize, 1200usize);
        let mut img = vec![225u8; w * h];
        let slope = skew.to_radians().tan();
        let mut seed = 7u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) % 100
        };
        for line in 0..30 {
            let base = 120 + line * 32;
            let mut x = 80;
            while x < 800 {
                let kind = next();
                let (top, bottom) = match kind {
                    0..=69 => (base - 11, base),
                    70..=91 => (base - 17, base),
                    _ => (base - 11, base + 5),
                };
                for px in x..x + 7 {
                    let dy = ((px as f64 - 450.0) * slope).round() as isize;
                    for py in top..bottom {
                        let y = (py as isize + dy) as usize;
                        img[y * w + px] = 40;
                    }
                }
                // Uneven spacing, as letters have.
                x += 9 + next() as usize % 4 + if next() < 15 { 10 } else { 0 };
            }
        }
        let (mut img, mut w, mut h) = (img, w, h);
        for _ in 0..quarters {
            // A quarter clockwise: (x, y) moves to (h - 1 - y, x).
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[x * h + (h - 1 - y)] = img[y * w + x];
                }
            }
            img = out;
            (w, h) = (h, w);
        }
        (img, w, h)
    }

    #[test]
    fn finds_which_way_up_a_scan_reads() {
        for (quarters, skew) in [(0, 0.0), (1, 2.0), (2, -3.0), (3, 1.0)] {
            let (img, w, h) = fake_page(skew, quarters);
            // Turned `quarters` clockwise, it needs the rest of the way round.
            let want = ((4 - quarters) % 4 * 90) as u16;
            assert_eq!(
                upright_from_image(&img, w, h),
                Some(want),
                "turned {quarters} quarters"
            );
        }
    }

    #[test]
    fn pictures_and_blank_pages_say_nothing() {
        let (w, h) = (400, 500);
        assert_eq!(upright_from_image(&vec![255; w * h], w, h), None);
        // A smooth gradient with no letters in it.
        let ramp: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8).collect();
        assert_eq!(upright_from_image(&ramp, w, h), None);
    }
}
