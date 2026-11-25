use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use atelier_raster::{Point, Transform};
use atelier_syntax::{
    Dict, Document, Error, Name, ObjRef, Object, Page, Parser, PdfString, Rect, Result, Stream,
    Token, Update, read_inline_image,
};

use crate::font::PdfFont;

mod color;
mod fonts;
mod images;
mod transparency;
mod type3;

// Store a vertical glyph in a horizontal output font with a counter-rotation.
// Its horizontal selection cell then maps back to the original vertical cell.
const VERTICAL_GLYPH: Transform = Transform::new(0.0, 1.0, -1.0, 0.0, 0.0, 0.3);

/// Newly generated page content and resources in the destination PDF.
pub struct RedactedPage {
    pub content: Vec<u8>,
    pub resources: Dict,
    pub asset_streams: Vec<ObjRef>,
    pub group: Option<Dict>,
    pub user_unit: Option<f64>,
}

struct Assets<'a, 'doc> {
    output: &'a mut Update<'doc>,
    streams: Vec<ObjRef>,
    bytes: usize,
}

impl Assets<'_, '_> {
    fn add(&mut self, dict: Dict, data: Vec<u8>) -> Result<Object> {
        self.bytes += data.len();
        if self.bytes > 64 * 1024 * 1024 || self.streams.len() >= 16_384 {
            return Err(unsupported(
                "Page resources exceed the redaction export limit",
            ));
        }
        let reference = self.output.add(Object::Stream(Stream {
            dict,
            data: data.into(),
        }));
        self.streams.push(reference);
        Ok(Object::Reference(reference))
    }
}

fn unsupported(reason: impl Into<String>) -> Error {
    Error::Unsupported(reason.into())
}

fn read_operation<'a>(parser: &mut Parser<'a>) -> Result<Option<(Vec<Object>, &'a [u8])>> {
    let mut operands = Vec::new();
    while let Some(token) = parser.lexer().next_token()? {
        if let Token::Keyword(op) = token {
            return Ok(Some((operands, op)));
        }
        operands.push(parser.object_from(token, 0)?);
        if operands.len() > 64 {
            return Err(unsupported("Too many operands in a page operation"));
        }
    }
    if operands.is_empty() {
        Ok(None)
    } else {
        Err(unsupported("Dangling page operands"))
    }
}

fn number(value: &Object) -> Result<f64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && n.abs() <= 1.0e8)
        .ok_or_else(|| unsupported("Invalid page coordinate or graphics value"))
}

fn nums<const N: usize>(args: &[Object]) -> Result<[f64; N]> {
    if args.len() != N {
        return Err(unsupported("Unexpected PDF operator arguments"));
    }
    let mut out = [0.0; N];
    for (value, arg) in out.iter_mut().zip(args) {
        *value = number(arg)?;
    }
    Ok(out)
}

fn pdf_number(n: f64) -> String {
    Object::Real(n).to_string()
}

