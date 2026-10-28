//! The content stream interpreter (ISO 32000-2, chapter 8).

use std::collections::HashSet;
use std::rc::Rc;

use atelier_raster::{
    Color, FillRule, ImageSampling, LineCap, LineJoin, Mask, Path, PathBuilder, Pixmap, Point,
    StrokeStyle, Transform, draw_image, fill_path, rasterize, stroke_to_path,
};
use atelier_syntax::{
    ContentParser, Dict, Document, Name, ObjRef, Object, Operation, Page, Result, Stream,
};

use crate::colorspace::{self, ColorSpace};
use crate::image;

/// Form XObjects nested deeper than this are skipped.
const MAX_FORM_DEPTH: usize = 32;
/// Hard cap on operators per page, so hostile content cannot hang the viewer.
const MAX_OPERATIONS: usize = 20_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Device pixels per PDF point (72 per inch). 1.0 is 72 dpi.
    pub scale: f64,
    pub background: Color,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 150.0 / 72.0,
            background: Color::WHITE,
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
    let (ctm, width, height) = page_transform(page, options.scale);
    let mut pixmap = Pixmap::filled(width, height, options.background);
    let content = page.contents(doc)?;
    let mut interp = Interpreter::new(doc, &mut pixmap, ctm);
    interp.run(&content, &page.resources, 0);
    let warnings = interp.warnings;
    Ok(Rendered { pixmap, warnings })
}

#[derive(Clone)]
struct GraphicsState {
    ctm: Transform,
    clip: Option<Rc<Mask>>,
    stroke: StrokeStyle,
    fill_space: ColorSpace,
    fill: Vec<f32>,
    stroke_space: ColorSpace,
    stroke_color: Vec<f32>,
    fill_alpha: f32,
    stroke_alpha: f32,
}

impl GraphicsState {
    fn new(ctm: Transform) -> Self {
        Self {
            ctm,
            clip: None,
            stroke: StrokeStyle::default(),
            fill_space: ColorSpace::Gray,
            fill: vec![0.0],
            stroke_space: ColorSpace::Gray,
            stroke_color: vec![0.0],
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
        }
    }

    fn fill_color(&self) -> Option<Color> {
        self.fill_space
            .to_rgb(&self.fill)
            .map(|c| c.with_alpha(self.fill_alpha))
    }

    fn stroke_paint(&self) -> Option<Color> {
        self.stroke_space
            .to_rgb(&self.stroke_color)
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
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.seen.insert(message.clone()) {
            self.warnings.push(message);
        }
    }

    fn run(&mut self, content: &[u8], resources: &Dict, depth: usize) {
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
                if matches!(space, ColorSpace::Pattern) {
                    self.warn("pattern fills are not drawn yet");
                }
                if matches!(space, ColorSpace::Spot { .. }) {
                    self.warn("Separation and DeviceN colours are approximated");
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
            b"SC" | b"SCN" => self.gs.stroke_color = n.iter().map(|&v| v as f32).collect(),
            b"sc" | b"scn" => self.gs.fill = n.iter().map(|&v| v as f32).collect(),
            b"G" if !n.is_empty() => self.set_stroke(ColorSpace::Gray, &n[..1]),
            b"g" if !n.is_empty() => self.set_fill(ColorSpace::Gray, &n[..1]),
            b"RG" if n.len() >= 3 => self.set_stroke(ColorSpace::Rgb, &n[..3]),
            b"rg" if n.len() >= 3 => self.set_fill(ColorSpace::Rgb, &n[..3]),
            b"K" if n.len() >= 4 => self.set_stroke(ColorSpace::Cmyk, &n[..4]),
            b"k" if n.len() >= 4 => self.set_fill(ColorSpace::Cmyk, &n[..4]),

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

            b"sh" => self.warn("shadings are not drawn yet"),
            b"BT" | b"ET" => {}
            b"Tj" | b"TJ" | b"'" | b"\"" => self.warn("text is not drawn yet"),
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
        if !path.is_empty() {
            let clip = self.gs.clip.clone();
            if let Some(rule) = fill {
                if let Some(color) = self.gs.fill_color() {
                    let device = path.transform(&self.gs.ctm);
                    fill_path(self.pixmap, &device, rule, color, clip.as_deref());
                }
            }
            if stroke {
                if let Some(color) = self.gs.stroke_paint() {
                    let outline = stroke_to_path(&path, &self.gs.stroke, &self.gs.ctm);
                    fill_path(
                        self.pixmap,
                        &outline,
                        FillRule::NonZero,
                        color,
                        clip.as_deref(),
                    );
                }
            }
        }
        if let Some(rule) = self.pending_clip.take() {
            self.clip_device(&path.transform(&self.gs.ctm), rule);
        }
    }

    /// Intersects the clip with a device-space path.
    fn clip_device(&mut self, device: &Path, rule: FillRule) {
        let (w, h) = self.pixmap.bounds();
        let coverage = rasterize(device, rule, (w, h));
        let mask = match &self.gs.clip {
            Some(existing) => {
                let mut m = (**existing).clone();
                m.intersect(coverage.as_ref());
                m
            }
            None => Mask::from_coverage(coverage.as_ref(), w, h),
        };
        self.gs.clip = Some(Rc::new(mask));
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
                    let normal = match &value {
                        Object::Name(n) => *n == b"Normal" || *n == b"Compatible",
                        Object::Array(a) => a
                            .first()
                            .and_then(Object::as_name)
                            .is_some_and(|n| *n == b"Normal"),
                        _ => true,
                    };
                    if !normal {
                        self.warn("blend modes are not supported yet; using Normal");
                    }
                }
                b"SMask" if !matches!(&value, Object::Name(n) if *n == b"None") => {
                    self.warn("soft masks are not supported yet");
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
        if stream.dict.get(b"Group").is_some() {
            self.warn("transparency groups are drawn as plain forms");
        }
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
        let matrix = self
            .doc
            .get_key(&stream.dict, b"Matrix")
            .ok()
            .and_then(|m| {
                let v = nums(m.as_array()?);
                (v.len() == 6).then(|| Transform::from_array([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Transform::IDENTITY);

        let saved = self.gs.clone();
        let saved_path = std::mem::take(&mut self.path);
        self.gs.ctm = matrix.then(&self.gs.ctm);
        if let Ok(bbox) = self.doc.get_key(&stream.dict, b"BBox") {
            let v = nums(bbox.as_array().unwrap_or(&[]));
            if v.len() == 4 {
                let rect = Path::rect(
                    v[0].min(v[2]),
                    v[1].min(v[3]),
                    (v[2] - v[0]).abs(),
                    (v[3] - v[1]).abs(),
                );
                self.clip_device(&rect.transform(&self.gs.ctm), FillRule::NonZero);
            }
        }
        if let Some(r) = r {
            self.forms.push(r);
        }
        self.run(&content, &resources, depth + 1);
        if r.is_some() {
            self.forms.pop();
        }
        self.gs = saved;
        self.path = saved_path;
    }

    fn image_xobject(&mut self, stream: &Stream, resources: &Dict) {
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
            .fill_space
            .to_rgb(&self.gs.fill)
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
        let clip = self.gs.clip.clone();
        draw_image(
            self.pixmap,
            &info.image,
            &ctm,
            sampling,
            self.gs.fill_alpha,
            clip.as_deref(),
        );
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
