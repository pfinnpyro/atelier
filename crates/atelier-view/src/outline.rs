//! A document's outline for the sidebar: its bookmarks, or for files
//! without any, headings found by their size in the text.

use std::collections::HashMap;

use atelier_render::{PageText, TextGlyph};
use atelier_syntax::{Dict, Document, ObjRef, Object};

/// One entry of the outline.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    /// Nesting, from 0 at the top.
    pub depth: usize,
    /// The page it goes to, when known.
    pub page: Option<usize>,
    /// The height on the page (user space) it goes to, when given.
    pub top: Option<f64>,
}

/// Bookmarks at most, against huge or looping outlines.
const MAX_ITEMS: usize = 20_000;
const MAX_DEPTH: usize = 32;

/// The file's bookmarks (/Outlines), in order.
pub fn bookmarks(doc: &Document) -> Vec<OutlineItem> {
    let mut out = Vec::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    let Ok(Object::Dict(root)) = doc.get_key(&catalog, b"Outlines") else {
        return out;
    };
    let pages: HashMap<ObjRef, usize> = doc
        .pages()
        .map(|p| {
            p.iter()
                .enumerate()
                .filter_map(|(i, p)| p.obj_ref.map(|r| (r, i)))
                .collect()
        })
        .unwrap_or_default();
    let names = named_destinations(doc, &catalog);
    let mut seen = std::collections::HashSet::new();
    let mut next = root.get(b"First").and_then(Object::as_reference);
    walk(doc, &mut next, 0, &pages, &names, &mut seen, &mut out);
    out
}

fn walk(
    doc: &Document,
    next: &mut Option<ObjRef>,
    depth: usize,
    pages: &HashMap<ObjRef, usize>,
    names: &HashMap<String, Object>,
    seen: &mut std::collections::HashSet<ObjRef>,
    out: &mut Vec<OutlineItem>,
) {
    while let Some(r) = next.take() {
        if depth > MAX_DEPTH || out.len() >= MAX_ITEMS || !seen.insert(r) {
            return;
        }
        let Ok(Object::Dict(item)) = doc.get(r) else {
            return;
        };
        let title = match doc.get_key(&item, b"Title") {
            Ok(Object::String(s)) => s.to_text().trim().to_string(),
            _ => String::new(),
        };
        let dest = destination(doc, &item, names);
        let (page, top) = dest.map_or((None, None), |d| target(doc, &d, pages));
        out.push(OutlineItem {
            title,
            depth,
            page,
            top,
        });
        let mut child = item.get(b"First").and_then(Object::as_reference);
        walk(doc, &mut child, depth + 1, pages, names, seen, out);
        *next = item.get(b"Next").and_then(Object::as_reference);
    }
}

/// An outline item's destination: its /Dest, or a GoTo action's /D,
/// with named destinations looked up.
fn destination(doc: &Document, item: &Dict, names: &HashMap<String, Object>) -> Option<Object> {
    let dest = match doc.get_key(item, b"Dest") {
        Ok(d) if !d.is_null() => d,
        _ => {
            let Ok(Object::Dict(action)) = doc.get_key(item, b"A") else {
                return None;
            };
            if action
                .get(b"S")
                .and_then(Object::as_name)
                .map(|n| n.as_bytes())
                != Some(b"GoTo")
            {
                return None;
            }
            doc.get_key(&action, b"D").ok()?
        }
    };
    let named = match &dest {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(n.as_str_lossy().into_owned()),
        _ => None,
    };
    match named {
        Some(n) => {
            let d = doc.resolve(names.get(&n)?).ok()?;
            // A named destination may be wrapped in a dictionary as /D.
            match d {
                Object::Dict(d) => doc.get_key(&d, b"D").ok(),
                other => Some(other),
            }
        }
        None => Some(dest),
    }
}

/// The page and height a destination array points at.
fn target(
    doc: &Document,
    dest: &Object,
    pages: &HashMap<ObjRef, usize>,
) -> (Option<usize>, Option<f64>) {
    let Some(a) = dest.as_array() else {
        return (None, None);
    };
    let page = match a.first() {
        Some(Object::Reference(r)) => pages.get(r).copied(),
        // Some files give a page number instead of a page.
        Some(Object::Integer(n)) => usize::try_from(*n).ok(),
        _ => None,
    };
    let kind = a
        .get(1)
        .and_then(Object::as_name)
        .map(|n| n.as_bytes().to_vec());
    let top = match kind.as_deref() {
        Some(b"XYZ") => a.get(3),
        Some(b"FitH" | b"FitBH") => a.get(2),
        Some(b"FitR") => a.get(5),
        _ => None,
    }
    .and_then(|o| doc.resolve(o).ok())
    .and_then(|o| o.as_f64());
    (page, top)
}

