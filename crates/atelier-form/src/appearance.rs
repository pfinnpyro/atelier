//! Draws field appearance streams (ISO 32000-2, 12.7.4.3 and 12.5.6.19):
//! the background and border from /MK, then the value in the /DA font.

use std::fmt::Write as _;

use atelier_render::TextFont;
use atelier_syntax::{Dict, Document, Name, Object, PdfString};

use crate::da::{DefaultAppearance, num};
use crate::{Field, FieldKind, Value, Widget, flags};

/// Line spacing for multi-line text and list boxes, in font sizes.
const LINE_HEIGHT: f64 = 1.15;
/// Smallest and largest sizes that auto-sizing picks.
const MIN_AUTO_SIZE: f64 = 4.0;
const MAX_AUTO_SIZE_MULTILINE: f64 = 12.0;
/// The highlight behind selected list box options.
const SELECTION: &str = "0.6 0.75 0.86 rg";

/// A built appearance stream.
pub(crate) struct Appearance {
    pub content: Vec<u8>,
    pub resources: Dict,
    pub bbox: [f64; 4],
    pub matrix: Option<[f64; 6]>,
}

impl Appearance {
    pub fn into_stream(self) -> Object {
        let mut dict = Dict::new();
        dict.insert("Type", Object::Name(Name::new("XObject")));
        dict.insert("Subtype", Object::Name(Name::new("Form")));
        dict.insert(
            "BBox",
            Object::Array(self.bbox.iter().map(|&v| Object::Real(v)).collect()),
        );
        if let Some(m) = self.matrix {
            dict.insert(
                "Matrix",
                Object::Array(m.iter().map(|&v| Object::Real(v)).collect()),
            );
        }
        dict.insert("Resources", Object::Dict(self.resources));
        Object::Stream(atelier_syntax::Stream {
            dict,
            data: self.content.into(),
        })
    }
}

/// The widget's box, background and border.
pub(crate) struct Frame {
    /// Size of the appearance box: the rect, turned by /MK /R.
    pub w: f64,
    pub h: f64,
    matrix: Option<[f64; 6]>,
    background: Option<Vec<f64>>,
    border_color: Option<Vec<f64>>,
    pub border_width: f64,
    border_style: u8,
    dash: Vec<f64>,
}

impl Frame {
    pub fn new(doc: &Document, dict: &Dict, widget: &Widget) -> Self {
        let (rw, rh) = (
            (widget.rect.x1 - widget.rect.x0).abs(),
            (widget.rect.y1 - widget.rect.y0).abs(),
        );
        let (w, h, matrix) = match widget.rotation {
            90 => (rh, rw, Some([0.0, 1.0, -1.0, 0.0, rh, 0.0])),
            180 => (rw, rh, Some([-1.0, 0.0, 0.0, -1.0, rw, rh])),
            270 => (rh, rw, Some([0.0, -1.0, 1.0, 0.0, 0.0, rw])),
            _ => (rw, rh, None),
        };
        let mk = dict_of(doc, dict, b"MK");
        let colour = |key: &[u8]| -> Option<Vec<f64>> {
            let arr = doc.get_key(&mk, key).ok()?;
            let v: Vec<f64> = arr.as_array()?.iter().filter_map(Object::as_f64).collect();
            matches!(v.len(), 1 | 3 | 4).then_some(v)
        };
        let bs = dict_of(doc, dict, b"BS");
        let mut border_width = doc
            .get_key(&bs, b"W")
            .ok()
            .and_then(|o| o.as_f64())
            .unwrap_or(1.0);
        if !bs.contains_key(b"W") {
            if let Ok(Object::Array(b)) = doc.get_key(dict, b"Border") {
                if let Some(w) = b.get(2).and_then(Object::as_f64) {
                    border_width = w;
                }
            }
        }
        let border_style = doc
            .get_key(&bs, b"S")
            .ok()
            .and_then(|o| o.as_name().and_then(|n| n.0.first().copied()))
            .unwrap_or(b'S');
        let dash = doc
            .get_key(&bs, b"D")
            .ok()
            .and_then(|o| {
                o.as_array()
                    .map(|a| a.iter().filter_map(Object::as_f64).collect())
            })
            .unwrap_or_else(|| vec![3.0]);
        let border_color = colour(b"BC");
        Self {
            w,
            h,
            matrix,
            background: colour(b"BG"),
            border_width: if border_color.is_some() {
                border_width.clamp(0.0, w.min(h) / 2.0)
            } else {
                0.0
            },
            border_color,
            border_style,
            dash,
        }
    }