fn line(command: &[u8], values: &[f64]) -> String {
    let mut result = String::new();
    for &value in values {
        write!(result, "{} ", pdf_number(value)).unwrap();
    }
    writeln!(result, "{}", String::from_utf8_lossy(command)).unwrap();
    result
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

fn contains(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x0 && a.x1 >= b.x1 && a.y0 <= b.y0 && a.y1 >= b.y1
}

fn cut_rectangle(rect: Rect, marks: &[Rect]) -> Result<Vec<Rect>> {
    let mut pieces = vec![rect];
    for mark in marks {
        let mut next = Vec::new();
        for r in pieces {
            let Some(hole) = r.intersect(mark) else {
                next.push(r);
                continue;
            };
            next.extend(
                [
                    Rect { x1: hole.x0, ..r },
                    Rect { x0: hole.x1, ..r },
                    Rect {
                        x0: hole.x0,
                        x1: hole.x1,
                        y1: hole.y0,
                        ..r
                    },
                    Rect {
                        x0: hole.x0,
                        x1: hole.x1,
                        y0: hole.y1,
                        ..r
                    },
                ]
                .into_iter()
                .filter(|piece| piece.width() > 0.0 && piece.height() > 0.0),
            );
        }
        if next.len() > 2048 {
            return Err(unsupported(
                "The redaction splits a rectangle into too many pieces",
            ));
        }
        pieces = next;
    }
    Ok(pieces)
}

fn add_point(bounds: &mut Option<Rect>, p: Point) -> Result<()> {
    if !p.x.is_finite() || !p.y.is_finite() || p.x.abs().max(p.y.abs()) > 1.0e9 {
        return Err(unsupported("Page coordinates exceed the redaction limits"));
    }
    let area = bounds.get_or_insert(Rect {
        x0: p.x,
        y0: p.y,
        x1: p.x,
        y1: p.y,
    });
    area.x0 = area.x0.min(p.x);
    area.y0 = area.y0.min(p.y);
    area.x1 = area.x1.max(p.x);
    area.y1 = area.y1.max(p.y);
    Ok(())
}

fn add_box(bounds: &mut Option<Rect>, matrix: Transform, rect: Rect) -> Result<()> {
    for x in [rect.x0, rect.x1] {
        for y in [rect.y0, rect.y1] {
            add_point(bounds, matrix.apply(Point::new(x, y)))?;
        }
    }
    Ok(())
}

#[derive(Clone)]
struct State {
    ctm: Transform,
    fill: color::Paint,
    stroke: color::Paint,
    effects: transparency::Effects,
    width: f64,
    cap: f64,
    join: f64,
    miter: f64,
    font: Option<usize>,
    size: f64,
    spacing: f64,
    word: f64,
    scale: f64,
    leading: f64,
    rise: f64,
}

impl State {
    fn new(doc: &Document, resources: &Dict) -> Result<Self> {
        Ok(Self {
            ctm: Transform::IDENTITY,
            fill: color::Paint::device(doc, resources, b"g", &[Object::Integer(0)])?,
            stroke: color::Paint::device(doc, resources, b"G", &[Object::Integer(0)])?,
            effects: transparency::Effects::default(),
            width: 1.0,
            cap: 0.0,
            join: 0.0,
            miter: 10.0,
            font: None,
            size: 0.0,
            spacing: 0.0,
            word: 0.0,
            scale: 100.0,
            leading: 0.0,
            rise: 0.0,
        })
    }
}

struct Font {
    font: Arc<PdfFont>,
    kind: fonts::Kind,
    procedures: HashMap<u32, Arc<type3::Procedure>>,
}

struct StandardFont {
    dictionary: Dict,
    used: bool,
}

struct Rewriter<'a, 'out, 'doc> {
    doc: &'a Document,
    page: &'a Page,
    regions: &'a [Rect],
    state: State,
    stack: Vec<State>,
    path: String,
    path_bounds: Option<Rect>,
    single_rect: bool,
    path_steps: usize,
    single_line: bool,
    in_text: bool,
    tm: Transform,
    line_matrix: Transform,
    fonts: Vec<Font>,
    font_map: HashMap<Vec<u8>, usize>,
    images: Dict,
    effects: transparency::UsedEffects,
    procedure_bytes: usize,
    assets: Assets<'out, 'doc>,
    out: String,
}

