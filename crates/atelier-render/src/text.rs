//! Text found on a page, with where each glyph sits, for selection, search
//! and copying.

use atelier_raster::Point;

/// One shown glyph.
#[derive(Debug, Clone, PartialEq)]
pub struct TextGlyph {
    /// The text the glyph stands for; empty when the font gives no way to
    /// know.
    pub text: String,
    /// The glyph's box in default user space (PDF points, y up): bottom
    /// left, bottom right, top right, top left, along the baseline direction.
    pub quad: [Point; 4],
    /// Start and end of the glyph's advance along its baseline.
    pub baseline: (Point, Point),
    /// The font size in user space units.
    pub size: f64,
    pub font: String,
    /// Drawn with text render mode 3 (as OCR layers are), so not visible.
    pub invisible: bool,
}

/// The text of a page in content order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    pub glyphs: Vec<TextGlyph>,
}

impl PageText {
    /// The page's text as plain lines. Glyphs are kept in content order; a
    /// new line starts when the baseline moves off the current line, and a
    /// space is added at gaps wider than a fraction of the font size.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        let mut prev: Option<&TextGlyph> = None;
        for g in &self.glyphs {
            if g.text.is_empty() {
                continue;
            }
            if let Some(p) = prev {
                let dir = (p.baseline.1 - p.baseline.0).normalize();
                let dir = if dir.length() == 0.0 {
                    Point::new(1.0, 0.0)
                } else {
                    dir
                };
                let delta = g.baseline.0 - p.baseline.1;
                let along = delta.dot(dir);
                let across = delta.cross(dir).abs();
                let size = p.size.max(g.size).max(1e-6);
                if across > size * 0.5 || along < -size * 2.0 {
                    out.push('\n');
                } else if along > size * 0.15 && !out.ends_with(' ') && !g.text.starts_with(' ') {
                    out.push(' ');
                }
            }
            out.push_str(&g.text);
            // Glyphs without text do not count, so a gap is measured across them.
            if !g.text.is_empty() {
                prev = Some(g);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(text: &str, x: f64, y: f64, w: f64) -> TextGlyph {
        let p = |x, y| Point::new(x, y);
        TextGlyph {
            text: text.into(),
            quad: [
                p(x, y - 2.0),
                p(x + w, y - 2.0),
                p(x + w, y + 8.0),
                p(x, y + 8.0),
            ],
            baseline: (p(x, y), p(x + w, y)),
            size: 10.0,
            font: "F".into(),
            invisible: false,
        }
    }

    #[test]
    fn plain_text_adds_spaces_and_lines() {
        let t = PageText {
            glyphs: vec![
                glyph("a", 0.0, 100.0, 5.0),
                glyph("b", 5.0, 100.0, 5.0),
                // An unmapped glyph in the gap does not hide the gap.
                glyph("", 10.0, 100.0, 3.0),
                glyph("c", 13.0, 100.0, 5.0),
                glyph("d", 0.0, 88.0, 5.0),
            ],
        };
        assert_eq!(t.plain_text(), "ab c\nd");
    }
}
