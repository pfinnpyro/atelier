//! A loaded PDF file with lazy object access.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::decode::{DecodedStream, decode};
use crate::page::{Page, collect_pages};
use crate::parser::find;
use crate::xref::{self, XrefEntry, XrefTable};
use crate::{Dict, Error, Lexer, ObjRef, Object, Parser, Result, Stream, Token};

/// Reference chains and object-stream lookups deeper than this are treated as
/// cycles.
const MAX_RESOLVE_DEPTH: usize = 32;

/// A decoded object stream: its data and where each object starts.
struct ObjectStream {
    data: Vec<u8>,
    /// (object number, absolute offset into `data`).
    offsets: Vec<(u32, usize)>,
}

/// A PDF file. Objects are parsed on first use and cached. `Document` is
/// `Send + Sync`, so page rendering can share one across threads.
pub struct Document {
    data: Arc<[u8]>,
    version: String,
    header_offset: usize,
    entries: HashMap<u32, XrefEntry>,
    trailer: Dict,
    repaired: bool,
    cache: Mutex<HashMap<ObjRef, Object>>,
    object_streams: Mutex<HashMap<u32, Arc<ObjectStream>>>,
    /// Built on demand when an xref offset turns out to be wrong.
    scanned: OnceLock<HashMap<u32, (usize, u16)>>,
    pages: OnceLock<std::result::Result<Vec<Page>, Error>>,
    warnings: Mutex<Vec<String>>,
}

impl Document {
    /// Opens a PDF from its bytes. Broken cross-reference data is repaired by
    /// scanning the file; check [`Document::was_repaired`].
    pub fn open(data: impl Into<Arc<[u8]>>) -> Result<Self> {
        let data: Arc<[u8]> = data.into();
        let header_search = &data[..data.len().min(1024)];
        let (header_offset, version) = match find(header_search, b"%PDF-", 0) {
            Some(pos) => {
                let v: String = data[pos + 5..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit() || **b == b'.')
                    .map(|&b| b as char)
                    .collect();
                (pos, v)
            }
            None => (0, String::new()),
        };

        let mut warnings = Vec::new();
        if version.is_empty() {
            warnings.push("no %PDF- header".to_string());
        }
        if let Ok(table) = xref::read(&data, header_offset) {
            let doc = Self::build(
                data.clone(),
                version.clone(),
                header_offset,
                table,
                false,
                warnings.clone(),
            );
            if doc.catalog().is_ok() {
                return Ok(doc);
            }
            warnings.push("cross-reference data does not lead to a catalog; repairing".into());
        } else {
            warnings.push("cross-reference data is unreadable; repairing".into());
        }
        let table = xref::repair(&data)?;
        let doc = Self::build(data, version, header_offset, table, true, warnings);
        doc.catalog()?;
        Ok(doc)
    }