/// Named destinations: the catalog's /Dests dictionary and the /Dests
/// name tree.
fn named_destinations(doc: &Document, catalog: &Dict) -> HashMap<String, Object> {
    let mut out = HashMap::new();
    if let Ok(Object::Dict(d)) = doc.get_key(catalog, b"Dests") {
        for (k, v) in d.iter() {
            out.insert(k.as_str_lossy().into_owned(), v.clone());
        }
    }
    if let Ok(Object::Dict(names)) = doc.get_key(catalog, b"Names")
        && let Ok(Object::Dict(tree)) = doc.get_key(&names, b"Dests")
    {
        name_tree(doc, &tree, &mut out, 0);
    }
    out
}

fn name_tree(doc: &Document, node: &Dict, out: &mut HashMap<String, Object>, depth: usize) {
    if depth > MAX_DEPTH || out.len() >= MAX_ITEMS * 4 {
        return;
    }
    if let Ok(Object::Array(names)) = doc.get_key(node, b"Names") {
        for pair in names.chunks_exact(2) {
            if let Ok(Object::String(s)) = doc.resolve(&pair[0]) {
                out.insert(s.to_text(), pair[1].clone());
            }
        }
    }
    if let Ok(Object::Array(kids)) = doc.get_key(node, b"Kids") {
        for k in kids {
            if let Ok(Object::Dict(kid)) = doc.resolve(&k) {
                name_tree(doc, &kid, out, depth + 1);
            }
        }
    }
}

/// A line of text on a page, for finding headings.
#[derive(Debug, Clone)]
struct Line {
    text: String,
    size: f64,
    /// The top of the line, in user space.
    top: f64,
    bold: bool,
}

/// Splits a page's text into lines: a new line starts where the next
/// glyph is off the current baseline or goes back.
fn lines(text: &PageText) -> Vec<Line> {
    let mut out = Vec::new();
    let glyphs = &text.glyphs;
    let mut start = 0;
    let flush = |from: usize, to: usize, out: &mut Vec<Line>| {
        let gs: Vec<&TextGlyph> = glyphs[from..to]
            .iter()
            .filter(|g| !g.text.trim().is_empty() && !g.invisible)
            .collect();
        if gs.is_empty() {
            return;
        }
        let mut sizes: Vec<f64> = gs.iter().map(|g| g.size).collect();
        sizes.sort_by(f64::total_cmp);
        let size = sizes[sizes.len() / 2];
        let top = gs
            .iter()
            .flat_map(|g| g.quad.iter().map(|p| p.y))
            .fold(f64::NEG_INFINITY, f64::max);
        let bold = gs.iter().filter(|g| is_bold(&g.font)).count() * 2 > gs.len();
        let text = text.plain_text_range(from, to).0;
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push(Line {
            text,
            size,
            top,
            bold,
        });
    };
    let mut prev: Option<&TextGlyph> = None;
    for (i, g) in glyphs.iter().enumerate() {
        if g.text.is_empty() {
            continue;
        }
        if let Some(p) = prev {
            let dir = (p.baseline.1 - p.baseline.0).normalize();
            let delta = g.baseline.0 - p.baseline.1;
            let size = p.size.max(g.size).max(1e-6);
            let across = if dir.length() == 0.0 {
                delta.length()
            } else {
                delta.cross(dir).abs()
            };
            let along = delta.dot(dir);
            if across > size * 0.5 || along < -size || along > size * 6.0 {
                flush(start, i, &mut out);
                start = i;
            }
        }
        prev = Some(g);
    }
    flush(start, glyphs.len(), &mut out);
    out
}

fn is_bold(font: &str) -> bool {
    let f = font.to_ascii_lowercase();
    f.contains("bold") || f.contains("black") || f.contains("heavy") || f.contains("semibold")
}

