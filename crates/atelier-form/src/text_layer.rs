//! Invisible text over scanned pages: the words OCR read, written into the
//! page the way scanners make searchable PDFs (text render mode 3), so
//! other PDF apps can search, select and copy them too.

use atelier_raster::Point;
use atelier_render::{PageText, TextFont};
use atelier_syntax::{Dict, Document, Name, ObjRef, Object, Result, Stream, Update};

/// The font name the layer uses in the page's resources.
const FONT: &str = "AtelierOCR";
/// Helvetica's ascender and descender, in thousandths of the font size.
const ASCENT: f64 = 718.0;
const DESCENT: f64 = 207.0;

/// A word of the layer: its text and its box (bottom left, bottom right,
/// top left) in user space.
struct Word {
    text: String,
    bl: Point,
    br: Point,
    tl: Point,
}

/// Groups glyphs into words: a space glyph, or a jump away from the line,
/// ends one.
fn words(text: &PageText) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut open = false;
    for g in &text.glyphs {
        if g.text.trim().is_empty() {
            open = false;
            continue;
        }
        let [bl, br, _, tl] = g.quad;
        if let Some(w) = out.last_mut().filter(|_| open) {
            let gap = (bl - w.br).length();
            let height = (w.tl - w.bl).length().max(1e-6);
            if gap < height {
                w.text.push_str(&g.text);
                w.br = br;
                continue;
            }
        }
        out.push(Word {
            text: g.text.clone(),
            bl,
            br,
            tl,
        });
        open = true;
    }
    out
}

/// Helvetica, as the layer's font object and its metrics.
fn font(doc: &Document) -> (Dict, TextFont) {
    let mut dict = Dict::new();
    dict.insert("Type", Object::Name(Name::new("Font")));
    dict.insert("Subtype", Object::Name(Name::new("Type1")));
    dict.insert("BaseFont", Object::Name(Name::new("Helvetica")));
    dict.insert("Encoding", Object::Name(Name::new("WinAnsiEncoding")));
    let metrics = TextFont::load(doc, &dict);
    (dict, metrics)
}

fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The content stream that draws `text` invisibly.
fn content(text: &PageText, metrics: &TextFont) -> Vec<u8> {
    let mut out = format!("q BT 3 Tr /{FONT} 1 Tf\n");
    for w in words(text) {
        let along = w.br - w.bl;
        let len = along.length();
        let height = (w.tl - w.bl).length();
        if len <= 0.0 || height <= 0.0 {
            continue;
        }
        let u = Point::new(along.x / len, along.y / len);
        // Up the page, square to the line.
        let v = Point::new(-u.y, u.x);
        // The box runs from the descender to the ascender.
        let size = height * 1000.0 / (ASCENT + DESCENT);
        let lift = size * DESCENT / 1000.0;
        let origin = Point::new(w.bl.x + v.x * lift, w.bl.y + v.y * lift);
        let bytes: Vec<u8> = w
            .text
            .chars()
            .flat_map(|c| {
                metrics
                    .encode(&c.to_string())
                    .unwrap_or_else(|| b"?".to_vec())
            })
            .collect();
        let natural = metrics.width(&bytes) * size;
        let scale = if natural > 0.0 {
            (100.0 * len / natural).clamp(10.0, 1000.0)
        } else {
            100.0
        };
        let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
        out.push_str(&format!(
            "{} {} {} {} {} {} Tm {} Tz <{hex}> Tj\n",
            num(u.x * size),
            num(u.y * size),
            num(v.x * size),
            num(v.y * size),
            num(origin.x),
            num(origin.y),
            num(scale)
        ));
    }
    out.push_str("ET Q\n");
    out.into_bytes()
}

fn stream(update: &mut Update, data: &[u8]) -> ObjRef {
    let packed = atelier_filters::encode_flate(data);
    let mut dict = Dict::new();
    dict.insert("Length", Object::Integer(packed.len() as i64));
    dict.insert("Filter", Object::Name(Name::new("FlateDecode")));
    update.add(Object::Stream(Stream {
        dict,
        data: packed.into(),
    }))
}

/// Adds the layers: each page's words drawn invisibly over it. Each
/// layer names the page object, and the resources the page inherits for
/// when it has none of its own.
pub(crate) fn add_layers(update: &mut Update, layers: &[(ObjRef, &Dict, &PageText)]) -> Result<()> {
    if layers.is_empty() {
        return Ok(());
    }
    let (font_dict, metrics) = font(update.document());
    let font_ref = update.add(Object::Dict(font_dict));
    for &(page_ref, inherited, text) in layers {
        let Object::Dict(mut dict) = update.get(page_ref)? else {
            continue;
        };
        // The page's own drawing is wrapped in q/Q, so whatever state it
        // leaves behind does not move the layer.
        let before = stream(update, b"q\n");
        let mut after = b"Q\n".to_vec();
        after.extend(content(text, &metrics));
        let after = stream(update, &after);
        let mut parts = vec![Object::Reference(before)];
        match update.resolve(dict.get(b"Contents").unwrap_or(&Object::Null))? {
            Object::Array(items) => parts.extend(items),
            Object::Null => {}
            _ => parts.push(dict.get(b"Contents").cloned().unwrap_or(Object::Null)),
        }
        parts.push(Object::Reference(after));
        dict.insert("Contents", Object::Array(parts));
        // The page's resources (inherited ones included) with the font.
        let mut resources = match dict.get(b"Resources") {
            Some(r) => match update.resolve(r)? {
                Object::Dict(d) => d,
                _ => inherited.clone(),
            },
            None => inherited.clone(),
        };
        let mut fonts = match resources.get(b"Font") {
            Some(f) => match update.resolve(f)? {
                Object::Dict(d) => d,
                _ => Dict::new(),
            },
            None => Dict::new(),
        };
        fonts.insert(FONT, Object::Reference(font_ref));
        resources.insert("Font", Object::Dict(fonts));
        dict.insert("Resources", Object::Dict(resources));
        update.set(page_ref, Object::Dict(dict));
    }
    Ok(())
}