    /// Room between the box edge and the text.
    pub fn padding(&self) -> f64 {
        2.0 * self.border_width.max(1.0)
    }

    /// Background and border, for a rectangular widget.
    fn draw(&self, out: &mut String) {
        let (w, h, b) = (self.w, self.h, self.border_width);
        if let Some(bg) = &self.background {
            let _ = writeln!(out, "{} 0 0 {} {} re f", fill(bg), num(w), num(h));
        }
        let Some(bc) = &self.border_color else {
            return;
        };
        if b <= 0.0 {
            return;
        }
        match self.border_style {
            b'U' => {
                let _ = writeln!(
                    out,
                    "{} {} w 0 {} m {} {} l S",
                    stroke(bc),
                    num(b),
                    num(b / 2.0),
                    num(w),
                    num(b / 2.0)
                );
            }
            style => {
                if style == b'D' {
                    let dash: Vec<String> = self.dash.iter().map(|&d| num(d)).collect();
                    let _ = writeln!(out, "[{}] 0 d", dash.join(" "));
                }
                let _ = writeln!(
                    out,
                    "{} {} w {} {} {} {} re S",
                    stroke(bc),
                    num(b),
                    num(b / 2.0),
                    num(b / 2.0),
                    num(w - b),
                    num(h - b)
                );
                if style == b'D' {
                    out.push_str("[] 0 d\n");
                }
                if matches!(style, b'B' | b'I') {
                    // Beveled and inset borders: a light top-left edge and a
                    // dark bottom-right one inside the border.
                    let (light, dark) = if style == b'B' {
                        (
                            "1 g".to_string(),
                            match &self.background {
                                Some(bg) => fill(&bg.iter().map(|c| c * 0.5).collect::<Vec<_>>()),
                                None => "0.5 g".to_string(),
                            },
                        )
                    } else {
                        ("0.5 g".to_string(), "0.75 g".to_string())
                    };
                    let (x0, y0, x1, y1) = (b, b, w - b, h - b);
                    let _ = writeln!(
                        out,
                        "{light} {} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f",
                        num(x0),
                        num(y0),
                        num(x0),
                        num(y1),
                        num(x1),
                        num(y1),
                        num(x1 - b),
                        num(y1 - b),
                        num(x0 + b),
                        num(y1 - b),
                        num(x0 + b),
                        num(y0 + b)
                    );
                    let _ = writeln!(
                        out,
                        "{dark} {} {} m {} {} l {} {} l {} {} l {} {} l {} {} l f",
                        num(x1),
                        num(y1),
                        num(x1),
                        num(y0),
                        num(x0),
                        num(y0),
                        num(x0 + b),
                        num(y0 + b),
                        num(x1 - b),
                        num(y0 + b),
                        num(x1 - b),
                        num(y1 - b)
                    );
                }
            }
        }
    }

    /// Background and border, for a round radio button.
    fn draw_round(&self, out: &mut String) {
        let r = self.w.min(self.h) / 2.0;
        let (cx, cy) = (self.w / 2.0, self.h / 2.0);
        if let Some(bg) = &self.background {
            let _ = writeln!(out, "{} {} f", fill(bg), circle(cx, cy, r));
        }
        if let Some(bc) = &self.border_color {
            if self.border_width > 0.0 {
                let _ = writeln!(
                    out,
                    "{} {} w {} S",
                    stroke(bc),
                    num(self.border_width),
                    circle(cx, cy, r - self.border_width / 2.0)
                );
            }
        }
    }

    fn finish(&self, content: String, resources: Dict) -> Appearance {
        Appearance {
            content: content.into_bytes(),
            resources,
            bbox: [0.0, 0.0, self.w, self.h],
            matrix: self.matrix,
        }
    }
}

/// A font chosen for drawing a value: its resource name, the object to put
/// in the appearance's resources, and its metrics.
pub(crate) struct ChosenFont {
    pub name: Vec<u8>,
    pub object: Object,
    pub font: TextFont,
}

