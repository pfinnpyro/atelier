//! The content stream interpreter (ISO 32000-2, chapter 8).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use atelier_raster::{
    BlendMode, Color, Coverage, FillRule, ImageSampling, LineCap, LineJoin, Mask, Path,
    PathBuilder, Pixmap, Point, StrokeStyle, Transform, draw_image, fill_path, rasterize_in,
    stroke_to_path,
};
use atelier_syntax::{
    ContentParser, Dict, Document, Name, ObjRef, Object, Operation, Page, Rect, Result, Stream,
};

use crate::colorspace::{self, ColorSpace};
use crate::font::PdfFont;
use crate::function::Functions;
use crate::image;
use crate::pattern::{Cell, Pattern, PatternKind};
use crate::probe::{Paint, PaintKind, Probed};
use crate::shading::Shading;
use crate::text::{PageText, TextGlyph};

/// Patterns drawn inside pattern cells deeper than this are skipped.
const MAX_PATTERN_DEPTH: usize = 6;
/// Form XObjects nested deeper than this are skipped.
const MAX_FORM_DEPTH: usize = 32;
/// Hard cap on operators per page, so hostile content cannot hang the viewer.
const MAX_OPERATIONS: usize = 20_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Device pixels per PDF point (72 per inch). 1.0 is 72 dpi.
    pub scale: f64,
    pub background: Color,
    /// Draw annotations and form fields from their appearance streams.
    pub annotations: bool,
    /// Annotations to leave out, such as a form field the viewer is
    /// editing and draws itself.
    pub hidden_annotations: Vec<ObjRef>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 150.0 / 72.0,
            background: Color::WHITE,
            annotations: true,
            hidden_annotations: Vec::new(),
        }
    }
}

pub struct Rendered {
    pub pixmap: Pixmap,
    /// Unsupported features and damage worked around, without duplicates.
    pub warnings: Vec<String>,
}

/// The transform from page space to device pixels and the pixmap size, taking
/// the crop box and /Rotate into account.
pub fn page_transform(page: &Page, scale: f64) -> (Transform, usize, usize) {
    let crop = page.crop_box;
    // Round up, but not for float noise such as 1650.0002.
    let w = (crop.width() * scale - 0.01).ceil().max(1.0);
    let h = (crop.height() * scale - 0.01).ceil().max(1.0);
    // Flip y so the top of the crop box is row 0.
    let base = Transform::new(scale, 0.0, 0.0, -scale, -crop.x0 * scale, crop.y1 * scale);
    let (rot, dw, dh) = match page.rotate {
        90 => (Transform::new(0.0, 1.0, -1.0, 0.0, h, 0.0), h, w),
        180 => (Transform::new(-1.0, 0.0, 0.0, -1.0, w, h), w, h),
        270 => (Transform::new(0.0, -1.0, 1.0, 0.0, 0.0, w), h, w),
        _ => (Transform::IDENTITY, w, h),
    };
    (base.then(&rot), dw as usize, dh as usize)
}

/// Renders one page.
pub fn render_page(doc: &Document, page: &Page, options: &RenderOptions) -> Result<Rendered> {
    let (_, width, height) = page_transform(page, options.scale);
    render_region(doc, page, options, (0, 0, width, height))
}

/// Renders part of a page: the device-pixel rectangle `(x, y, width,
/// height)` of the page as `render_page` would draw it at this scale. The
/// viewer draws pages in tiles this way.
pub fn render_region(
    doc: &Document,
    page: &Page,
    options: &RenderOptions,
    (x, y, width, height): (usize, usize, usize, usize),
) -> Result<Rendered> {
    let (page_ctm, _, _) = page_transform(page, options.scale);
    let ctm = page_ctm.then(&Transform::translate(-(x as f64), -(y as f64)));
    let mut pixmap = Pixmap::filled(width.max(1), height.max(1), options.background);
    let content = page.contents(doc)?;
    let mut interp = Interpreter::new(doc, &mut pixmap, ctm);
    interp.run(&content, &page.resources, 0);
    if options.annotations {
        interp.annotations(page, ctm, &options.hidden_annotations);
    }
    let warnings = interp.warnings;
    Ok(Rendered { pixmap, warnings })
}

/// Picks the colour at device pixel `(x, y)` of the page drawn at
/// `options.scale`: the colour shown there and the paint that put it
/// there. Draws just that one pixel.
pub fn probe_color(
    doc: &Document,
    page: &Page,
    options: &RenderOptions,
    (x, y): (usize, usize),
) -> Result<Probed> {
    let (page_ctm, _, _) = page_transform(page, options.scale);
    let ctm = page_ctm.then(&Transform::translate(-(x as f64), -(y as f64)));
    let mut pixmap = Pixmap::filled(1, 1, options.background);
    let content = page.contents(doc)?;
    let found = Rc::new(RefCell::new(None));
    {
        let mut interp = Interpreter::new(doc, &mut pixmap, ctm);
        interp.probe = Some(found.clone());
        interp.run(&content, &page.resources, 0);
        if options.annotations {
            interp.annotations(page, ctm, &options.hidden_annotations);
        }
    }
    let d = pixmap.data();
    // Premultiplied over an opaque background, so the alpha is full.
    let rgb = [d[0], d[1], d[2]];
    let paint = found.borrow_mut().take();
    Ok(Probed { rgb, paint })
}

/// The text of a page, with each glyph's position in default user space.
/// Runs the page's content without drawing anything.
pub fn extract_text(doc: &Document, page: &Page) -> Result<PageText> {
    let mut pixmap = Pixmap::filled(1, 1, Color::WHITE);
    let content = page.contents(doc)?;
    let mut interp = Interpreter::new(doc, &mut pixmap, Transform::IDENTITY);
    interp.draw = false;
    interp.text = Some(Vec::new());
    interp.run(&content, &page.resources, 0);
    Ok(PageText {
        glyphs: interp.text.unwrap_or_default(),
    })
}

#[derive(Clone)]
struct TextState {
    char_spacing: f64,
    word_spacing: f64,
    /// Horizontal scaling as a factor (Tz / 100).
    scale: f64,
    leading: f64,
    font: Option<Rc<PdfFont>>,
    size: f64,
    render: u8,
    rise: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            font: None,
            size: 0.0,
            render: 0,
            rise: 0.0,
        }
    }
}

