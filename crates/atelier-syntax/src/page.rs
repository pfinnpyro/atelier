//! The page tree (ISO 32000-2, 7.7.3).

use std::collections::HashSet;

use crate::{Dict, Document, Error, ObjRef, Object, Result};

const MAX_TREE_DEPTH: usize = 64;

/// A rectangle in default user space, normalized so `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    /// US Letter, used when a page has no usable /MediaBox.
    pub const LETTER: Rect = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 612.0,
        y1: 792.0,
    };

    pub fn from_array(items: &[Object]) -> Option<Self> {
        if items.len() != 4 {
            return None;
        }
        let v: Vec<f64> = items.iter().filter_map(Object::as_f64).collect();
        if v.len() != 4 || v.iter().any(|x| !x.is_finite()) {
            return None;
        }
        Some(Self {
            x0: v[0].min(v[2]),
            y0: v[1].min(v[3]),
            x1: v[0].max(v[2]),
            y1: v[1].max(v[3]),
        })
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        (r.x0 < r.x1 && r.y0 < r.y1).then_some(r)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// The page object, when it is an indirect object (it nearly always is).
    pub obj_ref: Option<ObjRef>,
    pub dict: Dict,
    pub media_box: Rect,
    /// The visible region: /CropBox clipped to the media box.
    pub crop_box: Rect,
    /// Clockwise rotation in degrees: 0, 90, 180 or 270.
    pub rotate: u16,
    /// The resource dictionary, inherited from ancestors when the page lacks one.
    pub resources: Dict,
}

impl Page {
    /// The page's content streams, decoded and joined in order.
    pub fn contents(&self, doc: &Document) -> Result<Vec<u8>> {
        let parts = match doc.get_key(&self.dict, b"Contents")? {
            Object::Array(items) => items,
            Object::Null => return Ok(Vec::new()),
            single => vec![single],
        };
        let mut out = Vec::new();
        for part in parts {
            match doc.resolve(&part)? {
                Object::Stream(s) => {
                    out.extend_from_slice(&doc.decode_stream(&s)?.data);
                    // Streams may split mid-token only at whitespace; keep them apart.
                    out.push(b'\n');
                }
                Object::Null => {}
                other => {
                    return Err(Error::WrongType {
                        expected: "content stream",
                        found: other.type_name(),
                    });
                }
            }
        }
        Ok(out)
    }
}

#[derive(Clone, Default)]
struct Inherited {
    resources: Option<Dict>,
    media_box: Option<Rect>,
    crop_box: Option<Rect>,
    rotate: Option<i64>,
}

pub(crate) fn collect_pages(doc: &Document) -> Result<Vec<Page>> {
    let catalog = doc.catalog()?;
    let mut pages = Vec::new();
    let mut visited = HashSet::new();
    if let Some(root) = catalog.get(b"Pages") {
        walk(doc, root, Inherited::default(), &mut visited, &mut pages, 0)?;
    }
    if pages.is_empty() {
        // Broken or missing page tree: fall back to every /Type /Page object.
        for num in doc.object_numbers() {
            let generation = match doc.xref_entry(num) {
                Some(crate::XrefEntry::Offset { generation, .. }) => generation,
                _ => 0,
            };
            let r = ObjRef::new(num, generation);
            if let Ok(Object::Dict(d)) = doc.get(r) {
                if d.is_type(b"Page") {
                    pages.push(make_page(doc, Some(r), d, &Inherited::default())?);
                }
            }
        }
    }
    Ok(pages)
}

fn walk(
    doc: &Document,
    node: &Object,
    mut inherited: Inherited,
    visited: &mut HashSet<ObjRef>,
    pages: &mut Vec<Page>,
    depth: usize,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Err(Error::TooDeep);
    }
    let obj_ref = node.as_reference();
    if let Some(r) = obj_ref {
        if !visited.insert(r) {
            return Ok(()); // cycle or shared node
        }
    }
    let dict = match doc.resolve(node)? {
        Object::Dict(d) => d,
        _ => return Ok(()),
    };

    let kids = doc.get_key(&dict, b"Kids")?;
    let is_page = dict.is_type(b"Page") || (!dict.is_type(b"Pages") && kids.as_array().is_none());
    if is_page {
        pages.push(make_page(doc, obj_ref, dict, &inherited)?);
        return Ok(());
    }

    if let Object::Dict(r) = doc.get_key(&dict, b"Resources")? {
        inherited.resources = Some(r);
    }
    if let Some(b) = rect(doc, &dict, b"MediaBox")? {
        inherited.media_box = Some(b);
    }
    if let Some(b) = rect(doc, &dict, b"CropBox")? {
        inherited.crop_box = Some(b);
    }
    if let Some(r) = doc.get_key(&dict, b"Rotate")?.as_i64() {
        inherited.rotate = Some(r);
    }
    for kid in kids.as_array().unwrap_or(&[]) {
        walk(doc, kid, inherited.clone(), visited, pages, depth + 1)?;
    }
    Ok(())
}

fn make_page(
    doc: &Document,
    obj_ref: Option<ObjRef>,
    dict: Dict,
    inherited: &Inherited,
) -> Result<Page> {
    let media_box = rect(doc, &dict, b"MediaBox")?
        .or(inherited.media_box)
        .unwrap_or(Rect::LETTER);
    let crop_box = rect(doc, &dict, b"CropBox")?
        .or(inherited.crop_box)
        .and_then(|c| c.intersect(&media_box))
        .unwrap_or(media_box);
    let rotate = doc
        .get_key(&dict, b"Rotate")?
        .as_i64()
        .or(inherited.rotate)
        .unwrap_or(0);
    let rotate = (((rotate % 360) + 360) % 360 / 90 * 90) as u16;
    let resources = match doc.get_key(&dict, b"Resources")? {
        Object::Dict(d) => d,
        _ => inherited.resources.clone().unwrap_or_default(),
    };
    Ok(Page {
        obj_ref,
        dict,
        media_box,
        crop_box,
        rotate,
        resources,
    })
}

fn rect(doc: &Document, dict: &Dict, key: &[u8]) -> Result<Option<Rect>> {
    let obj = doc.get_key(dict, key)?;
    let Some(items) = obj.as_array() else {
        return Ok(None);
    };
    let resolved: Vec<Object> = items
        .iter()
        .map(|o| doc.resolve(o))
        .collect::<Result<_>>()?;
    Ok(Rect::from_array(&resolved))
}