/// Helvetica with WinAnsiEncoding, for values the field's own font cannot
/// show (often a subset holding only the characters it was made with).
pub(crate) fn fallback_font(doc: &Document) -> ChosenFont {
    let mut dict = Dict::new();
    dict.insert("Type", Object::Name(Name::new("Font")));
    dict.insert("Subtype", Object::Name(Name::new("Type1")));
    dict.insert("BaseFont", Object::Name(Name::new("Helvetica")));
    dict.insert("Encoding", Object::Name(Name::new("WinAnsiEncoding")));
    let font = TextFont::load(doc, &dict);
    ChosenFont {
        name: b"AtelierHelv".to_vec(),
        object: Object::Dict(dict),
        font,
    }
}

/// The DA font from the form's resources, if it can show all of `texts`;
/// otherwise the fallback.
pub(crate) fn choose_font(
    doc: &Document,
    dr: &Dict,
    da: &DefaultAppearance,
    texts: &[&str],
) -> ChosenFont {
    if let Some(name) = &da.font {
        let fonts = dict_of(doc, dr, b"Font");
        if let Some(obj) = fonts.get(name) {
            if let Ok(Object::Dict(dict)) = doc.resolve(obj) {
                let font = TextFont::load(doc, &dict);
                if texts.iter().all(|t| font.encode(t).is_some()) {
                    return ChosenFont {
                        name: name.clone(),
                        object: obj.clone(),
                        font,
                    };
                }
            }
        }
    }
    fallback_font(doc)
}

/// Encodes text, putting `?` in place of characters the font lacks.
/// Returns the bytes and whether anything was replaced.
pub(crate) fn encode_lossy(font: &TextFont, text: &str) -> (Vec<u8>, bool) {
    if let Some(b) = font.encode(text) {
        return (b, false);
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 4];
    for c in text.chars() {
        let s = c.encode_utf8(&mut buf);
        match font.encode(s) {
            Some(b) => out.extend(b),
            None => out.extend(font.encode("?").unwrap_or_default()),
        }
    }
    (out, true)
}

