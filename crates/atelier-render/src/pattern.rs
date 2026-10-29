//! Patterns (ISO 32000-2, 8.7.3): tiling patterns, drawn by rendering one
//! cell and repeating it, and shading patterns.

use std::cell::RefCell;
use std::rc::Rc;

use atelier_raster::{Color, Pixmap, Point, Transform};
use atelier_syntax::{Dict, Document, Object, Stream};

use crate::shading::Shading;

/// Cells bigger than this many pixels on a side are drawn at lower
/// resolution.
const MAX_CELL: f64 = 2048.0;
/// At most this many overlapping copies of a cell are composited per pixel.
const MAX_COPIES: i64 = 4;

pub struct Pattern {
    /// Pattern space to device space.
    pub to_device: Transform,
    pub kind: PatternKind,
}

pub enum PatternKind {
    Tiling(Tiling),
    Shading(Box<Shading>),
}

pub struct Tiling {
    pub bbox: [f64; 4],
    pub xstep: f64,
    pub ystep: f64,
    /// PaintType 2: the cell is a stencil painted in `color`.
    pub color: Option<Color>,
    pub uncolored: bool,
    pub stream: Stream,
    pub resources: Dict,
    /// The rendered cell and the pattern-space to cell-pixel transform,
    /// made on first use.
    pub cell: RefCell<Option<Rc<Cell>>>,
}

pub struct Cell {
    pub pixmap: Pixmap,
    /// Pattern space to cell pixels.
    pub to_cell: Transform,
}

impl Pattern {
    /// Parses a pattern object; `base` is the default space of the content
    /// stream using it.
    pub fn parse(
        doc: &Document,
        obj: &Object,
        base: &Transform,
        resources: &Dict,
        warn: &mut dyn FnMut(String),
    ) -> Option<Pattern> {
        let obj = doc.resolve(obj).ok()?;
        let dict = obj.as_dict()?;
        let matrix = doc
            .get_key(dict, b"Matrix")
            .ok()
            .and_then(|m| {
                let v: Vec<f64> = m.as_array()?.iter().filter_map(Object::as_f64).collect();
                (v.len() == 6).then(|| Transform::from_array([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Transform::IDENTITY);
        let to_device = matrix.then(base);
        let kind = match doc.get_key(dict, b"PatternType").ok()?.as_i64()? {
            1 => {
                let Object::Stream(stream) = &obj else {
                    return None;
                };
                let num = |k: &[u8]| doc.get_key(dict, k).ok().and_then(|o| o.as_f64());
                let bbox: Vec<f64> = doc
                    .get_key(dict, b"BBox")
                    .ok()?
                    .as_array()?
                    .iter()
                    .filter_map(Object::as_f64)
                    .collect();
                if bbox.len() != 4 {
                    return None;
                }
                let bbox = [
                    bbox[0].min(bbox[2]),
                    bbox[1].min(bbox[3]),
                    bbox[0].max(bbox[2]),
                    bbox[1].max(bbox[3]),
                ];
                let xstep = num(b"XStep")?.abs();
                let ystep = num(b"YStep")?.abs();
                if xstep == 0.0 || ystep == 0.0 || bbox[2] <= bbox[0] || bbox[3] <= bbox[1] {
                    return None;
                }
                let pattern_resources = match doc.get_key(dict, b"Resources") {
                    Ok(Object::Dict(d)) => d,
                    _ => resources.clone(),
                };
                PatternKind::Tiling(Tiling {
                    bbox,
                    xstep,
                    ystep,
                    color: None,
                    uncolored: num(b"PaintType") == Some(2.0),
                    stream: stream.clone(),
                    resources: pattern_resources,
                    cell: RefCell::new(None),
                })
            }
            2 => {
                let shading =
                    Shading::parse(doc, &doc.get_key(dict, b"Shading").ok()?, resources, warn)?;
                PatternKind::Shading(Box::new(shading))
            }
            _ => return None,
        };
        Some(Pattern { to_device, kind })
    }
}

impl Tiling {
    /// The size of the cell pixmap and the pattern-space to cell transform
    /// for drawing with `to_device`.
    pub fn cell_geometry(&self, to_device: &Transform) -> (usize, usize, Transform) {
        let sx = to_device.apply_vector(Point::new(1.0, 0.0)).length();
        let sy = to_device.apply_vector(Point::new(0.0, 1.0)).length();
        let (bw, bh) = (self.bbox[2] - self.bbox[0], self.bbox[3] - self.bbox[1]);
        let mut w = (bw * sx).ceil().max(1.0);
        let mut h = (bh * sy).ceil().max(1.0);
        let shrink = (MAX_CELL / w).min(MAX_CELL / h).min(1.0);
        w = (w * shrink).ceil().max(1.0);
        h = (h * shrink).ceil().max(1.0);
        let to_cell = Transform::translate(-self.bbox[0], -self.bbox[1]).then(&Transform::new(
            w / bw,
            0.0,
            0.0,
            h / bh,
            0.0,
            0.0,
        ));
        (w as usize, h as usize, to_cell)
    }

    /// The premultiplied colour of pattern-space point `u`, compositing
    /// every copy of the cell that covers it.
    pub fn sample(&self, cell: &Cell, u: Point) -> Option<[u8; 4]> {
        let [x0, y0, x1, y1] = self.bbox;
        let i_lo = ((u.x - x1) / self.xstep).ceil() as i64;
        let i_hi = ((u.x - x0) / self.xstep).floor() as i64;
        let j_lo = ((u.y - y1) / self.ystep).ceil() as i64;
        let j_hi = ((u.y - y0) / self.ystep).floor() as i64;
        if i_hi < i_lo || j_hi < j_lo {
            return None;
        }
        let (w, h) = (cell.pixmap.width(), cell.pixmap.height());
        let mut out = [0u32; 4];
        let mut any = false;
        for j in j_lo..=j_hi.min(j_lo + MAX_COPIES - 1) {
            for i in i_lo..=i_hi.min(i_lo + MAX_COPIES - 1) {
                let q = Point::new(u.x - i as f64 * self.xstep, u.y - j as f64 * self.ystep);
                let c = cell.to_cell.apply(q);
                if c.x < 0.0 || c.y < 0.0 {
                    continue;
                }
                let (cx, cy) = (c.x as usize, c.y as usize);
                if cx >= w || cy >= h {
                    continue;
                }
                let px = cell.pixmap.pixel(cx, cy);
                // Later copies go over earlier ones.
                let inv = 255 - u32::from(px[3]);
                for k in 0..4 {
                    out[k] = u32::from(px[k]) + (out[k] * inv + 127) / 255;
                }
                any = true;
            }
        }
        (any && out[3] > 0).then(|| out.map(|v| v.min(255) as u8))
    }
}
