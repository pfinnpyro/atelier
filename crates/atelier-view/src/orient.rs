//! Auto-orient: which way up a page should be shown, judged by which way
//! its text runs. Scans with an OCR text layer count too, since that text
//! runs the way the scan does.

use atelier_render::PageText;

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
}