/// What a text-like appearance shows.
pub(crate) enum Content<'a> {
    /// A text field or combo box: one value.
    Text(&'a str),
    /// A list box: every option, and which are selected.
    List(Vec<(&'a str, bool)>),
}

/// The appearance of a text field, combo box or list box.
pub(crate) fn text_appearance(
    frame: &Frame,
    field: &Field,
    da: &DefaultAppearance,
    chosen: &ChosenFont,
    content: &Content<'_>,
    lossy: &mut bool,
) -> Appearance {
    let mut out = String::new();
    frame.draw(&mut out);
    let font = &chosen.font;
    let pad = frame.padding();
    let (w, h) = (frame.w, frame.h);
    let inner_w = (w - 2.0 * pad).max(0.0);
    let (asc, desc) = (font.ascent(), font.descent());
    let mut runs: Vec<(f64, f64, Vec<u8>)> = Vec::new();
    let mut highlights: Vec<(f64, f64)> = Vec::new();
    let size;
    match content {
        Content::Text(text) => {
            let shown: String = if field.has_flag(flags::PASSWORD) {
                "*".repeat(text.chars().count())
            } else {
                (*text).to_string()
            };
            let multiline = field.has_flag(flags::MULTILINE) && field.kind == FieldKind::Text;
            let comb = field.has_flag(flags::COMB)
                && field.kind == FieldKind::Text
                && !multiline
                && !field.has_flag(flags::PASSWORD)
                && field.max_len.is_some_and(|m| m > 0);
            if comb {
                let cells = field.max_len.unwrap_or(1).max(1);
                let cell = w / cells as f64;
                size = if da.size > 0.0 {
                    da.size
                } else {
                    ((h - 2.0 * frame.border_width - 2.0) / LINE_HEIGHT)
                        .clamp(MIN_AUTO_SIZE, cell.max(MIN_AUTO_SIZE))
                };
                if let (Some(bc), true) = (&frame.border_color, frame.border_width > 0.0) {
                    // Acrobat draws a divider between comb cells.
                    let _ = write!(out, "{} {} w", stroke(bc), num(frame.border_width));
                    for i in 1..cells {
                        let x = cell * i as f64;
                        let _ = write!(out, " {} 0 m {} {} l", num(x), num(x), num(h));
                    }
                    out.push_str(" S\n");
                }
                let y = baseline_centred(h, size, asc, desc);
                for (i, c) in shown.chars().take(cells).enumerate() {
                    let (bytes, l) = encode_lossy(font, c.encode_utf8(&mut [0; 4]));
                    *lossy |= l;
                    let cw = font.width(&bytes) * size;
                    runs.push((cell * i as f64 + (cell - cw) / 2.0, y, bytes));
                }
            } else if multiline {
                size = if da.size > 0.0 {
                    da.size
                } else {
                    auto_size_multiline(font, &shown, inner_w, h - 2.0 * pad, da)
                };
                let lines = wrap_measured(&shown, inner_w, |s| spaced_width(font, s, size, da));
                let mut y = h - pad - asc * size;
                for line in lines {
                    let (bytes, l) = encode_lossy(font, &line);
                    *lossy |= l;
                    let lw = spaced_width(font, &line, size, da);
                    runs.push((align(field.quadding, pad, w, lw), y, bytes));
                    y -= line_step(size, da);
                }
            } else {
                let (bytes, l) = encode_lossy(font, &shown);
                *lossy |= l;
                let unit = font.width(&bytes);
                size = if da.size > 0.0 {
                    da.size
                } else {
                    let by_height = (h - 2.0 * frame.border_width - 2.0) / LINE_HEIGHT;
                    let by_width = if unit > 0.0 {
                        (inner_w - (spaced_width(font, &shown, 0.0, da))).max(0.0) / unit
                    } else {
                        by_height
                    };
                    by_height.min(by_width).max(MIN_AUTO_SIZE)
                };
                let tw = spaced_width(font, &shown, size, da);
                // Text too long for the box starts at the left, like Acrobat.
                let x = if tw > inner_w {
                    pad
                } else {
                    align(field.quadding, pad, w, tw)
                };
                runs.push((x, baseline_centred(h, size, asc, desc), bytes));
            }
        }
        Content::List(options) => {
            size = if da.size > 0.0 {
                da.size
            } else {
                MAX_AUTO_SIZE_MULTILINE
            };
            let step = line_step(size, da);
            let mut top = h - frame.border_width;
            for (text, selected) in options.iter().skip(field.top_index) {
                if top <= frame.border_width {
                    break;
                }
                if *selected {
                    highlights.push((top - step, step));
                }
                let (bytes, l) = encode_lossy(font, text);
                *lossy |= l;
                let y = top - step + (step - (asc - desc) * size) / 2.0 - desc * size;
                runs.push((pad, y, bytes));
                top -= step;
            }
        }
    }

    let b = frame.border_width;
    out.push_str("/Tx BMC\nq\n");
    let _ = writeln!(
        out,
        "{} {} {} {} re W n",
        num(b),
        num(b),
        num(w - 2.0 * b),
        num(h - 2.0 * b)
    );
    for (y, hh) in highlights {
        let _ = writeln!(
            out,
            "{SELECTION} {} {} {} {} re f",
            num(b),
            num(y),
            num(w - 2.0 * b),
            num(hh)
        );
    }
    if !runs.is_empty() {
        out.push_str("BT\n");
        let _ = writeln!(out, "{} {} Tf", Name(chosen.name.clone()), num(size));
        let _ = writeln!(out, "{}", da.color);
        let _ = writeln!(
            out,
            "{} Tc {} Tw",
            num(da.character_spacing),
            num(da.word_spacing)
        );
        for (x, y, bytes) in runs {
            let s = PdfString::literal(bytes);
            let _ = writeln!(out, "1 0 0 1 {} {} Tm {s} Tj", num(x), num(y));
        }
        out.push_str("ET\n");
    }
    out.push_str("Q\nEMC\n");
    let mut fonts = Dict::new();
    fonts.insert(Name(chosen.name.clone()), chosen.object.clone());
    let mut resources = Dict::new();
    resources.insert("Font", Object::Dict(fonts));
    frame.finish(out, resources)
}

/// The on and off appearances of a checkbox or radio widget that has none:
/// the /MK /CA character in ZapfDingbats (a check, or a dot for radios).
pub(crate) fn button_appearance(
    doc: &Document,
    frame: &Frame,
    dict: &Dict,
    da: &DefaultAppearance,
    radio: bool,
) -> (Appearance, Appearance) {
    let mk = dict_of(doc, dict, b"MK");
    let ca = match doc.get_key(&mk, b"CA") {
        Ok(Object::String(s)) if !s.bytes.is_empty() => s.bytes[0],
        _ if radio => b'l',
        _ => b'4',
    };
    let mut zadb = Dict::new();
    zadb.insert("Type", Object::Name(Name::new("Font")));
    zadb.insert("Subtype", Object::Name(Name::new("Type1")));
    zadb.insert("BaseFont", Object::Name(Name::new("ZapfDingbats")));
    let font = TextFont::load(doc, &zadb);
    let (w, h) = (frame.w, frame.h);
    let mut background = String::new();
    if radio {
        frame.draw_round(&mut background);
    } else {
        frame.draw(&mut background);
    }
    let mut on = background.clone();
    // Common marks are drawn as shapes, so they show in any viewer whether
    // or not it has a ZapfDingbats font; other characters are text.
    let side = ((w.min(h) - 2.0 * frame.padding()) * if radio { 0.6 } else { 0.85 }).max(1.0);
    let side = if da.size > 0.0 {
        side.min(da.size)
    } else {
        side
    };
    if let Some(shape) = mark_shape(ca, (w - side) / 2.0, (h - side) / 2.0, side) {
        let _ = writeln!(on, "q {} {shape} Q", da.color);
    } else {
        let unit = font.width(&[ca]).max(0.1);
        let size = if da.size > 0.0 {
            da.size
        } else {
            let room = w.min(h) - 2.0 * frame.padding();
            ((w.min(h) * 0.8).min(room.max(1.0) / unit)).max(1.0)
        };
        let (asc, desc) = (font.ascent(), font.descent());
        let x = (w - unit * size) / 2.0;
        let y = baseline_centred(h, size, asc, desc);
        let _ = writeln!(
            on,
            "q BT /ZaDb {} Tf {} 1 0 0 1 {} {} Tm {} Tj ET Q",
            num(size),
            da.color,
            num(x),
            num(y),
            PdfString::literal(vec![ca])
        );
    }
    let mut fonts = Dict::new();
    fonts.insert("ZaDb", Object::Dict(zadb));
    let mut resources = Dict::new();
    resources.insert("Font", Object::Dict(fonts));
    (
        frame.finish(on, resources.clone()),
        frame.finish(background, resources),
    )
}

/// A ZapfDingbats mark as a filled path in the square at (x, y) of
/// side `s`: check (4), cross (8), dot (l), square (n), diamond (u), star (H).
fn mark_shape(ca: u8, x: f64, y: f64, s: f64) -> Option<String> {
    let poly = |pts: &[(f64, f64)]| {
        let mut out = String::new();
        for (i, (px, py)) in pts.iter().enumerate() {
            let op = if i == 0 { "m" } else { "l" };
            let _ = write!(out, "{} {} {op} ", num(x + px * s), num(y + py * s));
        }
        out.push('h');
        out
    };
    Some(match ca {
        b'4' => format!(
            "{} f",
            poly(&[
                (0.0, 0.52),
                (0.14, 0.66),
                (0.38, 0.40),
                (0.86, 0.96),
                (1.0, 0.84),
                (0.38, 0.12)
            ])
        ),
        b'8' => format!(
            "{} {} f",
            poly(&[(0.0, 0.12), (0.12, 0.0), (1.0, 0.88), (0.88, 1.0)]),
            poly(&[(0.0, 0.88), (0.88, 0.0), (1.0, 0.12), (0.12, 1.0)])
        ),
        b'l' => format!("{} f", circle(x + s / 2.0, y + s / 2.0, s / 2.0)),
        b'n' => format!("{} {} {} {} re f", num(x), num(y), num(s), num(s)),
        b'u' => format!(
            "{} f",
            poly(&[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)])
        ),
        b'H' => {
            let pts: Vec<(f64, f64)> = (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 0.5 } else { 0.2 };
                    let a = std::f64::consts::FRAC_PI_2 + f64::from(i) * std::f64::consts::PI / 5.0;
                    (0.5 + r * a.cos(), 0.5 + r * a.sin())
                })
                .collect();
            format!("{} f", poly(&pts))
        }
        _ => return None,
    })
}

