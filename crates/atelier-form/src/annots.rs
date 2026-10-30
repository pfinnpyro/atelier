//! Annotations made in the viewer: highlights over text, comments (sticky
//! notes) and ink (freehand drawing and signatures). Each is written with
//! its own appearance stream, so every PDF app draws it the same way.

use atelier_raster::Point;
use atelier_render::TextGlyph;
use atelier_syntax::{Dict, Name, ObjRef, Object, PdfString, Result, Stream, Update};

/// What an annotation marks.
#[derive(Debug, Clone, PartialEq)]
pub enum Markup {
    /// Boxes over text, each bottom left, bottom right, top right, top
    /// left in user space.
    Highlight { quads: Vec<[Point; 4]> },
    /// A comment pinned at a point (the icon's top left).
    Note { at: Point, text: String },
    /// Freehand lines, `width` points wide.
    Ink {
        strokes: Vec<Vec<Point>>,
        width: f64,
    },
}

/// An annotation to add, in the colour it is drawn with (RGB, 0 to 1).
#[derive(Debug, Clone, PartialEq)]
pub struct NewAnnot {
    pub markup: Markup,
    pub color: [f64; 3],
}

/// The size of a comment's icon, in points.
pub const NOTE_SIZE: f64 = 20.0;

impl NewAnnot {
    /// The box the annotation covers, in user space.
    pub fn rect(&self) -> [f64; 4] {
        let grow = |pts: &mut dyn Iterator<Item = Point>, by: f64| {
            let mut r = [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ];
            for p in pts {
                r = [r[0].min(p.x), r[1].min(p.y), r[2].max(p.x), r[3].max(p.y)];
            }
            if r[0] > r[2] {
                return [0.0; 4];
            }
            [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
        };
        match &self.markup {
            Markup::Highlight { quads } => grow(&mut quads.iter().flatten().copied(), 0.0),
            Markup::Note { at, .. } => [at.x, at.y - NOTE_SIZE, at.x + NOTE_SIZE, at.y],
            Markup::Ink { strokes, width } => {
                grow(&mut strokes.iter().flatten().copied(), width / 2.0 + 1.0)
            }
        }
    }
}

/// Joins the glyphs of a selection into one box per line of text.
pub fn line_quads(glyphs: &[TextGlyph]) -> Vec<[Point; 4]> {
    let mut out: Vec<[Point; 4]> = Vec::new();
    let mut last: Option<[Point; 4]> = None;
    for g in glyphs {
        let [bl, br, tr, tl] = g.quad;
        let height = (tl - bl).length();
        if height <= 0.0 {
            continue;
        }
        let joins = last.is_some_and(|q| {
            let along = q[1] - q[0];
            let len = along.length().max(1e-9);
            let gap = bl - q[1];
            // How far the glyph sits from the line, square to it.
            let off = (gap.x * -along.y + gap.y * along.x).abs() / len;
            off < height * 0.5 && gap.length() < height * 3.0
        });
        if joins {
            if let Some(q) = out.last_mut() {
                q[1] = br;
                q[2] = tr;
            }
        } else if !g.text.trim().is_empty() {
            out.push([bl, br, tr, tl]);
        } else {
            continue;
        }
        last = out.last().copied();
    }
    out
}

fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn rect_obj(r: [f64; 4]) -> Object {
    Object::Array(r.iter().map(|&v| Object::Real(v)).collect())
}

fn name(s: &str) -> Object {
    Object::Name(Name::new(s))
}

/// The time now as a PDF date, in UTC.
fn pdf_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// The drawing of an annotation, in user space.
fn appearance(a: &NewAnnot) -> (Vec<u8>, Dict) {
    let [r, g, b] = a.color.map(num);
    let mut resources = Dict::new();
    let mut ops = String::new();
    match &a.markup {
        Markup::Highlight { quads } => {
            // Multiply, so the text shows through like a marker pen.
            let mut gs = Dict::new();
            gs.insert("Type", name("ExtGState"));
            gs.insert("BM", name("Multiply"));
            let mut states = Dict::new();
            states.insert("H", Object::Dict(gs));
            resources.insert("ExtGState", Object::Dict(states));
            ops.push_str(&format!("/H gs {r} {g} {b} rg\n"));
            for q in quads {
                ops.push_str(&format!(
                    "{} {} m {} {} l {} {} l {} {} l h f\n",
                    num(q[0].x),
                    num(q[0].y),
                    num(q[1].x),
                    num(q[1].y),
                    num(q[2].x),
                    num(q[2].y),
                    num(q[3].x),
                    num(q[3].y)
                ));
            }
        }
        Markup::Note { at, .. } => {
            // A folded note: the colour, a dark edge and three lines.
            let (x, y, s) = (at.x, at.y - NOTE_SIZE, NOTE_SIZE);
            ops.push_str(&format!(
                "{r} {g} {b} rg 0.25 0.25 0.25 RG 1 w\n\
                 {} {} {} {} re B\n0.25 0.25 0.25 RG 1.2 w\n",
                num(x + 0.5),
                num(y + 0.5),
                num(s - 1.0),
                num(s - 1.0)
            ));
            for i in 1..=3 {
                let ly = y + s - s * f64::from(i) / 4.0;
                ops.push_str(&format!(
                    "{} {} m {} {} l S\n",
                    num(x + 4.0),
                    num(ly),
                    num(x + s - 4.0),
                    num(ly)
                ));
            }
        }
        Markup::Ink { strokes, width } => {
            ops.push_str(&format!("{r} {g} {b} RG {} w 1 J 1 j\n", num(*width)));
            for s in strokes {
                let Some(first) = s.first() else { continue };
                ops.push_str(&format!("{} {} m\n", num(first.x), num(first.y)));
                if s.len() == 1 {
                    // A dot.
                    ops.push_str(&format!("{} {} l\n", num(first.x), num(first.y)));
                }
                for p in &s[1..] {
                    ops.push_str(&format!("{} {} l\n", num(p.x), num(p.y)));
                }
                ops.push_str("S\n");
            }
        }
    }
    (ops.into_bytes(), resources)
}

/// Writes an annotation onto a page and returns it.
pub(crate) fn add(update: &mut Update, page: ObjRef, a: &NewAnnot, n: usize) -> Result<ObjRef> {
    let rect = a.rect();
    let (content, resources) = appearance(a);
    let mut ap = Dict::new();
    ap.insert("Type", name("XObject"));
    ap.insert("Subtype", name("Form"));
    ap.insert("BBox", rect_obj(rect));
    ap.insert("Resources", Object::Dict(resources));
    let packed = atelier_filters::encode_flate(&content);
    ap.insert("Filter", name("FlateDecode"));
    ap.insert("Length", Object::Integer(packed.len() as i64));
    let ap = update.add(Object::Stream(Stream {
        dict: ap,
        data: packed.into(),
    }));
    let mut d = Dict::new();
    d.insert("Type", name("Annot"));
    let date = pdf_date();
    d.insert(
        "NM",
        Object::String(PdfString::from_text(&format!("atelier-{date}-{n}"))),
    );
    d.insert("M", Object::String(PdfString::from_text(&date)));
    d.insert("CreationDate", Object::String(PdfString::from_text(&date)));
    d.insert("Rect", rect_obj(rect));
    d.insert(
        "C",
        Object::Array(a.color.iter().map(|&c| Object::Real(c)).collect()),
    );
    // Print.
    d.insert("F", Object::Integer(4));
    d.insert("P", Object::Reference(page));
    let mut aps = Dict::new();
    aps.insert("N", Object::Reference(ap));
    d.insert("AP", Object::Dict(aps));
    match &a.markup {
        Markup::Highlight { quads } => {
            d.insert("Subtype", name("Highlight"));
            // QuadPoints go top left, top right, bottom left, bottom right.
            let mut pts = Vec::new();
            for q in quads {
                for p in [q[3], q[2], q[0], q[1]] {
                    pts.push(Object::Real(p.x));
                    pts.push(Object::Real(p.y));
                }
            }
            d.insert("QuadPoints", Object::Array(pts));
        }
        Markup::Note { text, .. } => {
            d.insert("Subtype", name("Text"));
            d.insert("Name", name("Comment"));
            d.insert("Contents", Object::String(PdfString::from_text(text)));
            d.insert("Open", Object::Bool(false));
            // NoZoom and NoRotate, as sticky notes are.
            d.insert("F", Object::Integer(4 | 8 | 16));
        }
        Markup::Ink { strokes, width } => {
            d.insert("Subtype", name("Ink"));
            let list = strokes
                .iter()
                .map(|s| {
                    Object::Array(
                        s.iter()
                            .flat_map(|p| [Object::Real(p.x), Object::Real(p.y)])
                            .collect(),
                    )
                })
                .collect();
            d.insert("InkList", Object::Array(list));
            let mut bs = Dict::new();
            bs.insert("W", Object::Real(*width));
            d.insert("BS", Object::Dict(bs));
        }
    }
    let annot = update.add(Object::Dict(d));
    let Object::Dict(mut page_dict) = update.get(page)? else {
        return Ok(annot);
    };
    let mut list = match page_dict.get(b"Annots") {
        Some(a) => match update.resolve(a)? {
            Object::Array(items) => items,
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    list.push(Object::Reference(annot));
    page_dict.insert("Annots", Object::Array(list));
    update.set(page, Object::Dict(page_dict));
    Ok(annot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_pdf_dates() {
        let d = pdf_date();
        assert!(
            d.starts_with("D:20") && d.ends_with('Z') && d.len() == 17,
            "{d}"
        );
    }
}
