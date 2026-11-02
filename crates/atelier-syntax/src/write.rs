//! Saving (ISO 32000-2, 7.5.6): changed and new objects are appended to
//! the original bytes as an incremental update, so everything already in
//! the file, signatures included, stays byte for byte. Files whose
//! cross-reference data had to be repaired are written out whole instead.

use std::collections::BTreeMap;

use atelier_crypto::md5;

use crate::xref::XrefEntry;
use crate::{Dict, Document, Error, Name, ObjRef, Object, PdfString, Result};

/// Edits to a document, written with [`Update::to_bytes`].
pub struct Update<'a> {
    doc: &'a Document,
    objects: BTreeMap<u32, (u16, Object)>,
    next: u32,
}

impl Document {
    /// Starts a set of edits to this document.
    pub fn update(&self) -> Update<'_> {
        Update {
            doc: self,
            objects: BTreeMap::new(),
            next: self.next_object_number(),
        }
    }
}

impl<'a> Update<'a> {
    pub fn document(&self) -> &'a Document {
        self.doc
    }

    /// Exports a fresh AES-256 revision 6 encrypted PDF. Does not mutate the source.
    /// Existing encrypted files and signature fields are rejected in this first pass.
    pub fn to_encrypted_bytes(mut self, user: &str, owner: &str) -> Result<Vec<u8>> {
        if self.doc.is_encrypted() {
            return Err(Error::Unsupported("changing existing encryption".into()));
        }
        for n in self.doc.object_numbers() {
            let generation = match self.doc.xref_entry(n) {
                Some(XrefEntry::Offset { generation, .. }) => generation,
                _ => 0,
            };
            let obj = self.doc.get(ObjRef::new(n, generation))?;
            let dict = match &obj {
                Object::Dict(d) => Some(d),
                Object::Stream(s) => Some(&s.dict),
                _ => None,
            };
            if let Some(d) = dict {
                if d.is_type(b"Sig")
                    || d.get(b"FT")
                        .and_then(Object::as_name)
                        .is_some_and(|n| *n == b"Sig")
                {
                    return Err(Error::Unsupported(
                        "encrypting a document with signature fields would invalidate signatures"
                            .into(),
                    ));
                }
                if matches!(obj, Object::Stream(_)) && crate::document::has_identity_crypt_filter(d)
                {
                    return Err(Error::Unsupported(
                        "encrypting streams with an explicit Identity crypt filter".into(),
                    ));
                }
            }
        }
        let r = ObjRef::new(self.next, 0);
        let (security, dict) = crate::crypt::Security::create(user, owner, r)?;
        self.add(Object::Dict(dict));
        self.write_whole_secured(None, Some(&security), Some(r))
    }

    /// Replaces object `r`.
    pub fn set(&mut self, r: ObjRef, obj: Object) {
        self.objects.insert(r.num, (r.generation, obj));
        self.next = self.next.max(r.num + 1);
    }

    /// Adds a new object and returns its reference.
    pub fn add(&mut self, obj: Object) -> ObjRef {
        let r = ObjRef::new(self.next, 0);
        self.set(r, obj);
        r
    }

    /// Object `r` as edited, or as in the file.
    pub fn get(&self, r: ObjRef) -> Result<Object> {
        match self.objects.get(&r.num) {
            Some((_, obj)) => Ok(obj.clone()),
            None => self.doc.get(r),
        }
    }

    /// Follows references through the edits.
    pub fn resolve(&self, obj: &Object) -> Result<Object> {
        let mut current = obj.clone();
        for _ in 0..32 {
            let Object::Reference(r) = current else {
                return Ok(current);
            };
            current = match self.get(r) {
                Ok(o) => o,
                Err(Error::MissingObject(_)) => Object::Null,
                Err(e) => return Err(e),
            };
        }
        Err(Error::TooDeep)
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// The edited objects, by reference.
    pub fn changed(&self) -> impl Iterator<Item = (ObjRef, &Object)> {
        self.objects
            .iter()
            .map(|(&num, (generation, obj))| (ObjRef::new(num, *generation), obj))
    }

    /// The file with the edits: an incremental update, or a full rewrite
    /// when the original's cross-reference data was broken.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        match self.doc.startxref() {
            Some(prev) if !self.doc.was_repaired() => self.incremental(prev),
            _ => self.full(),
        }
    }

    /// Appends the edits after the original bytes.
    fn incremental(&self, prev: usize) -> Result<Vec<u8>> {
        let mut out = self.doc.data().to_vec();
        if !matches!(out.last(), Some(b'\n' | b'\r')) {
            out.push(b'\n');
        }
        let mut offsets = BTreeMap::new();
        for (r, obj) in self.changed() {
            offsets.insert(r.num, (out.len(), r.generation));
            write_object(&mut out, r, obj, self.doc)?;
        }
        let mut size = self.next.max(self.doc.next_object_number());
        let mut trailer = Dict::new();
        for key in [b"Root".as_slice(), b"Info", b"Encrypt"] {
            if let Some(v) = self.doc.trailer().get(key) {
                trailer.insert(Name::new(key), v.clone());
            }
        }
        if let Some(id) = new_id(self.doc, &out) {
            trailer.insert("ID", id);
        }
        trailer.insert("Prev", Object::Integer(prev as i64));
        let xref_at = out.len();
        if self.doc.trailer().is_type(b"XRef") {
            // The file uses cross-reference streams, so the update does too.
            let own = size;
            size += 1;
            offsets.insert(own, (xref_at, 0));
            trailer.insert("Size", Object::Integer(i64::from(size)));
            write_xref_stream(&mut out, own, &offsets, trailer);
        } else {
            trailer.insert("Size", Object::Integer(i64::from(size)));
            write_xref_table(&mut out, &offsets, false);
            out.extend_from_slice(format!("trailer\n{trailer}\n").as_bytes());
        }
        out.extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());
        Ok(out)
    }

    /// The file with the edits, written out whole with only the objects
    /// the document still uses, so removed pages, and anything only they
    /// used, are gone from the bytes, which an incremental update cannot
    /// do. References to `leave_out` (such as a link's target page that
    /// was removed) are not followed, so they do not keep it in.
    pub fn to_compact_bytes(
        &self,
        leave_out: &std::collections::HashSet<ObjRef>,
    ) -> Result<Vec<u8>> {
        let mut seen = std::collections::HashSet::new();
        let mut todo: Vec<ObjRef> = Vec::new();
        for key in [b"Root".as_slice(), b"Info", b"Encrypt"] {
            if let Some(Object::Reference(r)) = self.doc.trailer().get(key) {
                todo.push(*r);
            }
        }
        while let Some(r) = todo.pop() {
            if leave_out.contains(&r) || !seen.insert(r.num) {
                continue;
            }
            let obj = match self.get(r) {
                Ok(o) => o,
                Err(Error::MissingObject(_)) => continue,
                Err(e) => return Err(e),
            };
            refs_in(&obj, &mut todo, 0);
        }
        self.write_whole(Some(&seen))
    }

    /// Writes every object again, with a fresh cross-reference table.
    fn full(&self) -> Result<Vec<u8>> {
        self.write_whole(None)
    }

    /// Writes the objects (all, or those in `keep`) with a fresh
    /// cross-reference table.
    fn write_whole(&self, keep: Option<&std::collections::HashSet<u32>>) -> Result<Vec<u8>> {
        self.write_whole_secured(keep, self.doc.security(), None)
    }

    fn write_whole_secured(
        &self,
        keep: Option<&std::collections::HashSet<u32>>,
        security: Option<&crate::crypt::Security>,
        encrypt: Option<ObjRef>,
    ) -> Result<Vec<u8>> {
        let doc = self.doc;
        let version = if encrypt.is_some() {
            "2.0".to_owned()
        } else {
            doc.version().to_string()
        };
        let mut out = format!("%PDF-{}\n%\u{e2}\u{e3}\u{cf}\u{d3}\n", version).into_bytes();
        let mut offsets = BTreeMap::new();
        let mut nums: Vec<u32> = doc.object_numbers();
        nums.extend(self.objects.keys());
        nums.sort_unstable();
        nums.dedup();
        nums.retain(|n| keep.is_none_or(|k| k.contains(n)));
        for num in nums {
            let (r, obj) = match self.objects.get(&num) {
                Some((generation, obj)) => (ObjRef::new(num, *generation), obj.clone()),
                None => {
                    let generation = match doc.xref_entry(num) {
                        Some(XrefEntry::Offset { generation, .. }) => generation,
                        Some(XrefEntry::Compressed { .. }) => 0,
                        _ => continue,
                    };
                    let r = ObjRef::new(num, generation);
                    let Ok(obj) = doc.get(r) else { continue };
                    (r, obj)
                }
            };
            // Cross-reference and object streams are rebuilt, not copied.
            if let Object::Stream(s) = &obj {
                if s.dict.is_type(b"XRef") || s.dict.is_type(b"ObjStm") {
                    continue;
                }
            }
            offsets.insert(num, (out.len(), r.generation));
            write_object_with_security(&mut out, r, &obj, security)?;
        }
        let size = offsets.keys().max().map_or(1, |m| m + 1);
        let mut trailer = Dict::new();
        trailer.insert("Size", Object::Integer(i64::from(size)));
        for key in [b"Root".as_slice(), b"Info", b"Encrypt", b"ID"] {
            if let Some(v) = doc.trailer().get(key) {
                trailer.insert(Name::new(key), v.clone());
            }
        }
        if let Some(r) = encrypt {
            trailer.insert("Encrypt", Object::Reference(r));
        }
        let xref_at = out.len();
        write_xref_table(&mut out, &offsets, true);
        out.extend_from_slice(
            format!("trailer\n{trailer}\nstartxref\n{xref_at}\n%%EOF\n").as_bytes(),
        );
        Ok(out)
    }
}