/// The text a choice field shows, and for list boxes every option with
/// whether it is selected.
pub(crate) fn choice_content(field: &Field) -> (String, Vec<(String, bool)>) {
    let selected: Vec<String> = match &field.value {
        Value::Text(t) | Value::State(t) => vec![t.clone()],
        Value::Choices(c) => c.clone(),
        Value::None => Vec::new(),
    };
    let list = field
        .options
        .iter()
        .map(|o| {
            let on = selected.iter().any(|s| *s == o.export || *s == o.display);
            (o.display.clone(), on)
        })
        .collect();
    (field.display_value(), list)
}

fn baseline_centred(h: f64, size: f64, asc: f64, desc: f64) -> f64 {
    (h - (asc - desc) * size) / 2.0 - desc * size
}

fn align(q: u8, pad: f64, w: f64, tw: f64) -> f64 {
    match q {
        1 => (w - tw) / 2.0,
        2 => w - pad - tw,
        _ => pad,
    }
}

/// The largest size up to 12 at which wrapped text fits the box.
fn auto_size_multiline(font: &TextFont, text: &str, w: f64, h: f64, da: &DefaultAppearance) -> f64 {
    let mut size = MAX_AUTO_SIZE_MULTILINE;
    while size > MIN_AUTO_SIZE {
        let lines = wrap_measured(text, w, |s| spaced_width(font, s, size, da)).len() as f64;
        if lines * line_step(size, da) <= h {
            return size;
        }
        size -= 0.5;
    }
    MIN_AUTO_SIZE
}

