//! Page organizing: the order of the pages, and pages copied in from
//! other files. The page tree is rewritten as one flat list when the order
//! changes; pages copied in bring every object they use, renumbered.

use std::collections::{HashMap, HashSet, VecDeque};

use atelier_syntax::{Dict, Document, Name, ObjRef, Object, Page, Result, Stream, Update};

/// Where a page of the edited document comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PageSlot {
    /// Page `n` of the document as opened.
    Own(usize),
    /// Page `page` of the `file`th file added.
    Added { file: usize, page: usize },
}

/// How deep direct objects may nest when copied.
const MAX_DEPTH: usize = 64;

/// Copies objects from another document into an update, giving each a
/// new number. References to pages that are not being copied become null,
/// so a link to another page does not drag that page in.
pub(crate) struct Copier<'a> {
    src: &'a Document,
    map: HashMap<ObjRef, ObjRef>,
    /// Pages of the source that are not copied.
    left_out: HashSet<ObjRef>,
    queue: VecDeque<(ObjRef, ObjRef)>,
}

impl<'a> Copier<'a> {
    pub fn new(src: &'a Document, copied: &[usize]) -> Self {
        let pages = src.pages().unwrap_or_default();
        let keep: HashSet<usize> = copied.iter().copied().collect();
        let left_out = pages
            .iter()
            .enumerate()
            .filter(|(i, _)| !keep.contains(i))
            .filter_map(|(_, p)| p.obj_ref)
            .collect();
        Self {
            src,
            map: HashMap::new(),
            left_out,
            queue: VecDeque::new(),
        }
    }

    /// The new number for a source object, reserving one the first time.
    fn target(&mut self, update: &mut Update, r: ObjRef) -> ObjRef {
        if let Some(&t) = self.map.get(&r) {
            return t;
        }
        let t = update.add(Object::Null);
        self.map.insert(r, t);
        self.queue.push_back((r, t));
        t
    }

    /// An object with its references pointing at the copies.
    fn convert(&mut self, update: &mut Update, obj: &Object, depth: usize) -> Object {
        if depth > MAX_DEPTH {
            return Object::Null;
        }
        match obj {
            Object::Reference(r) if self.left_out.contains(r) => Object::Null,
            Object::Reference(r) => Object::Reference(self.target(update, *r)),
            Object::Array(items) => Object::Array(
                items
                    .iter()
                    .map(|o| self.convert(update, o, depth + 1))
                    .collect(),
            ),
            Object::Dict(d) => Object::Dict(self.convert_dict(update, d, depth)),
            Object::Stream(s) => Object::Stream(Stream {
                dict: self.convert_dict(update, &s.dict, depth),
                data: s.data.clone(),
            }),
            other => other.clone(),
        }
    }

    fn convert_dict(&mut self, update: &mut Update, d: &Dict, depth: usize) -> Dict {
        let mut out = Dict::new();
        for (k, v) in d.iter() {
            out.insert(k.clone(), self.convert(update, v, depth + 1));
        }
        out
    }

    /// Copies everything reserved so far, and what that uses in turn.
    fn drain(&mut self, update: &mut Update) -> Result<()> {
        while let Some((src, target)) = self.queue.pop_front() {
            let obj = match self.src.get(src) {
                Ok(o) => o,
                Err(_) => Object::Null,
            };
            let copied = self.convert(update, &obj, 0);
            update.set(target, copied);
        }
        Ok(())
    }

    /// Copies a page as a child of `parent`, with the attributes it
    /// inherits written onto it. Returns the new page's reference.
    pub fn page(&mut self, update: &mut Update, page: &Page, parent: ObjRef) -> Result<ObjRef> {
        // Reserve the page first, so its annotations' /P point at the copy.
        let target = match page.obj_ref {
            Some(r) => self.target(update, r),
            None => update.add(Object::Null),
        };
        // The queued copy of the page itself is replaced below.
        self.queue.retain(|(_, t)| *t != target);
        let mut dict = self.convert_dict(update, &page.dict, 0);
        dict.insert("Parent", Object::Reference(parent));
        inherit(&mut dict, page);
        let resources = self.convert_dict(update, &page.resources, 0);
        dict.insert("Resources", Object::Dict(resources));
        update.set(target, Object::Dict(dict));
        self.drain(update)?;
        Ok(target)
    }
}

/// Writes what a page inherits from the page tree onto the page itself,
/// so it keeps its look when it moves to a new parent.
pub(crate) fn inherit(dict: &mut Dict, page: &Page) {
    let rect = |r: &atelier_syntax::Rect| {
        Object::Array(vec![
            Object::Real(r.x0),
            Object::Real(r.y0),
            Object::Real(r.x1),
            Object::Real(r.y1),
        ])
    };
    if dict.get(b"MediaBox").is_none() {
        dict.insert("MediaBox", rect(&page.media_box));
    }
    if dict.get(b"CropBox").is_none() && page.crop_box != page.media_box {
        dict.insert("CropBox", rect(&page.crop_box));
    }
    if dict.get(b"Rotate").is_none() && page.rotate != 0 {
        dict.insert("Rotate", Object::Integer(i64::from(page.rotate)));
    }
}

/// The catalog's page tree root.
pub(crate) fn pages_root(update: &Update) -> Result<ObjRef> {
    let catalog = update.document().catalog()?;
    match catalog.get(b"Pages") {
        Some(Object::Reference(r)) => Ok(*r),
        _ => Err(atelier_syntax::Error::Structure(
            "the page tree is not an object of its own".into(),
        )),
    }
}

/// Makes `kids` the whole page tree, in order.
pub(crate) fn set_kids(update: &mut Update, root: ObjRef, kids: &[ObjRef]) -> Result<()> {
    let mut dict = match update.get(root)? {
        Object::Dict(d) => d,
        _ => Dict::new(),
    };
    dict.insert("Type", Object::Name(Name::new("Pages")));
    dict.insert(
        "Kids",
        Object::Array(kids.iter().map(|r| Object::Reference(*r)).collect()),
    );
    dict.insert("Count", Object::Integer(kids.len() as i64));
    update.set(root, Object::Dict(dict));
    Ok(())
}
