use atelier_raster::Point;
use atelier_render::{PageText, TextGlyph};

/// A position between glyphs: before glyph `index` of `page`'s text, or at
/// the end when `index` equals the glyph count. Carets order by page, then
/// by content order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Caret {
    pub page: usize,
    pub index: usize,
}

/// Distance from a point to a glyph's box, zero inside it.
fn distance(q: &[Point; 4], p: Point) -> f64 {
    // The box is a parallelogram: measure in its own axes.
    let ex = q[1] - q[0];
    let ey = q[3] - q[0];
    let lx = ex.length();
    let ly = ey.length();
    if lx == 0.0 || ly == 0.0 {
        return (p - q[0]).length();
    }
    let d = p - q[0];
    let u = d.dot(ex) / lx;
    let v = d.dot(ey) / ly;
    let du = if u < 0.0 { -u } else { (u - lx).max(0.0) };
    let dv = if v < 0.0 { -v } else { (v - ly).max(0.0) };
    (du * du + dv * dv).sqrt()
}

/// The caret nearest a point in the page's user space: before the closest
/// glyph, or after it when the point is past the glyph's middle. Glyphs
/// with no text are skipped. Vertical distance counts for more than
/// horizontal, so a point beside a line picks that line.
pub fn hit_test(text: &PageText, page: usize, p: Point) -> Option<Caret> {
    let mut best: Option<(f64, usize)> = None;
    for (i, g) in text.glyphs.iter().enumerate() {
        if g.text.is_empty() {
            continue;
        }
        let d = weighted_distance(g, p);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    let (_, i) = best?;
    let g = &text.glyphs[i];
    let dir = g.baseline.1 - g.baseline.0;
    let mid = g.baseline.0.lerp(g.baseline.1, 0.5);
    let after = dir.length() > 0.0 && (p - mid).dot(dir) > 0.0;
    Some(Caret {
        page,
        index: i + usize::from(after),
    })
}

fn weighted_distance(g: &TextGlyph, p: Point) -> f64 {
    let q = &g.quad;
    let ex = q[1] - q[0];
    let ey = q[3] - q[0];
    if ex.length() == 0.0 || ey.length() == 0.0 {
        return distance(q, p);
    }
    let d = p - q[0];
    let lx = ex.length();
    let ly = ey.length();
    let u = d.dot(ex) / lx;
    let v = d.dot(ey) / ly;
    let du = if u < 0.0 { -u } else { (u - lx).max(0.0) };
    let dv = if v < 0.0 { -v } else { (v - ly).max(0.0) };
    (du * du + 16.0 * dv * dv).sqrt()
}

/// A text selection from an anchor (where the drag started) to a focus
/// (where it is now), possibly across pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Caret,
    pub focus: Caret,
}

impl Selection {
    pub fn start(&self) -> Caret {
        self.anchor.min(self.focus)
    }

    pub fn end(&self) -> Caret {
        self.anchor.max(self.focus)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }

    /// The selected glyph range on `page`, given its glyph count.
    pub fn range_on(&self, page: usize, glyph_count: usize) -> Option<std::ops::Range<usize>> {
        let (s, e) = (self.start(), self.end());
        if page < s.page || page > e.page {
            return None;
        }
        let a = if page == s.page { s.index } else { 0 };
        let b = if page == e.page { e.index } else { glyph_count };
        let b = b.min(glyph_count);
        (a < b).then_some(a..b)
    }

    /// The selected text. `text` gives a page's text when it is known;
    /// pages without it are skipped. Pages are joined by line breaks.
    pub fn text<'a>(&self, text: impl Fn(usize) -> Option<&'a PageText>) -> String {
        let mut out = String::new();
        for page in self.start().page..=self.end().page {
            let Some(t) = text(page) else { continue };
            let Some(r) = self.range_on(page, t.glyphs.len()) else {
                continue;
            };
            let (s, _) = t.plain_text_range(r.start, r.end);
            if s.is_empty() {
                continue;
            }
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&s);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(text: &str, x: f64, y: f64) -> TextGlyph {
        let p = Point::new;
        TextGlyph {
            text: text.into(),
            quad: [
                p(x, y - 2.0),
                p(x + 5.0, y - 2.0),
                p(x + 5.0, y + 8.0),
                p(x, y + 8.0),
            ],
            baseline: (p(x, y), p(x + 5.0, y)),
            size: 10.0,
            font: "F".into(),
            invisible: false,
        }
    }

    fn page() -> PageText {
        PageText {
            glyphs: vec![
                glyph("a", 0.0, 100.0),
                glyph("b", 5.0, 100.0),
                glyph("c", 10.0, 100.0),
                glyph("d", 0.0, 80.0),
                glyph("e", 5.0, 80.0),
            ],
        }
    }

    #[test]
    fn hits_glyph_halves() {
        let t = page();
        let c = |x, y| hit_test(&t, 0, Point::new(x, y)).unwrap().index;
        assert_eq!(c(6.0, 101.0), 1);
        assert_eq!(c(9.0, 101.0), 2);
        // Right of the line's end: after its last glyph.
        assert_eq!(c(40.0, 100.0), 3);
        // Left of the second line: before its first glyph.
        assert_eq!(c(-30.0, 82.0), 3);
        assert_eq!(c(1.0, 79.0), 3);
        assert!(hit_test(&PageText::default(), 0, Point::new(0.0, 0.0)).is_none());
    }

    #[test]
    fn selects_text_backwards_and_across_pages() {
        let t = page();
        let sel = Selection {
            anchor: Caret { page: 0, index: 4 },
            focus: Caret { page: 0, index: 1 },
        };
        assert_eq!(sel.range_on(0, 5), Some(1..4));
        assert_eq!(sel.text(|_| Some(&t)), "bc\nd");
        let across = Selection {
            anchor: Caret { page: 0, index: 3 },
            focus: Caret { page: 2, index: 2 },
        };
        assert_eq!(across.range_on(1, 5), Some(0..5));
        assert_eq!(across.range_on(3, 5), None);
        assert_eq!(across.text(|_| Some(&t)), "de\nabc\nde\nab");
        let empty = Selection {
            anchor: Caret { page: 0, index: 2 },
            focus: Caret { page: 0, index: 2 },
        };
        assert!(empty.is_empty() && empty.text(|_| Some(&t)).is_empty());
    }
}
