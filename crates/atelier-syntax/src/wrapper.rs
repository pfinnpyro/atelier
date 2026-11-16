//! Unencrypted wrapper documents (ISO 32000-2, 7.6.7): an ordinary,
//! readable PDF that carries a whole encrypted document as an embedded
//! file, with a page saying what it is and how to open it.
//!
//! This is how a document protected by something other than the standard
//! security handler travels: a reader that does not know the scheme shows
//! the cover page and not an error, and one that does opens the payload.
//! Nothing of the real document is in the wrapper's own objects.
//!
//! This module only packs and unpacks. What the payload is and how it is
//! decrypted is for whoever asks, by the name of its cryptographic filter.

use std::collections::HashSet;

use atelier_crypto::md5;

use crate::{Dict, Document, Name, ObjRef, Object, PdfString, Result, Stream, serialize};

/// The cryptographic filter name of a payload that is a binary OpenPGP
/// message (RFC 9580) whose content is the complete PDF file.
pub const OPENPGP_PAYLOAD: &str = "AtelierOpenPGP";

/// The version of that arrangement.
pub const OPENPGP_PAYLOAD_VERSION: &str = "1";

/// The encrypted document a wrapper carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedPayload {
    /// The name of the cryptographic filter that made it.
    pub filter: String,
    /// The filter's version, if the wrapper gives one.
    pub version: Option<String>,
    /// The embedded file's name.
    pub name: String,
    /// The embedded file: the encrypted document.
    pub data: Vec<u8>,
}

/// What goes on the cover page.
#[derive(Debug, Clone, Default)]
pub struct Cover<'a> {
    pub title: &'a str,
    /// Lines of text under the title; an empty one leaves a gap.
    pub lines: &'a [&'a str],
}