/// Headings found in the text, for files without bookmarks: lines set
/// clearly larger than the body text (or bold at a size just above it),
/// nested by size. `pages` is each page's text, in order.
pub fn headings(pages: &[PageText]) -> Vec<OutlineItem> {
    let all: Vec<(usize, Line)> = pages
        .iter()
        .enumerate()
        .flat_map(|(p, t)| lines(t).into_iter().map(move |l| (p, l)))
        .collect();
    // The body size: the size most characters are set in.
    let mut weight: HashMap<i64, usize> = HashMap::new();
    for (_, l) in &all {
        *weight.entry((l.size * 2.0).round() as i64).or_default() += l.text.chars().count();
    }
    let Some(body) = weight
        .iter()
        .max_by_key(|(size, n)| (**n, -**size))
        .map(|(s, _)| *s as f64 / 2.0)
    else {
        return Vec::new();
    };
    let is_heading = |l: &Line| {
        let words = l.text.split_whitespace().count();
        let letters = l.text.chars().any(char::is_alphabetic);
        let big = l.size >= body * 1.15 || (l.bold && l.size >= body * 1.05 && words <= 12);
        big && letters && (1..=20).contains(&words) && l.text.chars().count() <= 140
    };
    let mut found: Vec<(usize, Line)> = all.into_iter().filter(|(_, l)| is_heading(l)).collect();
    // A heading set over several lines is one heading.
    let mut merged: Vec<(usize, Line)> = Vec::new();
    for (p, l) in found.drain(..) {
        if let Some((mp, m)) = merged.last_mut()
            && *mp == p
            && (m.size - l.size).abs() < 0.25
            && (m.top - l.top).abs() < l.size * 1.6
            && m.text.chars().count() < 140
        {
            m.text.push(' ');
            m.text.push_str(&l.text);
            continue;
        }
        merged.push((p, l));
    }
    // Too many "headings" means the sizes say nothing about structure.
    let total_lines: usize = pages.iter().map(|t| lines(t).len()).sum();
    if merged.is_empty() || merged.len() * 4 > total_lines.max(8) {
        return Vec::new();
    }
    // Levels: the distinct heading sizes, largest first, at most three.
    let mut sizes: Vec<f64> = merged.iter().map(|(_, l)| l.size).collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    sizes.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    merged
        .into_iter()
        .map(|(page, l)| {
            let depth = sizes
                .iter()
                .position(|s| (s - l.size).abs() < 0.5)
                .unwrap_or(0)
                .min(2);
            OutlineItem {
                title: l.text,
                depth,
                page: Some(page),
                top: Some(l.top),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_raster::Point;

    fn line(text: &str, x: f64, y: f64, size: f64, font: &str) -> Vec<TextGlyph> {
        text.chars()
            .enumerate()
            .map(|(i, c)| {
                let x0 = x + i as f64 * size * 0.5;
                let a = Point::new(x0, y);
                let b = Point::new(x0 + size * 0.5, y);
                TextGlyph {
                    text: c.to_string(),
                    quad: [a, b, Point::new(b.x, y + size), Point::new(a.x, y + size)],
                    baseline: (a, b),
                    size,
                    font: font.into(),
                    invisible: false,
                }
            })
            .collect()
    }

    #[test]
    fn finds_headings_by_size() {
        let mut page = PageText::default();
        page.glyphs
            .extend(line("Introduction", 50.0, 700.0, 18.0, "Helvetica-Bold"));
        for i in 0..10 {
            page.glyphs.extend(line(
                "body text that goes on and on",
                50.0,
                670.0 - i as f64 * 14.0,
                10.0,
                "Times",
            ));
        }
        page.glyphs
            .extend(line("Background", 50.0, 500.0, 14.0, "Helvetica"));
        for i in 0..10 {
            page.glyphs.extend(line(
                "more body text here as well",
                50.0,
                480.0 - i as f64 * 14.0,
                10.0,
                "Times",
            ));
        }
        let h = headings(&[page]);
        let titles: Vec<(&str, usize)> = h.iter().map(|i| (i.title.as_str(), i.depth)).collect();
        assert_eq!(titles, [("Introduction", 0), ("Background", 1)]);
        assert_eq!(h[0].page, Some(0));
        assert_eq!(h[0].top, Some(718.0));
    }

    #[test]
    fn reads_bookmarks() {
        let pdf = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /Outlines 5 0 R /Dests << /end [4 0 R /Fit] >> >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
5 0 obj << /Type /Outlines /First 6 0 R /Last 7 0 R >> endobj
6 0 obj << /Title (One) /Parent 5 0 R /Next 7 0 R /First 8 0 R /Dest [3 0 R /XYZ 0 150 0] >> endobj
7 0 obj << /Title (Two) /Parent 5 0 R /Prev 6 0 R /A << /S /GoTo /D /end >> >> endobj
8 0 obj << /Title (One point one) /Parent 6 0 R /Dest [4 0 R /FitH 90] >> endobj
trailer << /Root 1 0 R >>
%%EOF
";
        let doc = Document::open(&pdf[..]).unwrap();
        let b = bookmarks(&doc);
        let got: Vec<_> = b
            .iter()
            .map(|i| (i.title.as_str(), i.depth, i.page, i.top))
            .collect();
        assert_eq!(
            got,
            [
                ("One", 0, Some(0), Some(150.0)),
                ("One point one", 1, Some(1), Some(90.0)),
                ("Two", 0, Some(1), None),
            ]
        );
    }
}