/// Breaks text into lines no wider than `width` (in font size units):
/// at line breaks, then between words, then inside words that are too long.
fn line_step(size: f64, da: &DefaultAppearance) -> f64 {
    if da.leading > 0.0 {
        da.leading
    } else {
        size * LINE_HEIGHT
    }
}

fn spaced_width(font: &TextFont, text: &str, size: f64, da: &DefaultAppearance) -> f64 {
    // Word spacing applies only to single-byte character code 32 in PDF.
    let bytes = encode_lossy(font, text).0;
    let chars = text.chars().count();
    let spaces = if bytes.len() == chars {
        bytes.iter().filter(|&&b| b == 32).count()
    } else {
        0
    };
    font.width(&bytes) * size
        + chars.saturating_sub(1) as f64 * da.character_spacing
        + spaces as f64 * da.word_spacing
}

fn wrap_measured(text: &str, width: f64, w: impl Fn(&str) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    let text = text.replace("\r\n", "\n");
    for para in text.split(['\n', '\r']) {
        let mut line = String::new();
        for word in para.split_inclusive(' ') {
            let candidate = format!("{line}{word}");
            if w(candidate.trim_end()) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(line.trim_end().to_string());
                line.clear();
            }
            if w(word.trim_end()) <= width {
                line.push_str(word);
                continue;
            }
            // A word wider than the box: break it between characters.
            for c in word.chars() {
                let mut next = line.clone();
                next.push(c);
                if !line.is_empty() && w(next.trim_end()) > width {
                    lines.push(std::mem::take(&mut line));
                    line.push(c);
                } else {
                    line = next;
                }
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

fn dict_of(doc: &Document, dict: &Dict, key: &[u8]) -> Dict {
    match doc.get_key(dict, key) {
        Ok(Object::Dict(d)) => d,
        _ => Dict::new(),
    }
}

fn colour_op(c: &[f64], ops: [&str; 3]) -> String {
    let nums: Vec<String> = c.iter().map(|&v| num(v)).collect();
    let op = match c.len() {
        1 => ops[0],
        3 => ops[1],
        _ => ops[2],
    };
    format!("{} {op}", nums.join(" "))
}

fn fill(c: &[f64]) -> String {
    colour_op(c, ["g", "rg", "k"])
}

fn stroke(c: &[f64]) -> String {
    colour_op(c, ["G", "RG", "K"])
}

/// A circle as four Bézier curves.
fn circle(cx: f64, cy: f64, r: f64) -> String {
    const K: f64 = 0.552_284_75;
    let k = r * K;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h",
        num(cx + r),
        num(cy),
        num(cx + r),
        num(cy + k),
        num(cx + k),
        num(cy + r),
        num(cx),
        num(cy + r),
        num(cx - k),
        num(cy + r),
        num(cx - r),
        num(cy + k),
        num(cx - r),
        num(cy),
        num(cx - r),
        num(cy - k),
        num(cx - k),
        num(cy - r),
        num(cx),
        num(cy - r),
        num(cx + k),
        num(cy - r),
        num(cx + r),
        num(cy - k),
        num(cx + r),
        num(cy),
    )
}