impl Rewriter<'_, '_, '_> {
    fn select_font(&mut self, args: &[Object]) -> Result<()> {
        let [Object::Name(key), size] = args else {
            return Err(unsupported("Invalid text font operator"));
        };
        self.state.size = number(size)?;
        if self.state.size <= 0.0 {
            return Err(unsupported("Unsupported text size"));
        }
        if let Some(&id) = self.font_map.get(key.as_bytes()) {
            self.state.font = Some(id);
            return Ok(());
        }
        let Object::Dict(fonts) = self.doc.get_key(&self.page.resources, b"Font")? else {
            return Err(unsupported("Missing font resources"));
        };
        let Object::Dict(source) = self.doc.get_key(&fonts, key.as_bytes())? else {
            return Err(unsupported("Missing page font"));
        };
        let is_type3 = source
            .get(b"Subtype")
            .and_then(Object::as_name)
            .is_some_and(|n| n.as_bytes() == b"Type3");
        let (dictionary, kind) = if is_type3 {
            let matrix = self.doc.get_key(&source, b"FontMatrix")?;
            nums::<6>(
                matrix
                    .as_array()
                    .ok_or_else(|| unsupported("Invalid Type 3 font matrix"))?,
            )?;
            (source, fonts::Kind::Type3(fonts::Embedded::new(false)))
        } else if let Some(two_byte) = fonts::embedded(self.doc, &source)? {
            (
                source,
                fonts::Kind::Embedded(fonts::Embedded::new(two_byte)),
            )
        } else {
            let dictionary = Self::standard_font(&source)?;
            (
                dictionary.clone(),
                fonts::Kind::Standard(StandardFont {
                    dictionary,
                    used: false,
                }),
            )
        };
        let mut warnings = Vec::new();
        let font = PdfFont::load(self.doc, &dictionary, &mut |warning| warnings.push(warning));
        if !warnings.is_empty()
            || (matches!(&kind, fonts::Kind::Embedded(_)) && !font.has_embedded_program())
        {
            return Err(unsupported("The source font could not be verified"));
        }
        let id = self.fonts.len();
        self.fonts.push(Font {
            font: Arc::new(font),
            kind,
            procedures: HashMap::new(),
        });
        self.font_map.insert(key.as_bytes().to_vec(), id);
        self.state.font = Some(id);
        Ok(())
    }

    fn standard_font(source: &Dict) -> Result<Dict> {
        let font_name = source
            .get(b"BaseFont")
            .and_then(Object::as_name)
            .ok_or_else(|| unsupported("No built-in font name"))?;
        let approved = [
            b"Helvetica".as_slice(),
            b"Helvetica-Bold",
            b"Helvetica-Oblique",
            b"Helvetica-BoldOblique",
            b"Times-Roman",
            b"Times-Bold",
            b"Times-Italic",
            b"Times-BoldItalic",
            b"Courier",
            b"Courier-Bold",
            b"Courier-Oblique",
            b"Courier-BoldOblique",
        ];
        if !approved.contains(&font_name.as_bytes())
            || source
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_none_or(|n| n.as_bytes() != b"Type1")
            || source.iter().any(|(n, _)| {
                !matches!(
                    n.as_bytes(),
                    b"Type" | b"Subtype" | b"BaseFont" | b"Encoding" | b"Name"
                )
            })
        {
            return Err(unsupported("Unsupported non-embedded font"));
        }
        let mut dictionary = Dict::new();
        dictionary.insert("Type", Object::Name(Name::from("Font")));
        dictionary.insert("Subtype", Object::Name(Name::from("Type1")));
        dictionary.insert("BaseFont", Object::Name(font_name.clone()));
        if let Some(encoding) = source.get(b"Encoding") {
            if encoding.as_name().is_none_or(|n| {
                !matches!(
                    n.as_bytes(),
                    b"WinAnsiEncoding" | b"MacRomanEncoding" | b"StandardEncoding"
                )
            }) {
                return Err(unsupported("Custom font encoding is unsupported"));
            }
            dictionary.insert("Encoding", encoding.clone());
        }
        Ok(dictionary)
    }

    fn flush_text(&mut self, id: usize, subset: Option<usize>, at: Transform, bytes: &mut Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.write_text(id, subset, at, false, std::mem::take(bytes));
    }

    fn write_text(
        &mut self,
        id: usize,
        subset: Option<usize>,
        at: Transform,
        positioned: bool,
        bytes: Vec<u8>,
    ) {
        if let fonts::Kind::Standard(font) = &mut self.fonts[id].kind {
            font.used = true;
        }
        self.out.push_str("q\n");
        self.out.push_str(&line(b"cm", &self.state.ctm.to_array()));
        self.effects.emit(&self.state.effects, &mut self.out);
        self.out.push_str(&self.state.fill.command);
        if matches!(&self.fonts[id].kind, fonts::Kind::Type3(_)) {
            self.out.push_str(&self.state.stroke.command);
        }
        self.out.push_str("BT\n");
        let resource = fonts::resource_name(id, subset);
        let size = if positioned { 1.0 } else { self.state.size };
        writeln!(self.out, "/{resource} {} Tf", pdf_number(size)).unwrap();
        for (op, value) in [
            (b"Tc", if positioned { 0.0 } else { self.state.spacing }),
            (
                b"Tw",
                if subset.is_some() {
                    0.0
                } else {
                    self.state.word
                },
            ),
            (b"Tz", if positioned { 100.0 } else { self.state.scale }),
            (b"Ts", if positioned { 0.0 } else { self.state.rise }),
        ] {
            self.out.push_str(&line(op, &[value]));
        }
        self.out.push_str(&line(b"Tm", &at.to_array()));
        writeln!(self.out, "{} Tj\nET\nQ", PdfString { bytes, hex: true }).unwrap();
    }

    fn show_text(&mut self, value: &Object) -> Result<()> {
        let source = value
            .as_string()
            .ok_or_else(|| unsupported("Invalid text string"))?;
        let id = self
            .state
            .font
            .ok_or_else(|| unsupported("Text has no font"))?;
        let source_type3 = matches!(&self.fonts[id].kind, fonts::Kind::Type3(_));
        let (embedded, stride) = match &self.fonts[id].kind {
            fonts::Kind::Standard(_) => (false, 1),
            fonts::Kind::Embedded(font) | fonts::Kind::Type3(font) => {
                (true, if font.two_byte { 2 } else { 1 })
            }
        };
        let font = self.fonts[id].font.clone();
        let glyphs = font.decode(&source.bytes);
        if glyphs.len() * stride != source.bytes.len() {
            return Err(unsupported("Text contains an incomplete character code"));
        }
        let mut retained = Vec::new();
        let mut run_at = self.tm;
        let mut run_subset = None;
        for (glyph, bytes) in glyphs.into_iter().zip(source.bytes.chunks_exact(stride)) {
            let code = bytes
                .iter()
                .fold(0, |code, &byte| (code << 8) | u32::from(byte));
            if glyph.code != code || !glyph.width.is_finite() {
                return Err(unsupported("A character's encoding cannot be verified"));
            }
            let size = self.state.size;
            let scale = self.state.scale / 100.0;
            let mut glyph_matrix =
                Transform::new(size * scale, 0.0, 0.0, size, 0.0, self.state.rise);
            let (cell, width) = if let Some((advance, vx, vy)) = glyph.vertical {
                glyph_matrix =
                    glyph_matrix.then(&Transform::translate(-vx * size * scale, -vy * size));
                (
                    Rect {
                        x0: -0.5,
                        y0: advance,
                        x1: 0.5,
                        y1: 0.0,
                    },
                    -advance,
                )
            } else {
                (
                    Rect {
                        x0: 0.0,
                        y0: -0.2,
                        x1: glyph.width,
                        y1: 0.8,
                    },
                    glyph.width,
                )
            };
            let placed = glyph_matrix.then(&self.tm);
            let matrix = placed.then(&self.state.ctm);
            if matrix.invert().is_none() {
                return Err(unsupported(
                    "Text with a degenerate transform is unsupported",
                ));
            }
            let mut bounds = None;
            // Include the selectable character cell as well as its painted outline.
            add_box(&mut bounds, matrix, cell)?;
            let drawing = if source_type3 {
                let procedure = match self.fonts[id].procedures.entry(code) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let procedure = type3::procedure(
                            self.doc,
                            &font,
                            code,
                            glyph.width,
                            &self.page.resources,
                        )?;
                        self.procedure_bytes += procedure.content.len();
                        if self.procedure_bytes > 16 * 1024 * 1024 {
                            return Err(unsupported(
                                "Type 3 procedures exceed the page export limit",
                            ));
                        }
                        entry.insert(Arc::new(procedure)).clone()
                    }
                };
                if let Some(painted) = procedure.bounds {
                    add_box(&mut bounds, matrix, painted)?;
                }
                Some(fonts::Drawing::Procedure(procedure))
            } else if let Some(outline) = glyph.gid.and_then(|gid| font.outline(gid)) {
                if let Some((min, max)) = outline.transform(&matrix).bounds() {
                    add_point(&mut bounds, min)?;
                    add_point(&mut bounds, max)?;
                }
                let outline = if glyph.vertical.is_some() {
                    Arc::new(outline.transform(&VERTICAL_GLYPH))
                } else {
                    outline
                };
                Some(fonts::Drawing::Outline(outline))
            } else if embedded || !glyph.is_space {
                return Err(unsupported("A character has no verifiable outline"));
            } else {
                None
            };
            let bounds = bounds.unwrap();
            if self
                .regions
                .iter()
                .any(|&region| intersects(region, bounds))
                || !intersects(self.page.crop_box, bounds)
            {
                self.flush_text(id, run_subset, run_at, &mut retained);
            } else {
                let unicode = glyph
                    .unicode
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| unsupported("A retained character has no Unicode mapping"))?;
                let (subset, output_code) = match &mut self.fonts[id].kind {
                    fonts::Kind::Standard(_) => (None, bytes[0]),
                    fonts::Kind::Embedded(font) | fonts::Kind::Type3(font) => {
                        let (subset, code) = font.retain(code, unicode, width, drawing.unwrap())?;
                        (Some(subset), code)
                    }
                };
                if glyph.vertical.is_some() {
                    let output_matrix = VERTICAL_GLYPH.invert().unwrap().then(&placed);
                    self.write_text(id, subset, output_matrix, true, vec![output_code]);
                } else {
                    if run_subset != subset {
                        self.flush_text(id, run_subset, run_at, &mut retained);
                        run_subset = subset;
                    }
                    if retained.is_empty() {
                        run_at = self.tm;
                    }
                    retained.push(output_code);
                    if embedded && glyph.is_space && self.state.word != 0.0 {
                        self.flush_text(id, run_subset, run_at, &mut retained);
                    }
                }
            }
            let spacing = self.state.spacing + if glyph.is_space { self.state.word } else { 0.0 };
            let step = if let Some((advance, _, _)) = glyph.vertical {
                Transform::translate(0.0, advance * size + spacing)
            } else {
                Transform::translate((glyph.width * size + spacing) * scale, 0.0)
            };
            self.tm = step.then(&self.tm);
        }
        self.flush_text(id, run_subset, run_at, &mut retained);
        Ok(())
    }

    fn shift_line(&mut self, x: f64, y: f64) {
        self.line_matrix = Transform::translate(x, y).then(&self.line_matrix);
        self.tm = self.line_matrix;
    }

    fn text(&mut self, op: &[u8], args: &[Object]) -> Result<()> {
        match op {
            b"ET" => {
                nums::<0>(args)?;
                self.in_text = false;
            }
            b"Tf" => self.select_font(args)?,
            b"Tm" => {
                self.tm = Transform::from_array(nums(args)?);
                self.line_matrix = self.tm;
            }
            b"Td" | b"TD" => {
                let [x, y] = nums(args)?;
                if op == b"TD" {
                    self.state.leading = -y;
                }
                self.shift_line(x, y);
            }
            b"T*" => {
                nums::<0>(args)?;
                self.shift_line(0.0, -self.state.leading);
            }
            b"Tc" => [self.state.spacing] = nums(args)?,
            b"Tw" => [self.state.word] = nums(args)?,
            b"Tz" => {
                let [scale] = nums(args)?;
                if scale <= 0.0 {
                    return Err(unsupported("Unsupported text scaling"));
                }
                self.state.scale = scale;
            }
            b"TL" => [self.state.leading] = nums(args)?,
            b"Ts" => [self.state.rise] = nums(args)?,
            b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K" | b"cs" | b"CS" | b"sc" | b"SC" | b"scn"
            | b"SCN" => self.set_color(op, args)?,
            b"gs" => {
                transparency::Effects::apply(&mut self.state, self.doc, &self.page.resources, args)?
            }
            b"Tr" if nums::<1>(args)? == [0.0] => {}
            b"Tj" | b"'" => {
                let [value] = args else {
                    return Err(unsupported("Invalid text operation"));
                };
                if op == b"'" {
                    self.shift_line(0.0, -self.state.leading);
                }
                self.show_text(value)?;
            }
            b"TJ" => {
                let [Object::Array(items)] = args else {
                    return Err(unsupported("Invalid text adjustment"));
                };
                for item in items {
                    if matches!(item, Object::String(_)) {
                        self.show_text(item)?;
                    } else {
                        let delta = -number(item)? * self.state.size / 1000.0;
                        let vertical = self
                            .state
                            .font
                            .is_some_and(|id| self.fonts[id].font.is_vertical());
                        let step = if vertical {
                            Transform::translate(0.0, delta)
                        } else {
                            Transform::translate(delta * self.state.scale / 100.0, 0.0)
                        };
                        self.tm = step.then(&self.tm);
                    }
                }
            }
            _ => {
                return Err(unsupported(format!(
                    "Unsupported text operator {}",
                    String::from_utf8_lossy(op)
                )));
            }
        }
        Ok(())
    }

    fn path_line(&mut self, op: &[u8], values: &[f64]) -> Result<()> {
        if self.path.is_empty() && op != b"m" {
            return Err(unsupported("A vector path has no starting point"));
        }
        self.single_rect = false;
        self.single_line = (self.path_steps == 0 && op == b"m")
            || (self.single_line && self.path_steps == 1 && op == b"l");
        self.path_steps += 1;
        for pair in values.chunks_exact(2) {
            let (x, y) = (pair[0], pair[1]);
            add_point(
                &mut self.path_bounds,
                self.state.ctm.apply(Point::new(x, y)),
            )?;
        }
        self.path.push_str(&line(op, values));
        Ok(())
    }

    fn paint(&mut self, op: &[u8]) -> Result<()> {
        let path = std::mem::take(&mut self.path);
        let single_rect = std::mem::take(&mut self.single_rect);
        let single_line = std::mem::take(&mut self.single_line) && self.path_steps == 2;
        self.path_steps = 0;
        let Some(mut bounds) = self.path_bounds.take() else {
            return Ok(());
        };
        let geometry = bounds;
        let strokes = matches!(op, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
        let t = self.state.ctm;
        let axis_aligned = (t.b == 0.0 && t.c == 0.0) || (t.a == 0.0 && t.d == 0.0);
        let straight = single_line
            && axis_aligned
            && op == b"S"
            && (bounds.width() == 0.0 || bounds.height() == 0.0);
        let mut hollow = None;
        if strokes {
            if self.state.width <= 0.0 {
                return Err(unsupported("Hairline strokes cannot be bounded reliably"));
            }
            let rule_box = single_rect && axis_aligned && matches!(op, b"S" | b"s");
            let half_width = self.state.width * t.max_scale() / 2.0;
            if rule_box
                && geometry.width() > 2.0 * half_width
                && geometry.height() > 2.0 * half_width
            {
                hollow = Some(Rect {
                    x0: geometry.x0 + half_width,
                    y0: geometry.y0 + half_width,
                    x1: geometry.x1 - half_width,
                    y1: geometry.y1 - half_width,
                });
            }
            let reach = if straight || rule_box {
                half_width
            } else {
                self.state.width * t.max_scale() * self.state.miter.max(2.0)
            };
            bounds.x0 -= reach;
            bounds.x1 += reach;
            bounds.y0 -= reach;
            bounds.y1 += reach;
        }
        if self.regions.iter().any(|&region| contains(region, bounds))
            || !intersects(self.page.crop_box, bounds)
        {
            return Ok(());
        }
        if self.regions.iter().any(|&region| {
            intersects(region, bounds)
                && hollow.is_none_or(|hole| {
                    !(region.x0 > hole.x0
                        && region.y0 > hole.y0
                        && region.x1 < hole.x1
                        && region.y1 < hole.y1)
                })
        }) {
            if single_rect && !strokes && axis_aligned {
                if let Some(visible) = bounds.intersect(&self.page.crop_box) {
                    self.out.push_str("q\n");
                    self.effects.emit(&self.state.effects, &mut self.out);
                    self.out.push_str(&self.state.fill.command);
                    for p in cut_rectangle(visible, self.regions)? {
                        self.out
                            .push_str(&line(b"re", &[p.x0, p.y0, p.width(), p.height()]));
                        self.out.push_str("f\n");
                    }
                    self.out.push_str("Q\n");
                }
                return Ok(());
            }
            return Err(unsupported(
                "A vector path crosses a redaction boundary; mark the entire path or use raster export",
            ));
        }
        self.out.push_str("q\n");
        self.out.push_str(&line(b"cm", &self.state.ctm.to_array()));
        self.effects.emit(&self.state.effects, &mut self.out);
        self.out.push_str(&self.state.fill.command);
        if strokes {
            self.out.push_str(&self.state.stroke.command);
            for (op, value) in [
                (b"w", self.state.width),
                (b"J", self.state.cap),
                (b"j", self.state.join),
                (b"M", self.state.miter),
            ] {
                self.out.push_str(&line(op, &[value]));
            }
        }
        self.out.push_str(&path);
        self.out.push_str(&line(op, &[]));
        self.out.push_str("Q\n");
        Ok(())
    }

    fn operation(&mut self, op: &[u8], args: &[Object]) -> Result<()> {
        if self.in_text {
            return self.text(op, args);
        }
        if !self.path.is_empty()
            && !matches!(
                op,
                b"m" | b"l"
                    | b"re"
                    | b"c"
                    | b"v"
                    | b"y"
                    | b"h"
                    | b"S"
                    | b"s"
                    | b"f"
                    | b"F"
                    | b"f*"
                    | b"B"
                    | b"B*"
                    | b"b"
                    | b"b*"
                    | b"n"
            )
        {
            return Err(unsupported(
                "Unsupported graphics state change while constructing a path",
            ));
        }
        match op {
            b"q" if self.stack.len() < 64 => {
                nums::<0>(args)?;
                self.stack.push(self.state.clone());
            }
            b"Q" => {
                nums::<0>(args)?;
                self.state = self
                    .stack
                    .pop()
                    .ok_or_else(|| unsupported("Unbalanced graphics state"))?;
            }
            b"cm" => {
                let ctm = Transform::from_array(nums(args)?).then(&self.state.ctm);
                if !ctm.is_finite()
                    || ctm.invert().is_none()
                    || ctm.to_array().iter().any(|n| n.abs() > 1.0e8)
                {
                    return Err(unsupported("Unusable graphics transform"));
                }
                self.state.ctm = ctm;
            }
            b"BT" => {
                nums::<0>(args)?;
                self.in_text = true;
                self.tm = Transform::IDENTITY;
                self.line_matrix = self.tm;
            }
            b"w" => [self.state.width] = nums(args)?,
            b"J" => [self.state.cap] = nums(args)?,
            b"j" => [self.state.join] = nums(args)?,
            b"M" => [self.state.miter] = nums(args)?,
            b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K" | b"cs" | b"CS" | b"sc" | b"SC" | b"scn"
            | b"SCN" => self.set_color(op, args)?,
            b"gs" => {
                transparency::Effects::apply(&mut self.state, self.doc, &self.page.resources, args)?
            }
            b"Do" => self.image_xobject(args)?,
            b"m" | b"l" => self.path_line(op, &nums::<2>(args)?)?,
            b"c" => self.path_line(op, &nums::<6>(args)?)?,
            b"v" | b"y" => self.path_line(op, &nums::<4>(args)?)?,
            b"h" => self.path_line(op, &nums::<0>(args)?)?,
            b"re" => {
                self.single_rect = self.path.is_empty();
                self.single_line = false;
                self.path_steps += 1;
                let [x, y, w, h] = nums(args)?;
                add_box(
                    &mut self.path_bounds,
                    self.state.ctm,
                    Rect {
                        x0: x,
                        y0: y,
                        x1: x + w,
                        y1: y + h,
                    },
                )?;
                self.path.push_str(&line(op, &[x, y, w, h]));
            }
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                nums::<0>(args)?;
                self.paint(op)?;
            }
            b"n" => {
                nums::<0>(args)?;
                self.path.clear();
                self.path_bounds = None;
                self.single_rect = false;
                self.path_steps = 0;
                self.single_line = false;
            }
            _ => {
                return Err(unsupported(format!(
                    "Unsupported page operator {}",
                    String::from_utf8_lossy(op)
                )));
            }
        }
        Ok(())
    }

    fn set_color(&mut self, op: &[u8], args: &[Object]) -> Result<()> {
        let stroke = op[0].is_ascii_uppercase();
        let paint = if stroke {
            &mut self.state.stroke
        } else {
            &mut self.state.fill
        };
        *paint = match op {
            b"cs" | b"CS" => color::Paint::named(self.doc, &self.page.resources, args, stroke)?,
            b"sc" | b"SC" | b"scn" | b"SCN" => paint.operands(args, stroke)?,
            _ => color::Paint::device(self.doc, &self.page.resources, op, args)?,
        };
        Ok(())
    }

    fn image_xobject(&mut self, args: &[Object]) -> Result<()> {
        let [Object::Name(key)] = args else {
            return Err(unsupported("Invalid external object operation"));
        };
        let Object::Dict(objects) = self.doc.get_key(&self.page.resources, b"XObject")? else {
            return Err(unsupported("Missing external object dictionary"));
        };
        let Object::Stream(image) = self.doc.get_key(&objects, key.as_bytes())? else {
            return Err(unsupported("Missing external object stream"));
        };
        if image
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .is_none_or(|name| name.as_bytes() != b"Image")
        {
            return Err(unsupported(
                "Unsupported external object in vector redaction",
            ));
        }
        self.rewrite_image(&image)
    }

    fn inline_image(&mut self, args: &[Object], parser: &mut Parser<'_>) -> Result<()> {
        if !args.is_empty() || self.in_text || !self.path.is_empty() {
            return Err(unsupported("Invalid inline image position"));
        }
        let (short, data) = read_inline_image(parser)?;
        let mut dict = Dict::new();
        for (key, value) in short.iter() {
            let key = match key.as_bytes() {
                b"BPC" => Name::from("BitsPerComponent"),
                b"CS" => Name::from("ColorSpace"),
                b"D" => Name::from("Decode"),
                b"DP" => Name::from("DecodeParms"),
                b"F" => Name::from("Filter"),
                b"H" => Name::from("Height"),
                b"IM" => Name::from("ImageMask"),
                b"I" => Name::from("Interpolate"),
                b"W" => Name::from("Width"),
                _ => key.clone(),
            };
            if dict.contains_key(key.as_bytes()) {
                return Err(unsupported("Ambiguous inline image dictionary"));
            }
            dict.insert(key, value.clone());
        }
        self.rewrite_image(&Stream {
            dict,
            data: data.into(),
        })
    }

    fn rewrite_image(&mut self, image: &Stream) -> Result<()> {
        let mut bounds = None;
        add_box(
            &mut bounds,
            self.state.ctm,
            Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            },
        )?;
        let bounds = bounds.unwrap();
        if self.regions.iter().any(|&region| contains(region, bounds))
            || !intersects(self.page.crop_box, bounds)
        {
            return Ok(());
        }
        let image = images::rewrite(
            self.doc,
            self.page,
            image,
            self.state.ctm,
            self.state.fill.rgb,
            self.regions
                .iter()
                .copied()
                .filter(|&r| intersects(r, bounds)),
            &mut self.assets,
        )?;
        let name = format!("I{}", self.images.len());
        self.images.insert(Name::from(name.as_str()), image);
        self.out.push_str("q\n");
        self.out.push_str(&line(b"cm", &self.state.ctm.to_array()));
        self.effects.emit(&self.state.effects, &mut self.out);
        writeln!(self.out, "/{name} Do\nQ").unwrap();
        Ok(())
    }
}