/// An annotation's appearance stream for `kind` (N, R or D): the stream
/// itself, or the one its /AS state picks from a dictionary of states.
pub(crate) fn appearance(
    doc: &Document,
    annot: &Dict,
    kind: &[u8],
) -> Option<(Stream, Option<ObjRef>)> {
    let ap = doc.get_key(annot, b"AP").ok()?;
    let entry = ap.as_dict()?.get(kind)?.clone();
    let resolved = doc.resolve(&entry).ok()?;
    match resolved {
        Object::Stream(s) => Some((s, entry.as_reference())),
        Object::Dict(states) => {
            let state = doc.get_key(annot, b"AS").ok()?;
            let entry = states.get(state.as_name()?.as_bytes())?.clone();
            match doc.resolve(&entry).ok()? {
                Object::Stream(s) => Some((s, entry.as_reference())),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A device rectangle `(x, y, width, height)` in pixels.
type Area = (usize, usize, usize, usize);

fn intersect_area(a: Area, b: Area) -> Area {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// The pixels a device-space path can touch, within a `w` x `h` pixmap.
fn path_area(path: &Path, (w, h): (usize, usize)) -> Area {
    let Some((min, max)) = path.bounds() else {
        return (0, 0, 0, 0);
    };
    if ![min.x, min.y, max.x, max.y].iter().all(|v| v.is_finite()) {
        return (0, 0, 0, 0);
    }
    let x0 = min.x.floor().clamp(0.0, w as f64) as usize;
    let y0 = min.y.floor().clamp(0.0, h as f64) as usize;
    let x1 = max.x.ceil().clamp(0.0, w as f64) as usize;
    let y1 = max.y.ceil().clamp(0.0, h as f64) as usize;
    (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// The clipping path: every clip set so far, each in device space. The
/// mask is worked out only for the area something is drawn in, so a big
/// clip around a small drawing costs only the small area.
struct Clip {
    parent: Option<Rc<Clip>>,
    path: Path,
    rule: FillRule,
    /// Where this clip and all its parents can let anything through.
    area: Area,
}

impl Clip {
    /// The clip mask over `area` (zero elsewhere).
    fn mask(&self, area: Area, size: (usize, usize)) -> Mask {
        let area = intersect_area(area, self.area);
        if area.2 == 0 || area.3 == 0 {
            return Mask::new(size.0, size.1);
        }
        let mut mask: Option<Mask> = None;
        let mut node = Some(self);
        while let Some(c) = node {
            let coverage = rasterize_in(&c.path, c.rule, area);
            match &mut mask {
                None => mask = Some(Mask::from_coverage(coverage.as_ref(), size.0, size.1)),
                Some(m) => m.intersect(coverage.as_ref()),
            }
            node = c.parent.as_deref();
        }
        mask.unwrap_or_else(|| Mask::new(size.0, size.1))
    }
}

#[derive(Clone)]
struct GraphicsState {
    ctm: Transform,
    text: TextState,
    clip: Option<Rc<Clip>>,
    stroke: StrokeStyle,
    fill_space: ColorSpace,
    fill: Vec<f32>,
    stroke_space: ColorSpace,
    stroke_color: Vec<f32>,
    fill_alpha: f32,
    stroke_alpha: f32,
    fill_pattern: Option<Rc<Pattern>>,
    stroke_pattern: Option<Rc<Pattern>>,
    /// Inside an uncoloured tiling pattern's cell every colour is this one.
    forced: Option<Color>,
    blend: BlendMode,
    /// The soft mask from the ExtGState, in device pixels.
    soft_mask: Option<Rc<Coverage>>,
}

impl GraphicsState {
    fn new(ctm: Transform) -> Self {
        Self {
            ctm,
            text: TextState::default(),
            clip: None,
            stroke: StrokeStyle::default(),
            fill_space: ColorSpace::Gray,
            fill: vec![0.0],
            stroke_space: ColorSpace::Gray,
            stroke_color: vec![0.0],
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            fill_pattern: None,
            stroke_pattern: None,
            forced: None,
            blend: BlendMode::Normal,
            soft_mask: None,
        }
    }

    fn fill_color(&self) -> Option<Color> {
        self.forced
            .or_else(|| self.fill_space.to_rgb(&self.fill))
            .map(|c| c.with_alpha(self.fill_alpha))
    }

    fn stroke_paint(&self) -> Option<Color> {
        self.forced
            .or_else(|| self.stroke_space.to_rgb(&self.stroke_color))
            .map(|c| c.with_alpha(self.stroke_alpha))
    }
}

struct Interpreter<'a> {
    doc: &'a Document,
    pixmap: &'a mut Pixmap,
    gs: GraphicsState,
    stack: Vec<GraphicsState>,
    path: PathBuilder,
    pending_clip: Option<FillRule>,
    forms: Vec<ObjRef>,
    operations: usize,
    warnings: Vec<String>,
    seen: HashSet<String>,
    /// Text matrix and text line matrix.
    tm: Transform,
    tlm: Transform,
    /// Device-space outlines of glyphs shown with a clipping render mode in
    /// the current text object.
    text_clip: Option<PathBuilder>,
    fonts: HashMap<ObjRef, Rc<PdfFont>>,
    /// False when only collecting text.
    draw: bool,
    /// Collected glyphs, when extracting text.
    text: Option<Vec<TextGlyph>>,
    /// The default space of the content stream being run: the page's, or a
    /// form's. Patterns are placed relative to it.
    base_ctm: Transform,
    /// How many pattern cells deep this interpreter is.
    pattern_depth: usize,
    /// Soft masks already rendered, by mask group and transform.
    soft_masks: HashMap<(ObjRef, [u64; 6]), Rc<Coverage>>,
    /// What DeviceGray, DeviceRGB and DeviceCMYK mean in the content being
    /// run (Default spaces, an output intent).
    devices: [ColorSpace; 3],
    /// When picking a colour: the last paint that changed the (single)
    /// pixel being drawn.
    pub(crate) probe: Option<Rc<RefCell<Option<Paint>>>>,
    /// Glyph outlines are being painted, so a fill counts as text.
    painting_text: bool,
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().filter_map(Object::as_f64).collect()
}

impl<'a> Interpreter<'a> {
    fn new(doc: &'a Document, pixmap: &'a mut Pixmap, ctm: Transform) -> Self {
        Self {
            doc,
            pixmap,
            gs: GraphicsState::new(ctm),
            stack: Vec::new(),
            path: PathBuilder::new(),
            pending_clip: None,
            forms: Vec::new(),
            operations: 0,
            warnings: Vec::new(),
            seen: HashSet::new(),
            tm: Transform::IDENTITY,
            tlm: Transform::IDENTITY,
            text_clip: None,
            fonts: HashMap::new(),
            draw: true,
            text: None,
            base_ctm: ctm,
            pattern_depth: 0,
            soft_masks: HashMap::new(),
            devices: [ColorSpace::Gray, ColorSpace::Rgb, ColorSpace::Cmyk],
            probe: None,
            painting_text: false,
        }
    }

    /// Draws the page's annotations from their normal appearance streams
    /// (ISO 32000-2, 12.5.5): each appearance's bounding box, after its
    /// matrix, is fitted to the annotation's rectangle.
    fn annotations(&mut self, page: &Page, page_ctm: Transform, hidden: &[ObjRef]) {
        let doc = self.doc;
        let Ok(Object::Array(annots)) = doc.get_key(&page.dict, b"Annots") else {
            return;
        };
        for entry in annots.iter().take(10_000) {
            if entry.as_reference().is_some_and(|r| hidden.contains(&r)) {
                continue;
            }
            let Ok(Object::Dict(annot)) = doc.resolve(entry) else {
                continue;
            };
            let Some((stream, r)) = appearance(doc, &annot, b"N") else {
                continue;
            };
            let flags = doc
                .get_key(&annot, b"F")
                .ok()
                .and_then(|f| f.as_i64())
                .unwrap_or(0);
            // Hidden (2) and NoView (32); popups only show when opened.
            if flags & (2 | 32) != 0
                || doc
                    .get_key(&annot, b"Subtype")
                    .ok()
                    .is_some_and(|s| s.as_name().is_some_and(|n| *n == b"Popup"))
            {
                continue;
            }
            let Some(rect) = doc
                .get_key(&annot, b"Rect")
                .ok()
                .and_then(|r| Rect::from_array(r.as_array()?))
            else {
                continue;
            };
            let (matrix, _) = self.form_geometry(&stream);
            let Some(bbox) = doc
                .get_key(&stream.dict, b"BBox")
                .ok()
                .and_then(|b| Rect::from_array(b.as_array()?))
            else {
                continue;
            };
            // The box after the form matrix, then the map onto the rectangle.
            let corners = [
                (bbox.x0, bbox.y0),
                (bbox.x1, bbox.y0),
                (bbox.x0, bbox.y1),
                (bbox.x1, bbox.y1),
            ]
            .map(|(x, y)| matrix.apply(Point::new(x, y)));
            let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
            let max_x = corners
                .iter()
                .map(|p| p.x)
                .fold(f64::NEG_INFINITY, f64::max);
            let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
            let max_y = corners
                .iter()
                .map(|p| p.y)
                .fold(f64::NEG_INFINITY, f64::max);
            let (w, h) = (max_x - min_x, max_y - min_y);
            if !(w > 1e-9 && h > 1e-9) {
                continue;
            }
            let (sx, sy) = (rect.width() / w, rect.height() / h);
            let fit = Transform::new(sx, 0.0, 0.0, sy, rect.x0 - min_x * sx, rect.y0 - min_y * sy);
            self.gs = GraphicsState::new(fit.then(&page_ctm));
            self.stack.clear();
            self.path = PathBuilder::new();
            self.text_clip = None;
            self.form(&stream, r, &page.resources, 0);
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.seen.insert(message.clone()) {
            self.warnings.push(message);
        }
    }

    fn run(&mut self, content: &[u8], resources: &Dict, depth: usize) {
        let devices = [1, 3, 4].map(|n| colorspace::device(self.doc, resources, n));
        if self.operations == 0 && self.gs.fill_space == ColorSpace::Gray {
            // The page starts out painting black in DeviceGray.
            self.gs.fill_space = devices[0].clone();
            self.gs.stroke_space = devices[0].clone();
        }
        let outer = std::mem::replace(&mut self.devices, devices);
        self.run_with(content, resources, depth);
        self.devices = outer;
    }

    fn run_with(&mut self, content: &[u8], resources: &Dict, depth: usize) {
        let base_depth = self.stack.len();
        for op in ContentParser::new(content) {
            self.operations += 1;
            if self.operations > MAX_OPERATIONS {
                self.warn("too many operators; stopped drawing");
                break;
            }
            self.operation(&op, resources, depth);
        }
        // Unbalanced q inside a stream must not leak out of it.
        while self.stack.len() > base_depth {
            if let Some(gs) = self.stack.pop() {
                self.gs = gs;
            }
        }
    }

    fn operation(&mut self, op: &Operation, resources: &Dict, depth: usize) {
        let o = &op.operands;
        let n = nums(o);
        let p = |i: usize| Point::new(n[i], n[i + 1]);
        self.pixmap.set_blend(self.gs.blend);
        match op.operator {
            // Graphics state
            b"q" => {
                if self.stack.len() < 4096 {
                    self.stack.push(self.gs.clone());
                }
            }
            b"Q" => {
                if let Some(gs) = self.stack.pop() {
                    self.gs = gs;
                }
            }
            b"cm" if n.len() >= 6 => {
                let m = Transform::from_array([n[0], n[1], n[2], n[3], n[4], n[5]]);
                if m.is_finite() {
                    self.gs.ctm = m.then(&self.gs.ctm);
                }
            }
            b"w" if !n.is_empty() => self.gs.stroke.width = n[0].abs(),
            b"J" if !n.is_empty() => self.gs.stroke.cap = cap(n[0]),
            b"j" if !n.is_empty() => self.gs.stroke.join = join(n[0]),
            b"M" if !n.is_empty() => self.gs.stroke.miter_limit = n[0].max(1.0),
            b"d" => {
                if let (Some(Object::Array(a)), Some(phase)) =
                    (o.first(), o.get(1).and_then(Object::as_f64))
                {
                    self.gs.stroke.dash = a.iter().filter_map(Object::as_f64).collect();
                    self.gs.stroke.dash_phase = phase;
                }
            }
            b"gs" => {
                if let Some(name) = o.first().and_then(Object::as_name) {
                    self.ext_gstate(name, resources);
                }
            }
            b"ri" | b"i" => {}

            // Path construction
            b"m" if n.len() >= 2 => self.path.move_to(p(0)),
            b"l" if n.len() >= 2 => self.path.line_to(p(0)),
            b"c" if n.len() >= 6 => self.path.cubic_to(p(0), p(2), p(4)),
            b"v" if n.len() >= 4 => {
                let c1 = self.path.current_point().unwrap_or(p(0));
                self.path.cubic_to(c1, p(0), p(2));
            }
            b"y" if n.len() >= 4 => self.path.cubic_to(p(0), p(2), p(2)),
            b"h" => self.path.close(),
            b"re" if n.len() >= 4 => {
                let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
                self.path.move_to(Point::new(x, y));
                self.path.line_to(Point::new(x + w, y));
                self.path.line_to(Point::new(x + w, y + h));
                self.path.line_to(Point::new(x, y + h));
                self.path.close();
            }

            // Path painting
            b"S" => self.paint(None, true),
            b"s" => {
                self.path.close();
                self.paint(None, true);
            }
            b"f" | b"F" => self.paint(Some(FillRule::NonZero), false),
            b"f*" => self.paint(Some(FillRule::EvenOdd), false),
            b"B" => self.paint(Some(FillRule::NonZero), true),
            b"B*" => self.paint(Some(FillRule::EvenOdd), true),
            b"b" => {
                self.path.close();
                self.paint(Some(FillRule::NonZero), true);
            }
            b"b*" => {
                self.path.close();
                self.paint(Some(FillRule::EvenOdd), true);
            }
            b"n" => self.paint(None, false),
            b"W" => self.pending_clip = Some(FillRule::NonZero),
            b"W*" => self.pending_clip = Some(FillRule::EvenOdd),

            // Colour
            b"CS" | b"cs" => {
                let space = o
                    .first()
                    .and_then(|obj| colorspace::parse(self.doc, obj, resources))
                    .unwrap_or(ColorSpace::Gray);
                if matches!(
                    space,
                    ColorSpace::Spot {
                        tint: None,
                        none: false,
                        ..
                    }
                ) {
                    self.warn("a Separation or DeviceN colour has no usable tint transform");
                }
                let initial = space.initial();
                if op.operator == b"CS" {
                    self.gs.stroke_space = space;
                    self.gs.stroke_color = initial;
                } else {
                    self.gs.fill_space = space;
                    self.gs.fill = initial;
                }
            }
            b"SC" | b"SCN" => {
                self.gs.stroke_color = n.iter().map(|&v| v as f32).collect();
                if matches!(self.gs.stroke_space, ColorSpace::Pattern { .. }) {
                    self.gs.stroke_pattern = self.set_pattern(o, resources, true);
                }
            }
            b"sc" | b"scn" => {
                self.gs.fill = n.iter().map(|&v| v as f32).collect();
                if matches!(self.gs.fill_space, ColorSpace::Pattern { .. }) {
                    self.gs.fill_pattern = self.set_pattern(o, resources, false);
                }
            }
            b"G" if !n.is_empty() => self.set_stroke(self.devices[0].clone(), &n[..1]),
            b"g" if !n.is_empty() => self.set_fill(self.devices[0].clone(), &n[..1]),
            b"RG" if n.len() >= 3 => self.set_stroke(self.devices[1].clone(), &n[..3]),
            b"rg" if n.len() >= 3 => self.set_fill(self.devices[1].clone(), &n[..3]),
            b"K" if n.len() >= 4 => self.set_stroke(self.devices[2].clone(), &n[..4]),
            b"k" if n.len() >= 4 => self.set_fill(self.devices[2].clone(), &n[..4]),

            // XObjects and images
            b"Do" => {
                if let Some(name) = o.first().and_then(Object::as_name) {
                    self.xobject(name, resources, depth);
                }
            }
            b"BI" => {
                if let Some((dict, data)) = &op.inline_image {
                    self.inline_image(dict, data, resources);
                }
            }

            b"sh" => {
                if let Some(name) = o.first().and_then(Object::as_name) {
                    self.shade(name, resources);
                }
            }

            // Text
            b"BT" => {
                self.tm = Transform::IDENTITY;
                self.tlm = Transform::IDENTITY;
                self.text_clip = None;
            }
            b"ET" => {
                if let Some(mut clip) = self.text_clip.take() {
                    self.clip_device(&clip.finish(), FillRule::NonZero);
                }
            }
            b"Tc" if !n.is_empty() => self.gs.text.char_spacing = n[0],
            b"Tw" if !n.is_empty() => self.gs.text.word_spacing = n[0],
            b"Tz" if !n.is_empty() => self.gs.text.scale = n[0] / 100.0,
            b"TL" if !n.is_empty() => self.gs.text.leading = n[0],
            b"Ts" if !n.is_empty() => self.gs.text.rise = n[0],
            b"Tr" if !n.is_empty() => self.gs.text.render = (n[0] as i64).clamp(0, 7) as u8,
            b"Tf" => {
                if let (Some(name), Some(size)) = (
                    o.first().and_then(Object::as_name),
                    o.get(1).and_then(Object::as_f64),
                ) {
                    self.gs.text.font = self.font(name, resources);
                    self.gs.text.size = size;
                }
            }
            b"Td" if n.len() >= 2 => self.next_line(n[0], n[1]),
            b"TD" if n.len() >= 2 => {
                self.gs.text.leading = -n[1];
                self.next_line(n[0], n[1]);
            }
            b"Tm" if n.len() >= 6 => {
                let m = Transform::from_array([n[0], n[1], n[2], n[3], n[4], n[5]]);
                if m.is_finite() {
                    self.tm = m;
                    self.tlm = m;
                }
            }
            b"T*" => self.next_line(0.0, -self.gs.text.leading),
            b"Tj" => {
                if let Some(s) = o.first().and_then(Object::as_string) {
                    self.show(&s.bytes, resources, depth);
                }
            }
            b"'" => {
                self.next_line(0.0, -self.gs.text.leading);
                if let Some(s) = o.first().and_then(Object::as_string) {
                    self.show(&s.bytes, resources, depth);
                }
            }
            b"\"" if o.len() >= 3 => {
                self.gs.text.word_spacing = o[0].as_f64().unwrap_or(0.0);
                self.gs.text.char_spacing = o[1].as_f64().unwrap_or(0.0);
                self.next_line(0.0, -self.gs.text.leading);
                if let Some(s) = o[2].as_string() {
                    self.show(&s.bytes, resources, depth);
                }
            }
            b"TJ" => {
                if let Some(items) = o.first().and_then(Object::as_array) {
                    for item in items {
                        match item {
                            Object::String(s) => self.show(&s.bytes, resources, depth),
                            other => {
                                if let Some(adj) = other.as_f64() {
                                    self.adjust(adj);
                                }
                            }
                        }
                    }
                }
            }
            b"d0" | b"d1" => {}
            _ => {}
        }
    }

    fn set_fill(&mut self, space: ColorSpace, n: &[f64]) {
        self.gs.fill_space = space;
        self.gs.fill = n.iter().map(|&v| v as f32).collect();
    }

    fn set_stroke(&mut self, space: ColorSpace, n: &[f64]) {
        self.gs.stroke_space = space;
        self.gs.stroke_color = n.iter().map(|&v| v as f32).collect();
    }

    /// Paints the current path (fill rule and/or stroke), applies a pending
    /// clip, and starts a new path.
    fn paint(&mut self, fill: Option<FillRule>, stroke: bool) {
        let path = self.path.finish();
        if !path.is_empty() && self.draw {
            if let Some(rule) = fill {
                let device = path.transform(&self.gs.ctm);
                self.paint_device(&device, rule, false);
            }
            if stroke {
                let outline = stroke_to_path(&path, &self.gs.stroke, &self.gs.ctm);
                self.paint_device(&outline, FillRule::NonZero, true);
            }
        }
        if let Some(rule) = self.pending_clip.take() {
            self.clip_device(&path.transform(&self.gs.ctm), rule);
        }
    }

    /// Fills a device-space shape with the current fill (or stroke) paint:
    /// a solid colour or a pattern.
    fn paint_device(&mut self, device: &Path, rule: FillRule, stroke: bool) {
        let before = self.probe_pixel();
        self.paint_device_inner(device, rule, stroke);
        let kind = match (stroke, self.painting_text) {
            (true, _) => PaintKind::Stroke,
            (false, true) => PaintKind::Text,
            (false, false) => PaintKind::Fill,
        };
        self.probe_paint(before, kind, stroke);
    }

    fn paint_device_inner(&mut self, device: &Path, rule: FillRule, stroke: bool) {
        let (is_pattern, pattern, alpha) = if stroke {
            (
                matches!(self.gs.stroke_space, ColorSpace::Pattern { .. }),
                self.gs.stroke_pattern.clone(),
                self.gs.stroke_alpha,
            )
        } else {
            (
                matches!(self.gs.fill_space, ColorSpace::Pattern { .. }),
                self.gs.fill_pattern.clone(),
                self.gs.fill_alpha,
            )
        };
        if is_pattern && self.gs.forced.is_none() {
            if let Some(p) = pattern {
                self.paint_pattern(&p, device, rule, alpha);
            }
            return;
        }
        let color = if stroke {
            self.gs.stroke_paint()
        } else {
            self.gs.fill_color()
        };
        if let Some(color) = color {
            let clip = self.clip_mask(device);
            fill_path(self.pixmap, device, rule, color, clip.as_ref());
        }
    }

    /// Reads the pattern named by the last operand of `scn`/`SCN`.
    fn set_pattern(
        &mut self,
        ops: &[Object],
        resources: &Dict,
        stroke: bool,
    ) -> Option<Rc<Pattern>> {
        let name = ops.last()?.as_name()?;
        let (obj, _) = self.lookup(resources, b"Pattern", name)?;
        let base = self.base_ctm;
        let mut warnings = Vec::new();
        let parsed = Pattern::parse(self.doc, &obj, &base, resources, &mut |w| warnings.push(w));
        for w in warnings {
            self.warn(w);
        }
        let Some(mut pattern) = parsed else {
            self.warn("a pattern could not be read");
            return None;
        };
        if let PatternKind::Tiling(t) = &mut pattern.kind {
            if t.uncolored {
                let space = if stroke {
                    &self.gs.stroke_space
                } else {
                    &self.gs.fill_space
                };
                let comps = if stroke {
                    &self.gs.stroke_color
                } else {
                    &self.gs.fill
                };
                t.color = match space {
                    ColorSpace::Pattern { base: Some(b) } => b.to_rgb(comps),
                    _ => None,
                };
            }
        }
        Some(Rc::new(pattern))
    }

    /// The area drawing can reach: the clip's bounds, or the whole pixmap.
    fn drawable_area(&self) -> Area {
        let size = self.pixmap.bounds();
        let area = self
            .gs
            .clip
            .as_ref()
            .map_or((0, 0, size.0, size.1), |c| c.area);
        match &self.gs.soft_mask {
            Some(m) => intersect_area(area, (m.x, m.y, m.width, m.height)),
            None => area,
        }
    }

    fn paint_pattern(&mut self, pattern: &Pattern, device: &Path, rule: FillRule, alpha: f32) {
        if !self.draw {
            return;
        }
        let area = intersect_area(
            self.drawable_area(),
            path_area(device, self.pixmap.bounds()),
        );
        let Some(coverage) = rasterize_in(device, rule, area) else {
            return;
        };
        match &pattern.kind {
            PatternKind::Shading(shading) => {
                let mut clip = self.clip_mask(device);
                if let Some(bg) = shading.background_color() {
                    fill_path(
                        self.pixmap,
                        device,
                        rule,
                        bg.with_alpha(alpha),
                        clip.as_ref(),
                    );
                }
                if let Some(b) = shading.bbox {
                    let rect = Path::rect(
                        b[0].min(b[2]),
                        b[1].min(b[3]),
                        (b[2] - b[0]).abs(),
                        (b[3] - b[1]).abs(),
                    );
                    clip = Some(self.extra_clip(clip, &rect.transform(&pattern.to_device), area));
                }
                shading.paint(
                    self.pixmap,
                    &coverage,
                    clip.as_ref(),
                    &pattern.to_device,
                    alpha,
                );
            }
            PatternKind::Tiling(tiling) => {
                if self.pattern_depth >= MAX_PATTERN_DEPTH {
                    self.warn("patterns nest too deeply");
                    return;
                }
                if tiling.uncolored && tiling.color.is_none() {
                    return;
                }
                let cell = self.tiling_cell(pattern);
                let Some(inverse) = pattern.to_device.invert() else {
                    return;
                };
                let clip = self.clip_mask(device);
                // Four samples a pixel smooth the cell's edges when it is
                // rotated or scaled.
                const OFFSETS: [(f64, f64); 4] =
                    [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
                self.pixmap
                    .fill_coverage_with(&coverage, clip.as_ref(), alpha, |x, y| {
                        let mut sum = [0u32; 4];
                        for (dx, dy) in OFFSETS {
                            let u = inverse.apply(Point::new(x as f64 + dx, y as f64 + dy));
                            if let Some(px) = tiling.sample(&cell, u) {
                                for k in 0..4 {
                                    sum[k] += u32::from(px[k]);
                                }
                            }
                        }
                        (sum[3] > 0).then(|| sum.map(|v| ((v + 2) / 4) as u8))
                    });
            }
        }
    }

    /// Renders a tiling pattern's cell once and keeps it with the pattern.
    fn tiling_cell(&mut self, pattern: &Pattern) -> Rc<Cell> {
        let PatternKind::Tiling(tiling) = &pattern.kind else {
            unreachable!("only tiling patterns have cells");
        };
        if let Some(cell) = tiling.cell.borrow().as_ref() {
            return cell.clone();
        }
        let (w, h, to_cell) = tiling.cell_geometry(&pattern.to_device);
        let mut pixmap = Pixmap::new(w, h);
        let content = match self.doc.decode_stream(&tiling.stream) {
            Ok(d) => d.data,
            Err(e) => {
                self.warn(format!("pattern content: {e}"));
                Vec::new()
            }
        };
        let warnings = {
            let mut sub = Interpreter::new(self.doc, &mut pixmap, to_cell);
            sub.pattern_depth = self.pattern_depth + 1;
            sub.operations = self.operations;
            if tiling.uncolored {
                sub.gs.forced = tiling.color;
            }
            sub.run(&content, &tiling.resources, 0);
            self.operations = sub.operations;
            sub.warnings
        };
        for w in warnings {
            self.warn(w);
        }
        let cell = Rc::new(Cell { pixmap, to_cell });
        *tiling.cell.borrow_mut() = Some(cell.clone());
        cell
    }

    /// `clip` narrowed by a device-space path, over `area`.
    fn extra_clip(&self, clip: Option<Mask>, device: &Path, area: Area) -> Mask {
        let size = self.pixmap.bounds();
        let coverage = rasterize_in(device, FillRule::NonZero, area);
        match clip {
            Some(mut m) => {
                m.intersect(coverage.as_ref());
                m
            }
            None => Mask::from_coverage(coverage.as_ref(), size.0, size.1),
        }
    }

    /// The `sh` operator: paints a shading over the whole clip.
    fn shade(&mut self, name: &Name, resources: &Dict) {
        if !self.draw {
            return;
        }
        let Some((obj, _)) = self.lookup(resources, b"Shading", name) else {
            return;
        };
        let mut warnings = Vec::new();
        let shading = Shading::parse(self.doc, &obj, resources, &mut |w| warnings.push(w));
        for w in warnings {
            self.warn(w);
        }
        let Some(shading) = shading else {
            self.warn("a shading could not be read");
            return;
        };
        let area = self.drawable_area();
        let rect = Path::rect(area.0 as f64, area.1 as f64, area.2 as f64, area.3 as f64);
        let Some(coverage) = rasterize_in(&rect, FillRule::NonZero, area) else {
            return;
        };
        let mut clip = self.clip_mask(&rect);
        if let Some(b) = shading.bbox {
            let bbox = Path::rect(
                b[0].min(b[2]),
                b[1].min(b[3]),
                (b[2] - b[0]).abs(),
                (b[3] - b[1]).abs(),
            );
            clip = Some(self.extra_clip(clip, &bbox.transform(&self.gs.ctm), area));
        }
        let ctm = self.gs.ctm;
        let alpha = self.gs.fill_alpha;
        let before = self.probe_pixel();
        shading.paint(self.pixmap, &coverage, clip.as_ref(), &ctm, alpha);
        self.probe_record(before, PaintKind::Shading, shading.space(), &[]);
    }

    /// The clip mask for drawing a device-space shape, or `None` when
    /// nothing is clipped.
    fn clip_mask(&self, device: &Path) -> Option<Mask> {
        let size = self.pixmap.bounds();
        let mut mask = self
            .gs
            .clip
            .as_ref()
            .map(|c| c.mask(path_area(device, size), size));
        if let Some(soft) = &self.gs.soft_mask {
            match &mut mask {
                Some(m) => m.intersect(Some(soft)),
                None => mask = Some(Mask::from_coverage(Some(soft), size.0, size.1)),
            }
        }
        mask
    }

    /// Intersects the clip with a device-space path.
    fn clip_device(&mut self, device: &Path, rule: FillRule) {
        let size = self.pixmap.bounds();
        let mut area = path_area(device, size);
        if let Some(parent) = &self.gs.clip {
            area = intersect_area(area, parent.area);
        }
        self.gs.clip = Some(Rc::new(Clip {
            parent: self.gs.clip.clone(),
            path: device.clone(),
            rule,
            area,
        }));
    }

    fn ext_gstate(&mut self, name: &Name, resources: &Dict) {
        let Some(dict) = self
            .lookup(resources, b"ExtGState", name)
            .and_then(|(o, _)| o.as_dict().cloned())
        else {
            return;
        };
        for (key, value) in dict.iter() {
            let value = self.doc.resolve(value).unwrap_or(Object::Null);
            let num = value.as_f64();
            match key.as_bytes() {
                b"LW" => self.gs.stroke.width = num.unwrap_or(1.0).abs(),
                b"LC" => self.gs.stroke.cap = cap(num.unwrap_or(0.0)),
                b"LJ" => self.gs.stroke.join = join(num.unwrap_or(0.0)),
                b"ML" => self.gs.stroke.miter_limit = num.unwrap_or(10.0).max(1.0),
                b"D" => {
                    if let Some([Object::Array(a), phase]) =
                        value.as_array().and_then(|a| a.get(..2))
                    {
                        self.gs.stroke.dash = a.iter().filter_map(Object::as_f64).collect();
                        self.gs.stroke.dash_phase = phase.as_f64().unwrap_or(0.0);
                    }
                }
                b"CA" => self.gs.stroke_alpha = num.unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                b"ca" => self.gs.fill_alpha = num.unwrap_or(1.0).clamp(0.0, 1.0) as f32,
                b"BM" => {
                    // An array lists modes in order of preference.
                    let names: Vec<&[u8]> = match &value {
                        Object::Name(n) => vec![n.as_bytes()],
                        Object::Array(a) => a
                            .iter()
                            .filter_map(Object::as_name)
                            .map(|n| n.as_bytes())
                            .collect(),
                        _ => Vec::new(),
                    };
                    self.gs.blend = names
                        .iter()
                        .find_map(|n| BlendMode::from_name(n))
                        .unwrap_or_default();
                }
                b"SMask" => {
                    self.gs.soft_mask = match &value {
                        Object::Dict(d) => self.soft_mask(d, resources),
                        _ => None,
                    };
                }
                _ => {}
            }
        }
    }

    /// Looks up `name` in a resource category; returns the object and its
    /// reference when it is indirect.
    fn lookup(
        &self,
        resources: &Dict,
        category: &[u8],
        name: &Name,
    ) -> Option<(Object, Option<ObjRef>)> {
        let cat = self.doc.get_key(resources, category).ok()?;
        let entry = cat.as_dict()?.get(name.as_bytes())?.clone();
        let r = entry.as_reference();
        Some((self.doc.resolve(&entry).ok()?, r))
    }

    fn xobject(&mut self, name: &Name, resources: &Dict, depth: usize) {
        let Some((Object::Stream(stream), r)) = self.lookup(resources, b"XObject", name) else {
            return;
        };
        let subtype = stream
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .map(|n| n.as_bytes().to_vec());
        match subtype.as_deref() {
            Some(b"Image") => self.image_xobject(&stream, resources),
            Some(b"Form") => self.form(&stream, r, resources, depth),
            _ => {}
        }
    }

    fn form(&mut self, stream: &Stream, r: Option<ObjRef>, parent_resources: &Dict, depth: usize) {
        if depth >= MAX_FORM_DEPTH || r.is_some_and(|r| self.forms.contains(&r)) {
            self.warn("form XObjects nest too deeply or recursively");
            return;
        }
        let doc = self.doc;
        let group = doc
            .get_key(&stream.dict, b"Group")
            .ok()
            .and_then(|g| g.as_dict().cloned())
            .filter(|g| {
                doc.get_key(g, b"S")
                    .ok()
                    .and_then(|s| s.as_name().map(|n| *n == b"Transparency"))
                    .unwrap_or(false)
            });
        if let Some(group) = group {
            let isolated = doc
                .get_key(&group, b"I")
                .ok()
                .is_some_and(|v| matches!(v, Object::Bool(true)));
            // A group composited normally onto its backdrop looks the same
            // as drawing its content straight onto the page.
            let plain = self.gs.blend == BlendMode::Normal
                && self.gs.fill_alpha >= 1.0
                && self.gs.soft_mask.is_none()
                && !isolated;
            if self.draw && !plain {
                self.group(stream, r, parent_resources, depth);
                return;
            }
        }
        self.form_content(stream, r, parent_resources, depth);
    }

    /// A form's matrix and its bounding box in device space.
    fn form_geometry(&self, stream: &Stream) -> (Transform, Option<Path>) {
        let matrix = self
            .doc
            .get_key(&stream.dict, b"Matrix")
            .ok()
            .and_then(|m| {
                let v = nums(m.as_array()?);
                (v.len() == 6).then(|| Transform::from_array([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Transform::IDENTITY);
        let ctm = matrix.then(&self.gs.ctm);
        let bbox = self.doc.get_key(&stream.dict, b"BBox").ok().and_then(|b| {
            let v = nums(b.as_array()?);
            (v.len() == 4).then(|| {
                Path::rect(
                    v[0].min(v[2]),
                    v[1].min(v[3]),
                    (v[2] - v[0]).abs(),
                    (v[3] - v[1]).abs(),
                )
                .transform(&ctm)
            })
        });
        (matrix, bbox)
    }

    /// A sub-interpreter drawing into `pixmap` with `ctm`, sharing this
    /// one's limits and recursion guards.
    fn sub_interpreter<'b>(&self, pixmap: &'b mut Pixmap, ctm: Transform) -> Interpreter<'b>
    where
        'a: 'b,
    {
        let mut sub = Interpreter::new(self.doc, pixmap, ctm);
        sub.forms = self.forms.clone();
        sub.operations = self.operations;
        sub.pattern_depth = self.pattern_depth;
        sub
    }

    /// Takes back what a sub-interpreter learned: its warnings and how much
    /// work it did.
    fn absorb(&mut self, operations: usize, warnings: Vec<String>) {
        self.operations = operations;
        for w in warnings {
            self.warn(w);
        }
    }

    /// Draws a transparency group: its content goes into a layer of its own,
    /// which is then composited with the current alpha, blend mode and soft
    /// mask.
    fn group(&mut self, stream: &Stream, r: Option<ObjRef>, resources: &Dict, depth: usize) {
        let size = self.pixmap.bounds();
        let (_, bbox) = self.form_geometry(stream);
        let mut area = self.drawable_area();
        if let Some(b) = &bbox {
            area = intersect_area(area, path_area(b, size));
        }
        if area.2 == 0 || area.3 == 0 {
            return;
        }
        let mut layer = Pixmap::new(area.2, area.3);
        let shift = Transform::translate(-(area.0 as f64), -(area.1 as f64));
        let ctm = self.gs.ctm.then(&shift);
        let (operations, warnings) = {
            let mut sub = self.sub_interpreter(&mut layer, ctm);
            sub.probe = self.probe.clone();
            // The group starts with a fresh compositing state; the clip is
            // applied when the layer is composited.
            sub.gs = GraphicsState {
                ctm,
                clip: None,
                soft_mask: None,
                blend: BlendMode::Normal,
                fill_alpha: 1.0,
                stroke_alpha: 1.0,
                ..self.gs.clone()
            };
            sub.fonts = std::mem::take(&mut self.fonts);
            sub.form_content(stream, r, resources, depth);
            self.fonts = std::mem::take(&mut sub.fonts);
            (sub.operations, sub.warnings)
        };
        self.absorb(operations, warnings);
        let rect = Path::rect(area.0 as f64, area.1 as f64, area.2 as f64, area.3 as f64);
        let clip = self.clip_mask(&rect);
        self.pixmap.set_blend(self.gs.blend);
        self.pixmap
            .composite(&layer, (area.0, area.1), clip.as_ref(), self.gs.fill_alpha);
    }

    /// Renders an ExtGState soft mask (ISO 32000-2, 11.6.5.2) for the
    /// current transform.
    fn soft_mask(&mut self, dict: &Dict, resources: &Dict) -> Option<Rc<Coverage>> {
        let doc = self.doc;
        let g = dict.get(b"G")?;
        let r = g.as_reference()?;
        if self.forms.contains(&r) || self.forms.len() >= MAX_FORM_DEPTH {
            return None;
        }
        let Ok(Object::Stream(stream)) = doc.resolve(g) else {
            return None;
        };
        let key = (r, self.gs.ctm.to_array().map(f64::to_bits));
        if let Some(mask) = self.soft_masks.get(&key) {
            return Some(mask.clone());
        }
        let luminosity = doc
            .get_key(dict, b"S")
            .ok()
            .and_then(|s| s.as_name().map(|n| *n == b"Luminosity"))
            .unwrap_or(false);
        let (w, h) = self.pixmap.bounds();
        let mut pixmap = if luminosity {
            // The backdrop is in the group's colour space; black by default.
            let bc: Vec<f32> = doc
                .get_key(dict, b"BC")
                .ok()
                .and_then(|b| {
                    b.as_array().map(|a| {
                        a.iter()
                            .filter_map(Object::as_f64)
                            .map(|v| v as f32)
                            .collect()
                    })
                })
                .unwrap_or_default();
            let space = doc
                .get_key(&stream.dict, b"Group")
                .ok()
                .and_then(|g| g.as_dict().and_then(|g| g.get(b"CS").cloned()))
                .and_then(|cs| colorspace::parse(doc, &cs, resources));
            let backdrop = match space {
                Some(space) if bc.len() == space.components() => space.to_rgb(&bc),
                _ => None,
            }
            .unwrap_or(Color::BLACK);
            Pixmap::filled(w, h, backdrop)
        } else {
            Pixmap::new(w, h)
        };
        let transfer = match doc.get_key(dict, b"TR") {
            Ok(Object::Name(_)) | Err(_) => None,
            Ok(obj) => Functions::parse(doc, &obj),
        };
        let ctm = self.gs.ctm;
        let (operations, warnings) = {
            let mut sub = self.sub_interpreter(&mut pixmap, ctm);
            sub.form_content(&stream, Some(r), resources, self.forms.len());
            (sub.operations, sub.warnings)
        };
        self.absorb(operations, warnings);

        let table: Vec<u8> = (0..256)
            .map(|i| {
                let v = i as f32 / 255.0;
                let v = transfer
                    .as_ref()
                    .map_or(v, |f| f.eval(&[v]).first().copied().unwrap_or(v));
                (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
            .collect();
        let data = pixmap.data();
        let values: Vec<u8> = data
            .chunks_exact(4)
            .map(|px| {
                let v = if luminosity {
                    (77 * u32::from(px[0]) + 151 * u32::from(px[1]) + 28 * u32::from(px[2]) + 128)
                        >> 8
                } else {
                    u32::from(px[3])
                };
                table[v.min(255) as usize]
            })
            .collect();
        // Keep only the rows and columns the mask lets anything through.
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            let row = &values[y * w..(y + 1) * w];
            if let Some(first) = row.iter().position(|&v| v != 0) {
                let last = row.iter().rposition(|&v| v != 0).unwrap_or(first);
                x0 = x0.min(first);
                x1 = x1.max(last + 1);
                y0 = y0.min(y);
                y1 = y + 1;
            }
        }
        let (cw, ch) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let mut cropped = Vec::with_capacity(cw * ch);
        for y in y0..y1 {
            cropped.extend_from_slice(&values[y * w + x0..y * w + x1]);
        }
        let mask = Rc::new(Coverage {
            x: if cw == 0 { 0 } else { x0 },
            y: if ch == 0 { 0 } else { y0 },
            width: cw,
            height: ch,
            data: cropped,
        });
        self.soft_masks.insert(key, mask.clone());
        Some(mask)
    }

    /// Runs a form's content with its matrix and bounding box.
    fn form_content(
        &mut self,
        stream: &Stream,
        r: Option<ObjRef>,
        parent_resources: &Dict,
        depth: usize,
    ) {
        let content = match self.doc.decode_stream(stream) {
            Ok(d) => d.data,
            Err(e) => {
                self.warn(format!("form content: {e}"));
                return;
            }
        };
        let resources = match self.doc.get_key(&stream.dict, b"Resources") {
            Ok(Object::Dict(d)) => d,
            _ => parent_resources.clone(),
        };
        let (matrix, bbox) = self.form_geometry(stream);
        let saved = self.gs.clone();
        let saved_path = std::mem::take(&mut self.path);
        self.gs.ctm = matrix.then(&self.gs.ctm);
        if let Some(bbox) = bbox {
            self.clip_device(&bbox, FillRule::NonZero);
        }
        if let Some(r) = r {
            self.forms.push(r);
        }
        let saved_base = std::mem::replace(&mut self.base_ctm, self.gs.ctm);
        self.run(&content, &resources, depth + 1);
        self.base_ctm = saved_base;
        if r.is_some() {
            self.forms.pop();
        }
        self.gs = saved;
        self.path = saved_path;
    }

    fn image_xobject(&mut self, stream: &Stream, resources: &Dict) {
        if !self.draw {
            return;
        }
        let decoded = match self.doc.decode_stream(stream) {
            Ok(d) => d,
            Err(e) => {
                self.warn(format!("image: {e}"));
                return;
            }
        };
        let filter = decoded.image_filter.as_ref();
        self.draw_decoded_image(&stream.dict, &decoded.data, filter, resources);
    }

    fn inline_image(&mut self, dict: &Dict, data: &[u8], resources: &Dict) {
        if !self.draw {
            return;
        }
        // Expand the abbreviated filter keys so the normal decoder applies.
        let mut full = dict.clone();
        if let Some(f) = dict.get(b"F") {
            full.insert("Filter", f.clone());
        }
        if let Some(dp) = dict.get(b"DP") {
            full.insert("DecodeParms", dp.clone());
        }
        let stream = Stream {
            dict: full,
            data: data.into(),
        };
        let decoded = match self.doc.decode_stream(&stream) {
            Ok(d) => d,
            Err(e) => {
                self.warn(format!("inline image: {e}"));
                return;
            }
        };
        let filter = decoded.image_filter.as_ref();
        self.draw_decoded_image(dict, &decoded.data, filter, resources);
    }

    fn draw_decoded_image(
        &mut self,
        dict: &Dict,
        data: &[u8],
        filter: Option<&(atelier_syntax::ImageFilter, Dict)>,
        resources: &Dict,
    ) {
        let fill = self
            .gs
            .forced
            .or_else(|| self.gs.fill_space.to_rgb(&self.gs.fill))
            .unwrap_or(Color::BLACK);
        let mut warnings = Vec::new();
        let info = image::decode(self.doc, dict, data, filter, resources, fill, &mut |w| {
            warnings.push(w)
        });
        for w in warnings {
            self.warn(w);
        }
        let Some(info) = info else { return };
        // Upscaled images stay crisp unless the file asks for smoothing.
        let ctm = self.gs.ctm;
        let device_w = ctm.apply_vector(Point::new(1.0, 0.0)).length();
        let device_h = ctm.apply_vector(Point::new(0.0, 1.0)).length();
        let upscaled = device_w > info.image.width as f64 && device_h > info.image.height as f64;
        let sampling = if upscaled && !info.interpolate {
            ImageSampling::Nearest
        } else {
            ImageSampling::Bilinear
        };
        let clip = self.clip_mask(&Path::rect(0.0, 0.0, 1.0, 1.0).transform(&ctm));
        let before = self.probe_pixel();
        draw_image(
            self.pixmap,
            &info.image,
            &ctm,
            sampling,
            self.gs.fill_alpha,
            clip.as_ref(),
        );
        if before.is_some() {
            let mask = dict
                .get(b"ImageMask")
                .or_else(|| dict.get(b"IM"))
                .and_then(Object::as_bool)
                .unwrap_or(false);
            if mask {
                self.probe_paint(before, PaintKind::Image, false);
            } else {
                let space = dict
                    .get(b"ColorSpace")
                    .or_else(|| dict.get(b"CS"))
                    .and_then(|cs| colorspace::parse(self.doc, cs, resources));
                match space {
                    Some(space) => self.probe_record(before, PaintKind::Image, &space, &[]),
                    None => self.probe_record(before, PaintKind::Image, &ColorSpace::Rgb, &[]),
                }
            }
        }
    }

    // ----- Colour picking ------------------------------------------------

    /// The pixel being picked, before a paint, when picking.
    fn probe_pixel(&self) -> Option<[u8; 4]> {
        self.probe.as_ref()?;
        let d = self.pixmap.data();
        (d.len() >= 4).then(|| [d[0], d[1], d[2], d[3]])
    }

    /// Records the current fill or stroke colour if the paint just done
    /// changed the picked pixel.
    fn probe_paint(&mut self, before: Option<[u8; 4]>, kind: PaintKind, stroke: bool) {
        if before.is_none() {
            return;
        }
        let (space, comps) = if stroke {
            (self.gs.stroke_space.clone(), self.gs.stroke_color.clone())
        } else {
            (self.gs.fill_space.clone(), self.gs.fill.clone())
        };
        self.probe_record(before, kind, &space, &comps);
    }

    fn probe_record(
        &mut self,
        before: Option<[u8; 4]>,
        kind: PaintKind,
        space: &ColorSpace,
        comps: &[f32],
    ) {
        let (Some(before), Some(probe)) = (before, &self.probe) else {
            return;
        };
        if self.probe_pixel() == Some(before) {
            return;
        }
        *probe.borrow_mut() = Some(Paint {
            kind,
            space: space.describe(),
            components: comps.to_vec(),
            spots: space.spot_names(),
            colorants: space.colorants().to_vec(),
            alternate: space
                .alternate(comps)
                .map(|(alt, values)| (alt.describe(), values)),
        });
    }

    fn next_line(&mut self, tx: f64, ty: f64) {
        self.tlm = Transform::translate(tx, ty).then(&self.tlm);
        self.tm = self.tlm;
    }

    /// A TJ number: moves the next glyph back by n / 1000 of the font size.
    fn adjust(&mut self, n: f64) {
        let t = &self.gs.text;
        let vertical = t.font.as_ref().is_some_and(|f| f.is_vertical());
        let d = -n / 1000.0 * t.size;
        self.tm = if vertical {
            Transform::translate(0.0, d)
        } else {
            Transform::translate(d * t.scale, 0.0)
        }
        .then(&self.tm);
    }

    fn font(&mut self, name: &Name, resources: &Dict) -> Option<Rc<PdfFont>> {
        let Some((Object::Dict(dict), r)) = self.lookup(resources, b"Font", name) else {
            self.warn(format!("font {} is missing", name.as_str_lossy()));
            return None;
        };
        if let Some(f) = r.and_then(|r| self.fonts.get(&r)) {
            return Some(f.clone());
        }
        let mut warnings = Vec::new();
        let font = Rc::new(PdfFont::load(self.doc, &dict, &mut |w| warnings.push(w)));
        for w in warnings {
            self.warn(w);
        }
        if let Some(r) = r {
            self.fonts.insert(r, font.clone());
        }
        Some(font)
    }

    /// Shows a string with the current font (Tj).
    fn show(&mut self, bytes: &[u8], resources: &Dict, depth: usize) {
        let Some(font) = self.gs.text.font.clone() else {
            self.warn("text shown without a font");
            return;
        };
        let vertical = font.is_vertical();
        for g in font.decode(bytes) {
            let t = &self.gs.text;
            let (size, scale, rise, render) = (t.size, t.scale, t.rise, t.render);
            // Glyph space (1.0 = font size) to user space.
            let mut glyph_m = Transform::new(size * scale, 0.0, 0.0, size, 0.0, rise);
            if let Some((_, vx, vy)) = g.vertical {
                glyph_m = glyph_m.then(&Transform::translate(-vx * size * scale, -vy * size));
            }
            let to_user = glyph_m.then(&self.tm);

            if font.is_type3() {
                self.type3_glyph(&font, g.code, &to_user, resources, depth);
            } else if render >= 4 || (render != 3 && self.draw) {
                if let Some(gid) = g.gid {
                    if !(render == 0 && self.stamp_glyph(&font, gid, &to_user)) {
                        if let Some(outline) = font.outline(gid) {
                            self.paint_glyph(&outline, &to_user, render);
                        }
                    }
                }
            }

            let advance = if vertical {
                g.vertical.map_or(-1.0, |v| v.0)
            } else {
                g.width
            };
            self.record_glyph(&font, &g, &to_user, advance, vertical, render == 3);

            let t = &self.gs.text;
            let spacing = t.char_spacing + if g.is_space { t.word_spacing } else { 0.0 };
            self.tm = if vertical {
                Transform::translate(0.0, advance * t.size + spacing)
            } else {
                Transform::translate((g.width * t.size + spacing) * t.scale, 0.0)
            }
            .then(&self.tm);
        }
    }

    /// Fills a glyph from the font's cache of rasterized glyphs, when it is
    /// filled with a plain colour. Returns false to have it drawn as a path.
    fn stamp_glyph(&mut self, font: &PdfFont, gid: u32, to_user: &Transform) -> bool {
        if !self.draw {
            return true;
        }
        let pattern = matches!(self.gs.fill_space, ColorSpace::Pattern { .. });
        if pattern && self.gs.forced.is_none() {
            return false;
        }
        let device = to_user.then(&self.gs.ctm);
        let Some((mask, x, y)) = font.glyph_mask(gid, &device) else {
            // No outline at all draws nothing either way.
            return font.outline(gid).is_none();
        };
        let Some(color) = self.gs.fill_color() else {
            return true;
        };
        let area = self.drawable_area();
        let Some(coverage) = mask.coverage.placed(x, y, area) else {
            return true;
        };
        let bounds = Path::rect(
            coverage.x as f64,
            coverage.y as f64,
            coverage.width as f64,
            coverage.height as f64,
        );
        let clip = self.clip_mask(&bounds);
        let before = self.probe_pixel();
        self.pixmap.fill_coverage(&coverage, color, clip.as_ref());
        self.probe_paint(before, PaintKind::Text, false);
        true
    }

    fn paint_glyph(&mut self, outline: &Path, to_user: &Transform, render: u8) {
        let user = outline.transform(to_user);
        if render >= 4 {
            let device = user.transform(&self.gs.ctm);
            self.text_clip
                .get_or_insert_with(PathBuilder::new)
                .append(&device);
        }
        if !self.draw {
            return;
        }
        self.painting_text = true;
        if matches!(render, 0 | 2 | 4 | 6) {
            let device = user.transform(&self.gs.ctm);
            self.paint_device(&device, FillRule::NonZero, false);
        }
        if matches!(render, 1 | 2 | 5 | 6) {
            let outline = stroke_to_path(&user, &self.gs.stroke, &self.gs.ctm);
            self.paint_device(&outline, FillRule::NonZero, true);
        }
        self.painting_text = false;
    }

    /// Draws a Type 3 glyph by running its content stream.
    fn type3_glyph(
        &mut self,
        font: &PdfFont,
        code: u32,
        to_user: &Transform,
        resources: &Dict,
        depth: usize,
    ) {
        let Some((matrix, proc, own_resources)) = font.type3(code) else {
            return;
        };
        let Some(Ok(Object::Stream(stream))) = proc.map(|p| self.doc.resolve(p)) else {
            return;
        };
        if depth >= MAX_FORM_DEPTH {
            self.warn("Type 3 glyphs nest too deeply");
            return;
        }
        let Ok(decoded) = self.doc.decode_stream(&stream) else {
            return;
        };
        let res = own_resources.cloned().unwrap_or_else(|| resources.clone());
        let saved = self.gs.clone();
        let saved_path = std::mem::take(&mut self.path);
        let (saved_tm, saved_tlm) = (self.tm, self.tlm);
        let saved_clip = self.text_clip.take();
        self.gs.ctm = matrix.then(to_user).then(&self.gs.ctm);
        self.gs.text = TextState::default();
        self.run(&decoded.data, &res, depth + 1);
        self.gs = saved;
        self.path = saved_path;
        self.tm = saved_tm;
        self.tlm = saved_tlm;
        self.text_clip = saved_clip;
    }

    fn record_glyph(
        &mut self,
        font: &PdfFont,
        g: &crate::font::Glyph,
        to_user: &Transform,
        advance: f64,
        vertical: bool,
        invisible: bool,
    ) {
        let Some(out) = self.text.as_mut() else {
            return;
        };
        let m = to_user.then(&self.gs.ctm);
        // Glyph box in glyph space: the advance by a typical ascent/descent.
        let (x0, x1, y0, y1) = if vertical {
            (-0.5, 0.5, advance, 0.0)
        } else {
            (0.0, advance, -0.2, 0.8)
        };
        let p = |x: f64, y: f64| m.apply(Point::new(x, y));
        let (b0, b1) = if vertical {
            (p(0.0, 0.0), p(0.0, advance))
        } else {
            (p(0.0, 0.0), p(advance, 0.0))
        };
        out.push(TextGlyph {
            text: g.unicode.clone().unwrap_or_default(),
            quad: [p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)],
            baseline: (b0, b1),
            size: m.apply_vector(Point::new(0.0, 1.0)).length(),
            font: font.name.clone(),
            invisible,
        });
    }
}

fn cap(v: f64) -> LineCap {
    match v as i64 {
        1 => LineCap::Round,
        2 => LineCap::Square,
        _ => LineCap::Butt,
    }
}

fn join(v: f64) -> LineJoin {
    match v as i64 {
        1 => LineJoin::Round,
        2 => LineJoin::Bevel,
        _ => LineJoin::Miter,
    }
}