    fn build(
        data: Arc<[u8]>,
        version: String,
        header_offset: usize,
        table: XrefTable,
        repaired: bool,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            data,
            version,
            header_offset,
            entries: table.entries,
            trailer: table.trailer,
            repaired,
            cache: Mutex::new(HashMap::new()),
            object_streams: Mutex::new(HashMap::new()),
            scanned: OnceLock::new(),
            pages: OnceLock::new(),
            warnings: Mutex::new(warnings),
        }
    }

    /// The version from the `%PDF-x.y` header, or the catalog's /Version if newer.
    pub fn version(&self) -> String {
        let catalog_version = self.catalog().ok().and_then(|c| {
            c.get(b"Version")
                .and_then(Object::as_name)
                .map(|n| n.as_str_lossy().into_owned())
        });
        match catalog_version {
            Some(v) if v > self.version => v,
            _ => self.version.clone(),
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    pub fn was_repaired(&self) -> bool {
        self.repaired
    }

    pub fn is_encrypted(&self) -> bool {
        self.trailer.contains_key(b"Encrypt")
    }

    /// Problems found and worked around while reading.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().map(|w| w.clone()).unwrap_or_default()
    }

    fn warn(&self, message: String) {
        if let Ok(mut w) = self.warnings.lock() {
            if w.len() < 1000 {
                w.push(message);
            }
        }
    }

    /// Object numbers that have an in-use cross-reference entry, sorted.
    pub fn object_numbers(&self) -> Vec<u32> {
        let mut nums: Vec<u32> = self
            .entries
            .iter()
            .filter(|(_, e)| !matches!(e, XrefEntry::Free))
            .map(|(&n, _)| n)
            .collect();
        nums.sort_unstable();
        nums
    }

    pub fn xref_entry(&self, num: u32) -> Option<XrefEntry> {
        self.entries.get(&num).copied()
    }

    /// Loads an indirect object.
    pub fn get(&self, r: ObjRef) -> Result<Object> {
        self.get_depth(r, 0)
    }

    fn get_depth(&self, r: ObjRef, depth: usize) -> Result<Object> {
        if depth > MAX_RESOLVE_DEPTH {
            return Err(Error::TooDeep);
        }
        if let Some(obj) = self.cache.lock().ok().and_then(|c| c.get(&r).cloned()) {
            return Ok(obj);
        }
        let obj = match self.entries.get(&r.num) {
            Some(XrefEntry::Offset { offset, .. }) => self.load_at(r, *offset, depth)?,
            Some(XrefEntry::Compressed { stream, index }) => {
                self.load_compressed(r, *stream, *index, depth)?
            }
            Some(XrefEntry::Free) | None => return Err(Error::MissingObject(r)),
        };
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(r, obj.clone());
        }
        Ok(obj)
    }

    fn load_at(&self, r: ObjRef, offset: usize, depth: usize) -> Result<Object> {
        let length = |lr: ObjRef| self.get_depth(lr, depth + 1).ok().and_then(|o| o.as_i64());
        let parse = |offset: usize| -> Option<Object> {
            let (found, obj) = Parser::at(&self.data, offset)
                .parse_indirect(&length)
                .ok()?;
            (found.num == r.num).then_some(obj)
        };
        if let Some(obj) = parse(offset) {
            return Ok(obj);
        }
        // The offset is wrong. Try a header shift, then a full scan.
        if self.header_offset > 0 {
            if let Some(obj) = parse(offset + self.header_offset) {
                return Ok(obj);
            }
        }
        let scanned = self.scanned.get_or_init(|| xref::scan_objects(&self.data));
        if let Some(&(real, _)) = scanned.get(&r.num) {
            if let Some(obj) = parse(real) {
                self.warn(format!(
                    "object {} had a wrong offset ({offset}, really {real})",
                    r.num
                ));
                return Ok(obj);
            }
        }
        Err(Error::MissingObject(r))
    }

    fn load_compressed(&self, r: ObjRef, stream: u32, index: u32, depth: usize) -> Result<Object> {
        let objstm = self.object_stream(stream, depth + 1)?;
        let offset = objstm
            .offsets
            .get(index as usize)
            .filter(|(num, _)| *num == r.num)
            .or_else(|| objstm.offsets.iter().find(|(num, _)| *num == r.num))
            .map(|&(_, off)| off)
            .ok_or(Error::MissingObject(r))?;
        Parser::at(&objstm.data, offset).parse_object()
    }

    fn object_stream(&self, num: u32, depth: usize) -> Result<Arc<ObjectStream>> {
        if let Some(s) = self
            .object_streams
            .lock()
            .ok()
            .and_then(|m| m.get(&num).cloned())
        {
            return Ok(s);
        }
        let generation = match self.entries.get(&num) {
            Some(XrefEntry::Offset { generation, .. }) => *generation,
            _ => 0,
        };
        let obj = self.get_depth(ObjRef::new(num, generation), depth)?;
        let Object::Stream(stream) = obj else {
            return Err(Error::WrongType {
                expected: "object stream",
                found: obj.type_name(),
            });
        };
        let decoded = self.decode_depth(&stream, depth)?.data;
        let n = stream
            .dict
            .get(b"N")
            .and_then(Object::as_i64)
            .unwrap_or(0)
            .max(0);
        let first = stream
            .dict
            .get(b"First")
            .and_then(Object::as_i64)
            .and_then(|f| usize::try_from(f).ok())
            .unwrap_or(0);
        let mut offsets = Vec::new();
        let mut lx = Lexer::new(&decoded);
        for _ in 0..n {
            let (Ok(Some(Token::Integer(obj_num))), Ok(Some(Token::Integer(off)))) =
                (lx.next_token(), lx.next_token())
            else {
                break;
            };
            if let (Ok(obj_num), Ok(off)) = (u32::try_from(obj_num), usize::try_from(off)) {
                offsets.push((obj_num, first.saturating_add(off)));
            }
        }
        let parsed = Arc::new(ObjectStream {
            data: decoded,
            offsets,
        });
        if let Ok(mut m) = self.object_streams.lock() {
            m.insert(num, parsed.clone());
        }
        Ok(parsed)
    }

    /// Follows references until a direct object. A reference to a missing object
    /// is null, as the spec requires.
    pub fn resolve(&self, obj: &Object) -> Result<Object> {
        let mut current = obj.clone();
        for depth in 0..MAX_RESOLVE_DEPTH {
            let Object::Reference(r) = current else {
                return Ok(current);
            };
            current = match self.get_depth(r, depth) {
                Ok(o) => o,
                Err(Error::MissingObject(_)) => Object::Null,
                Err(e) => return Err(e),
            };
        }
        Err(Error::TooDeep)
    }

    /// Looks up `key` in `dict` and resolves it. Missing keys are `Null`.
    pub fn get_key(&self, dict: &Dict, key: &[u8]) -> Result<Object> {
        match dict.get(key) {
            Some(v) => self.resolve(v),
            None => Ok(Object::Null),
        }
    }

    /// Resolves `obj` and requires a dictionary (or a stream's dictionary).
    pub fn resolve_dict(&self, obj: &Object) -> Result<Dict> {
        let resolved = self.resolve(obj)?;
        match resolved {
            Object::Dict(d) => Ok(d),
            Object::Stream(s) => Ok(s.dict),
            other => Err(Error::WrongType {
                expected: "dictionary",
                found: other.type_name(),
            }),
        }
    }

    /// The document catalog (/Root).
    pub fn catalog(&self) -> Result<Dict> {
        let root = self
            .trailer
            .get(b"Root")
            .ok_or_else(|| Error::Structure("trailer has no /Root".into()))?;
        let dict = self.resolve_dict(root)?;
        Ok(dict)
    }

    /// The document information dictionary (/Info), if any.
    pub fn info(&self) -> Option<Dict> {
        self.trailer
            .get(b"Info")
            .and_then(|i| self.resolve_dict(i).ok())
    }

    /// All pages in order.
    pub fn pages(&self) -> Result<&[Page]> {
        match self.pages.get_or_init(|| collect_pages(self)) {
            Ok(p) => Ok(p),
            Err(e) => Err(e.clone()),
        }
    }

    /// Decodes a stream's filter chain. Image codecs (DCT, JPX, JBIG2, CCITT) are
    /// left in place and reported in [`DecodedStream::image_filter`].
    pub fn decode_stream(&self, stream: &Stream) -> Result<DecodedStream> {
        self.decode_depth(stream, 0)
    }

    fn decode_depth(&self, stream: &Stream, depth: usize) -> Result<DecodedStream> {
        if self.is_encrypted() && !stream.dict.is_type(b"XRef") {
            return Err(Error::Encrypted);
        }
        let resolve = |o: &Object| match o {
            Object::Reference(r) => self.get_depth(*r, depth + 1).unwrap_or(Object::Null),
            other => other.clone(),
        };
        let mut warnings = Vec::new();
        let result = decode(&stream.data, &stream.dict, &resolve, &mut |w| {
            warnings.push(w)
        });
        for w in warnings {
            self.warn(w);
        }
        result
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("version", &self.version)
            .field("objects", &self.entries.len())
            .field("repaired", &self.repaired)
            .finish_non_exhaustive()
    }
}