/// Rewrites supported operations and adds generated resources to a fresh PDF.
/// Discard the destination update if rewriting fails.
pub fn redact_page_content(
    doc: &Document,
    page: &Page,
    regions: &[Rect],
    output: &mut Update<'_>,
) -> Result<RedactedPage> {
    let group = transparency::page_group(doc, page)?;
    let user_unit = transparency::user_unit(doc, page)?;
    let content = page.contents(doc)?;
    if content.len() > 16 * 1024 * 1024 {
        return Err(unsupported(
            "Page content exceeds the vector redaction limit",
        ));
    }
    let mut state = Rewriter {
        doc,
        page,
        regions,
        state: State::new(doc, &page.resources)?,
        stack: Vec::new(),
        path: String::new(),
        path_bounds: None,
        single_rect: false,
        path_steps: 0,
        single_line: false,
        in_text: false,
        tm: Transform::IDENTITY,
        line_matrix: Transform::IDENTITY,
        fonts: Vec::new(),
        font_map: HashMap::new(),
        images: Dict::new(),
        effects: transparency::UsedEffects::default(),
        procedure_bytes: 0,
        assets: Assets {
            output,
            streams: Vec::new(),
            bytes: 0,
        },
        out: String::new(),
    };
    let mut parser = Parser::new(&content);
    while let Some((args, op)) = read_operation(&mut parser)? {
        if op == b"BI" {
            state.inline_image(&args, &mut parser)?;
        } else {
            state.operation(op, &args)?;
        }
        if state.out.len() > 32 * 1024 * 1024 {
            return Err(unsupported(
                "Rewritten page exceeds the vector redaction limit",
            ));
        }
    }
    if state.in_text || !state.stack.is_empty() || !state.path.is_empty() {
        return Err(unsupported("Unbalanced page content"));
    }
    for region in regions {
        state.out.push_str("q\n0 g\n");
        state.out.push_str(&line(
            b"re",
            &[region.x0, region.y0, region.width(), region.height()],
        ));
        state.out.push_str("f\nQ\n");
    }
    let mut resources = Dict::new();
    let mut fonts = Dict::new();
    for (id, font) in state.fonts.into_iter().enumerate() {
        fonts::write(id, font.kind, &mut fonts, &mut state.assets)?;
    }
    if !fonts.is_empty() {
        resources.insert("Font", Object::Dict(fonts));
    }
    if !state.images.is_empty() {
        resources.insert("XObject", Object::Dict(state.images));
    }
    state.effects.write(&mut resources);
    Ok(RedactedPage {
        content: state.out.into_bytes(),
        resources,
        asset_streams: state.assets.streams,
        group,
        user_unit,
    })
}