/// A PDF string for a page's content stream, in the standard Latin
/// encoding the cover's font uses. Characters outside it become `?`.
fn shown(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

fn name(n: &str) -> Object {
    Object::Name(Name::from(n))
}

fn dict(entries: Vec<(&str, Object)>) -> Dict {
    let mut d = Dict::new();
    for (k, v) in entries {
        d.insert(k, v);
    }
    d
}

fn text(s: &str) -> Object {
    Object::String(PdfString::from_text(s))
}

fn reference(num: u32) -> Object {
    Object::Reference(ObjRef::new(num, 0))
}

/// Builds a wrapper document around `payload`, which `filter` (a
/// cryptographic filter name such as [`OPENPGP_PAYLOAD`]) produced.
/// `file_name` is what the embedded file is called.
pub fn wrap_encrypted_payload(
    payload: &[u8],
    filter: &str,
    version: &str,
    file_name: &str,
    cover: &Cover,
) -> Vec<u8> {
    let mut content = format!("BT /F1 20 Tf 72 700 Td {} Tj ET\n", shown(cover.title));
    content.push_str("BT /F1 11 Tf 16 TL 72 664 Td\n");
    for line in cover.lines {
        content.push_str(&format!("{} Tj T*\n", shown(line)));
    }
    content.push_str("ET\n");

    let catalog = dict(vec![
        ("Type", name("Catalog")),
        ("Pages", reference(2)),
        (
            "Names",
            Object::Dict(dict(vec![(
                "EmbeddedFiles",
                Object::Dict(dict(vec![(
                    "Names",
                    Object::Array(vec![text(file_name), reference(6)]),
                )])),
            )])),
        ),
        // A hidden collection: readers that know of wrappers go straight
        // to the payload, and the rest show the cover page.
        (
            "Collection",
            Object::Dict(dict(vec![
                ("Type", name("Collection")),
                ("View", name("H")),
                ("D", text(file_name)),
            ])),
        ),
        ("AF", Object::Array(vec![reference(6)])),
    ]);
    let pages = dict(vec![
        ("Type", name("Pages")),
        ("Kids", Object::Array(vec![reference(3)])),
        ("Count", Object::Integer(1)),
    ]);
    let page = dict(vec![
        ("Type", name("Page")),
        ("Parent", reference(2)),
        (
            "MediaBox",
            Object::Array(vec![0.into(), 0.into(), 612.into(), 792.into()]),
        ),
        (
            "Resources",
            Object::Dict(dict(vec![(
                "Font",
                Object::Dict(dict(vec![("F1", reference(5))])),
            )])),
        ),
        ("Contents", reference(4)),
    ]);
    let font = dict(vec![
        ("Type", name("Font")),
        ("Subtype", name("Type1")),
        ("BaseFont", name("Helvetica")),
        ("Encoding", name("WinAnsiEncoding")),
    ]);
    let file_spec = dict(vec![
        ("Type", name("Filespec")),
        ("F", text(file_name)),
        ("UF", text(file_name)),
        ("AFRelationship", name("EncryptedPayload")),
        ("EF", Object::Dict(dict(vec![("F", reference(7))]))),
        (
            "EP",
            Object::Dict(dict(vec![
                ("Type", name("EncryptedPayload")),
                ("Subtype", name(filter)),
                ("Version", name(version)),
            ])),
        ),
    ]);
    let embedded = dict(vec![
        ("Type", name("EmbeddedFile")),
        (
            "Params",
            Object::Dict(dict(vec![("Size", Object::Integer(payload.len() as i64))])),
        ),
    ]);
    let objects = [
        Object::Dict(catalog),
        Object::Dict(pages),
        Object::Dict(page),
        Object::Stream(Stream {
            dict: Dict::new(),
            data: content.into_bytes().into(),
        }),
        Object::Dict(font),
        Object::Dict(file_spec),
        Object::Stream(Stream {
            dict: embedded,
            data: payload.into(),
        }),
    ];

    let mut out = b"%PDF-2.0\n%\xE2\xE3\xCF\xD3\n".to_vec();
    out.reserve(payload.len() + 4096);
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        serialize(&mut out, obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f\r\n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
    }
    // The payload is random to look at, so its hash serves as the file
    // identifier; nothing secret goes into it.
    let id = Object::String(PdfString {
        bytes: md5(payload).to_vec(),
        hex: true,
    });
    let trailer = dict(vec![
        ("Size", Object::Integer(objects.len() as i64 + 1)),
        ("Root", reference(1)),
        ("ID", Object::Array(vec![id.clone(), id])),
    ]);
    out.extend_from_slice(format!("trailer\n{trailer}\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    out
}

impl Document {
    /// The encrypted document this one wraps, if it is a wrapper: the
    /// embedded file whose file specification has an encrypted payload
    /// dictionary.
    pub fn encrypted_payload(&self) -> Result<Option<EncryptedPayload>> {
        let catalog = self.catalog()?;
        let Ok(Object::Dict(names)) = self.get_key(&catalog, b"Names") else {
            return Ok(None);
        };
        let Ok(Object::Dict(tree)) = self.get_key(&names, b"EmbeddedFiles") else {
            return Ok(None);
        };
        let mut specs = Vec::new();
        self.name_tree_values(&tree, &mut specs, &mut HashSet::new(), 0);
        for spec in specs {
            let Ok(Object::Dict(spec)) = self.resolve(&spec) else {
                continue;
            };
            let Ok(Object::Dict(ep)) = self.get_key(&spec, b"EP") else {
                continue;
            };
            let Ok(Object::Name(filter)) = self.get_key(&ep, b"Subtype") else {
                continue;
            };
            let version = match self.get_key(&ep, b"Version") {
                Ok(Object::Name(n)) => Some(n.as_str_lossy().into_owned()),
                Ok(Object::String(s)) => Some(s.to_text()),
                _ => None,
            };
            let file_name = [b"UF".as_slice(), b"F"]
                .into_iter()
                .find_map(|k| match self.get_key(&spec, k) {
                    Ok(Object::String(s)) => Some(s.to_text()),
                    _ => None,
                })
                .unwrap_or_default();
            let Ok(Object::Dict(files)) = self.get_key(&spec, b"EF") else {
                continue;
            };
            let stream =
                [b"UF".as_slice(), b"F"]
                    .into_iter()
                    .find_map(|k| match self.get_key(&files, k) {
                        Ok(Object::Stream(s)) => Some(s),
                        _ => None,
                    });
            let Some(stream) = stream else { continue };
            return Ok(Some(EncryptedPayload {
                filter: filter.as_str_lossy().into_owned(),
                version,
                name: file_name,
                data: self.decode_stream(&stream)?.data.to_vec(),
            }));
        }
        Ok(None)
    }

    /// Collects the values of a name tree, which is small here, without
    /// following a node twice.
    fn name_tree_values(
        &self,
        node: &Dict,
        out: &mut Vec<Object>,
        seen: &mut HashSet<ObjRef>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        if let Ok(Object::Array(names)) = self.get_key(node, b"Names") {
            out.extend(names.chunks_exact(2).map(|pair| pair[1].clone()));
        }
        let Ok(Object::Array(kids)) = self.get_key(node, b"Kids") else {
            return;
        };
        for kid in kids {
            if let Object::Reference(r) = kid {
                if !seen.insert(r) {
                    continue;
                }
            }
            if let Ok(Object::Dict(kid)) = self.resolve(&kid) {
                self.name_tree_values(&kid, out, seen, depth + 1);
            }
        }
    }
}