/// Collects the references inside an object.
fn refs_in(obj: &Object, out: &mut Vec<ObjRef>, depth: usize) {
    if depth > 64 {
        return;
    }
    match obj {
        Object::Reference(r) => out.push(*r),
        Object::Array(items) => items.iter().for_each(|o| refs_in(o, out, depth + 1)),
        Object::Dict(d) => d.iter().for_each(|(_, o)| refs_in(o, out, depth + 1)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, o)| refs_in(o, out, depth + 1)),
        _ => {}
    }
}

/// The file identifier for a new revision: the original's first part, and
/// a second part that changes with the content.
fn new_id(doc: &Document, content: &[u8]) -> Option<Object> {
    let first = match doc.trailer().get(b"ID") {
        Some(Object::Array(a)) => a.first().cloned()?,
        _ => return None,
    };
    let second = PdfString {
        bytes: md5(content).to_vec(),
        hex: true,
    };
    Some(Object::Array(vec![first, Object::String(second)]))
}

/// Writes `r` as an indirect object, encrypted when the file is.
fn write_object(out: &mut Vec<u8>, r: ObjRef, obj: &Object, doc: &Document) -> Result<()> {
    write_object_with_security(out, r, obj, doc.security())
}

fn write_object_with_security(
    out: &mut Vec<u8>,
    r: ObjRef,
    obj: &Object,
    security: Option<&crate::crypt::Security>,
) -> Result<()> {
    let mut obj = obj.clone();
    if let Some(security) = security {
        security.encrypt_object(r, &mut obj)?;
    }
    out.extend_from_slice(format!("{} {} obj\n", r.num, r.generation).as_bytes());
    serialize(out, &obj);
    out.extend_from_slice(b"\nendobj\n");
    Ok(())
}

