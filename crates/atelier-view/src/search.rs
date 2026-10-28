use atelier_render::PageText;

/// One match: glyphs `glyphs` of `page`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub page: usize,
    pub glyphs: std::ops::Range<usize>,
}

/// Folds case and treats any run of whitespace as one space.
fn fold(s: &str) -> Vec<char> {
    let mut out = Vec::new();
    for c in s.chars() {
        if c.is_whitespace() {
            out.push(' ');
        } else {
            out.push(c.to_lowercase().next().unwrap_or(c));
        }
    }
    out
}

/// Every place `query` occurs in a page's text, ignoring case and treating
/// line breaks as spaces. Matches do not overlap.
pub fn find_in_page(text: &PageText, page: usize, query: &str) -> Vec<SearchMatch> {
    let q = fold(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    let (s, map) = text.plain_text_range(0, text.glyphs.len());
    // Collapse whitespace runs in the page text too, keeping the map.
    let mut hay = Vec::new();
    let mut hay_map = Vec::new();
    for (c, m) in fold(&s).into_iter().zip(map) {
        if c == ' ' && hay.last() == Some(&' ') {
            continue;
        }
        hay.push(c);
        hay_map.push(m);
    }
    let q: Vec<char> = q
        .iter()
        .enumerate()
        .filter(|&(i, &c)| !(c == ' ' && i > 0 && q[i - 1] == ' '))
        .map(|(_, &c)| c)
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + q.len() <= hay.len() {
        if hay[i..i + q.len()] == q[..] {
            let glyphs: Vec<usize> = hay_map[i..i + q.len()].iter().flatten().copied().collect();
            if let (Some(&a), Some(&b)) = (glyphs.iter().min(), glyphs.iter().max()) {
                out.push(SearchMatch {
                    page,
                    glyphs: a..b + 1,
                });
            }
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_raster::Point;
    use atelier_render::TextGlyph;

    fn line(words: &[(&str, f64)]) -> PageText {
        let mut glyphs = Vec::new();
        for &(w, y) in words {
            let mut x = 0.0;
            for c in w.chars() {
                let p = Point::new;
                glyphs.push(TextGlyph {
                    text: c.to_string(),
                    quad: [p(x, y), p(x + 5.0, y), p(x + 5.0, y + 8.0), p(x, y + 8.0)],
                    baseline: (p(x, y), p(x + 5.0, y)),
                    size: 10.0,
                    font: "F".into(),
                    invisible: false,
                });
                x += if c == ' ' { 0.0 } else { 5.0 };
            }
        }
        PageText { glyphs }
    }

    #[test]
    fn finds_case_insensitive_across_lines() {
        let t = line(&[("Hello world", 100.0), ("WORLD peace", 80.0)]);
        let m = find_in_page(&t, 3, "world");
        assert_eq!(
            m,
            vec![
                SearchMatch {
                    page: 3,
                    glyphs: 6..11
                },
                SearchMatch {
                    page: 3,
                    glyphs: 11..16
                },
            ]
        );
        // A phrase broken over two lines still matches.
        let m = find_in_page(&t, 0, "world  world");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].glyphs, 6..16);
        assert!(find_in_page(&t, 0, "  ").is_empty());
        assert!(find_in_page(&t, 0, "nothing").is_empty());
    }
}