/// Writes an object in PDF syntax. A stream's /Length is set to its data.
pub fn serialize(out: &mut Vec<u8>, obj: &Object) {
    match obj {
        Object::Stream(s) => {
            let mut dict = s.dict.clone();
            dict.insert("Length", Object::Integer(s.data.len() as i64));
            out.extend_from_slice(format!("{dict}\nstream\n").as_bytes());
            out.extend_from_slice(&s.data);
            out.extend_from_slice(b"\nendstream");
        }
        other => out.extend_from_slice(other.to_string().as_bytes()),
    }
}

/// A classic cross-reference table, in subsections of consecutive numbers.
fn write_xref_table(
    out: &mut Vec<u8>,
    offsets: &BTreeMap<u32, (usize, u16)>,
    with_free_head: bool,
) {
    out.extend_from_slice(b"xref\n");
    let mut entries: Vec<(u32, String)> = offsets
        .iter()
        .map(|(&num, &(off, generation))| (num, format!("{off:010} {generation:05} n\r\n")))
        .collect();
    // Object 0 heads the free list; only a full table lists it.
    if with_free_head {
        entries.insert(0, (0, "0000000000 65535 f\r\n".to_string()));
    }
    let mut i = 0;
    while i < entries.len() {
        let start = entries[i].0;
        let mut j = i + 1;
        while j < entries.len() && entries[j].0 == entries[j - 1].0 + 1 {
            j += 1;
        }
        out.extend_from_slice(format!("{start} {}\n", j - i).as_bytes());
        for (_, line) in &entries[i..j] {
            out.extend_from_slice(line.as_bytes());
        }
        i = j;
    }
}

/// A cross-reference stream (uncompressed) listing `offsets`, which
/// includes the stream itself as object `own`.
fn write_xref_stream(
    out: &mut Vec<u8>,
    own: u32,
    offsets: &BTreeMap<u32, (usize, u16)>,
    mut dict: Dict,
) {
    let mut index = Vec::new();
    let mut data = Vec::new();
    let nums: Vec<u32> = offsets.keys().copied().collect();
    let mut i = 0;
    while i < nums.len() {
        let mut j = i + 1;
        while j < nums.len() && nums[j] == nums[j - 1] + 1 {
            j += 1;
        }
        index.push(Object::Integer(i64::from(nums[i])));
        index.push(Object::Integer((j - i) as i64));
        for num in &nums[i..j] {
            let (off, generation) = offsets[num];
            data.push(1u8);
            data.extend_from_slice(&(off as u32).to_be_bytes());
            data.extend_from_slice(&generation.to_be_bytes());
        }
        i = j;
    }
    dict.insert("Type", Object::Name(Name::from("XRef")));
    dict.insert("W", Object::Array(vec![1.into(), 4.into(), 2.into()]));
    dict.insert("Index", Object::Array(index));
    let stream = Object::Stream(crate::Stream {
        dict,
        data: data.into(),
    });
    out.extend_from_slice(format!("{own} 0 obj\n").as_bytes());
    serialize(out, &stream);
    out.extend_from_slice(b"\nendobj\n");
}
