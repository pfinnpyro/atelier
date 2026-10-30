//! Files embedded in a PDF (ISO 32000-2, 7.11.4 and 14.13): the
//! /EmbeddedFiles name tree, file attachment annotations and associated
//! files. They are listed, checked and saved out on request; nothing here
//! opens or runs them.

use std::collections::HashSet;

use atelier_syntax::{Dict, Document, ObjRef, Object, Result};

/// Where an attachment is listed.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// The document's /EmbeddedFiles name tree, under this key.
    Document { key: String },
    /// A file attachment annotation on a page (index).
    Annotation { page: usize },
    /// An associated file (/AF) of the document or a page.
    Associated { page: Option<usize> },
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Self::Document { .. } => "Document".into(),
            Self::Annotation { page } => format!("Page {} attachment", page + 1),
            Self::Associated { page: None } => "Associated with the document".into(),
            Self::Associated { page: Some(p) } => format!("Associated with page {}", p + 1),
        }
    }
}

/// An embedded file.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// The file name it carries (/UF, else /F, else the tree key).
    pub name: String,
    pub source: Source,
    /// /Desc, or the annotation's /Contents.
    pub description: Option<String>,
    /// The MIME type it declares (/Subtype).
    pub mime: Option<String>,
    /// The size it declares (/Params /Size).
    pub declared_size: Option<u64>,
    /// /Params /CreationDate and /ModDate, as written.
    pub created: Option<String>,
    pub modified: Option<String>,
    /// The MD5 it declares (/Params /CheckSum).
    pub checksum: Option<Vec<u8>>,
    /// /AFRelationship: Source, Data, Alternative, Supplement...
    pub relationship: Option<String>,
    /// The file specification dictionary.
    pub filespec: Option<ObjRef>,
    /// The embedded file stream; None when the file is only referred to.
    pub stream: Option<ObjRef>,
}

const MAX_DEPTH: usize = 32;
const MAX_FILES: usize = 10_000;

/// Every embedded file, the name tree first, then page attachments and
/// associated files. A file listed in several places is listed once.
pub fn attachments(doc: &Document) -> Vec<Attachment> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let Ok(catalog) = doc.catalog() else {
        return out;
    };
    if let Ok(Object::Dict(names)) = doc.get_key(&catalog, b"Names") {
        if let Ok(Object::Dict(tree)) = doc.get_key(&names, b"EmbeddedFiles") {
            let mut entries = Vec::new();
            name_tree(doc, &tree, &mut entries, 0);
            for (key, spec) in entries {
                push(
                    doc,
                    &spec,
                    Source::Document { key },
                    None,
                    &mut out,
                    &mut seen,
                );
            }
        }
    }
    if let Ok(pages) = doc.pages() {
        for (i, page) in pages.iter().enumerate() {
            let Ok(annots) = doc.get_key(&page.dict, b"Annots") else {
                continue;
            };
            for a in annots.as_array().unwrap_or_default() {
                let Ok(Object::Dict(annot)) = doc.resolve(a) else {
                    continue;
                };
                let subtype = doc.get_key(&annot, b"Subtype").ok();
                if subtype
                    .as_ref()
                    .and_then(Object::as_name)
                    .map(|n| n.as_bytes())
                    != Some(b"FileAttachment")
                {
                    continue;
                }
                if let Some(fs) = annot.get(b"FS") {
                    let contents = text(doc, &annot, b"Contents");
                    push(
                        doc,
                        fs,
                        Source::Annotation { page: i },
                        contents,
                        &mut out,
                        &mut seen,
                    );
                }
            }
            associated(doc, &page.dict, Some(i), &mut out, &mut seen);
        }
    }
    associated(doc, &catalog, None, &mut out, &mut seen);
    out
}

fn associated(
    doc: &Document,
    dict: &Dict,
    page: Option<usize>,
    out: &mut Vec<Attachment>,
    seen: &mut HashSet<ObjRef>,
) {
    if let Ok(Object::Array(af)) = doc.get_key(dict, b"AF") {
        for spec in af {
            push(doc, &spec, Source::Associated { page }, None, out, seen);
        }
    }
}

fn push(
    doc: &Document,
    spec: &Object,
    source: Source,
    note: Option<String>,
    out: &mut Vec<Attachment>,
    seen: &mut HashSet<ObjRef>,
) {
    if out.len() >= MAX_FILES {
        return;
    }
    let filespec = spec.as_reference();
    let dict = match doc.resolve(spec) {
        Ok(Object::Dict(d)) => d,
        // A bare string names a file outside the PDF: nothing embedded.
        _ => return,
    };
    let ef = doc
        .get_key(&dict, b"EF")
        .ok()
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    let stream = ef
        .get(b"UF")
        .or_else(|| ef.get(b"F"))
        .and_then(Object::as_reference);
    let key = stream.or(filespec);
    if let Some(k) = key {
        if !seen.insert(k) {
            return;
        }
    }
    let fallback = match &source {
        Source::Document { key } if !key.is_empty() => key.clone(),
        _ => "(unnamed)".into(),
    };
    let name = text(doc, &dict, b"UF")
        .or_else(|| text(doc, &dict, b"F"))
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback);
    let mut a = Attachment {
        name,
        source,
        description: text(doc, &dict, b"Desc").or(note),
        mime: None,
        declared_size: None,
        created: None,
        modified: None,
        checksum: None,
        relationship: doc
            .get_key(&dict, b"AFRelationship")
            .ok()
            .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned())),
        filespec,
        stream,
    };
    if let Some(Ok(Object::Stream(s))) = stream.map(|r| doc.get(r)) {
        a.mime = doc
            .get_key(&s.dict, b"Subtype")
            .ok()
            .and_then(|o| o.as_name().map(|n| n.as_str_lossy().into_owned()));
        if let Ok(Object::Dict(params)) = doc.get_key(&s.dict, b"Params") {
            a.declared_size = doc
                .get_key(&params, b"Size")
                .ok()
                .and_then(|o| o.as_i64())
                .and_then(|n| u64::try_from(n).ok());
            a.created = text(doc, &params, b"CreationDate").map(|d| pdf_date(&d));
            a.modified = text(doc, &params, b"ModDate").map(|d| pdf_date(&d));
            a.checksum = match doc.get_key(&params, b"CheckSum") {
                Ok(Object::String(s)) => Some(s.bytes.clone()),
                _ => None,
            };
        }
    }
    out.push(a);
}

/// The file's decoded contents.
pub fn attachment_data(doc: &Document, a: &Attachment) -> Result<Vec<u8>> {
    let Some(r) = a.stream else {
        return Err(atelier_syntax::Error::Structure(
            "the file is referred to, not embedded".into(),
        ));
    };
    match doc.get(r)? {
        Object::Stream(s) => Ok(doc.decode_stream(&s)?.data),
        _ => Err(atelier_syntax::Error::Structure(
            "the embedded file is not a stream".into(),
        )),
    }
}

/// What the contents of a file are and whether they deserve care.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub size: usize,
    pub sha256: String,
    /// Whether the contents match the MD5 the file declares, when it
    /// declares one.
    pub checksum_ok: Option<bool>,
    /// What the bytes look like: "PDF document", "Windows program"...
    pub kind: &'static str,
    /// Reasons to be careful with the file, for people. Empty when none.
    pub cautions: Vec<String>,
}

/// Looks at a file's contents, without opening them.
pub fn check(a: &Attachment, data: &[u8]) -> Checked {
    let (kind, risky) = sniff(data);
    let mut cautions = Vec::new();
    if risky {
        cautions.push(format!("The contents look like {}.", with_article(kind)));
    }
    let ext = a
        .name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if RISKY_EXTENSIONS.contains(&ext.as_str()) {
        cautions.push(format!("Files named .{ext} can run code when opened."));
    }
    if let Some(expected) = expected_kind(&ext) {
        if kind != expected && kind != "binary data" && kind != "text" {
            cautions.push(format!(
                "Its name ends in .{ext}, but the contents look like {}.",
                with_article(kind)
            ));
        }
    }
    if let Some(size) = a.declared_size {
        if size != data.len() as u64 {
            cautions.push(format!(
                "It says it is {size} bytes, but holds {}.",
                data.len()
            ));
        }
    }
    let checksum_ok = a
        .checksum
        .as_ref()
        .filter(|c| c.len() == 16)
        .map(|c| c[..] == atelier_crypto::md5(data)[..]);
    if checksum_ok == Some(false) {
        cautions.push("Its checksum does not match its contents.".into());
    }
    Checked {
        size: data.len(),
        sha256: atelier_crypto::sha256(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        checksum_ok,
        kind,
        cautions,
    }
}

const RISKY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "scr", "com", "pif", "cpl", "msi", "msp", "bat", "cmd", "ps1", "psm1", "vbs",
    "vbe", "js", "jse", "wsf", "wsh", "hta", "lnk", "reg", "jar", "sh", "app", "dmg", "pkg", "iso",
    "img", "docm", "xlsm", "pptm", "dotm", "xlam", "one", "url", "html", "htm", "svg",
];

fn expected_kind(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "pdf" => "PDF document",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" => "ZIP archive",
        "txt" | "csv" => "text",
        "xml" => "XML document",
        _ => return None,
    })
}

/// What a file's bytes look like, and whether that kind can run code.
pub fn sniff(data: &[u8]) -> (&'static str, bool) {
    let starts = |m: &[u8]| data.starts_with(m);
    let head = String::from_utf8_lossy(&data[..data.len().min(512)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if data.len() > 4
        && data[..data.len().min(1024)]
            .windows(5)
            .any(|w| w == b"%PDF-")
    {
        ("PDF document", false)
    } else if starts(b"MZ") {
        ("Windows program", true)
    } else if starts(b"\x7fELF") {
        ("Linux program", true)
    } else if starts(&[0xfe, 0xed, 0xfa, 0xce])
        || starts(&[0xfe, 0xed, 0xfa, 0xcf])
        || starts(&[0xcf, 0xfa, 0xed, 0xfe])
        || starts(&[0xca, 0xfe, 0xba, 0xbe])
    {
        ("Mac program", true)
    } else if starts(b"#!") {
        ("script", true)
    } else if starts(&[0xd0, 0xcf, 0x11, 0xe0]) {
        ("legacy Office document (may hold macros)", true)
    } else if starts(b"PK\x03\x04") {
        ("ZIP archive", false)
    } else if starts(b"\x89PNG") {
        ("PNG image", false)
    } else if starts(&[0xff, 0xd8, 0xff]) {
        ("JPEG image", false)
    } else if starts(b"GIF8") {
        ("GIF image", false)
    } else if starts(b"{\\rtf") {
        ("RTF document", false)
    } else if starts(b"Rar!") || starts(b"7z\xbc\xaf") {
        ("archive", false)
    } else if head.starts_with("<!doctype html") || head.starts_with("<html") {
        ("HTML page", true)
    } else if head.starts_with("<?xml") || head.starts_with("<svg") {
        if head.contains("<svg") {
            ("SVG image (may hold scripts)", true)
        } else {
            ("XML document", false)
        }
    } else if data.is_empty() {
        ("empty file", false)
    } else if std::str::from_utf8(&data[..data.len().min(4096)])
        .is_ok_and(|s| s.chars().all(|c| !c.is_control() || c.is_whitespace()))
    {
        ("text", false)
    } else {
        ("binary data", false)
    }
}

/// "a PNG image", "an HTML page".
fn with_article(kind: &str) -> String {
    let vowel = kind.starts_with(['a', 'e', 'i', 'o', 'u'])
        || ["HTML", "SVG", "XML", "RTF"]
            .iter()
            .any(|p| kind.starts_with(p));
    let a = if vowel { "an" } else { "a" };
    format!("{a} {kind}")
}

/// "D:20231004123000+02'00'" as "2023-10-04 12:30:00 +02:00".
pub fn pdf_date(d: &str) -> String {
    let s = d.strip_prefix("D:").unwrap_or(d);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return d.to_string();
    }
    let part = |from: usize, to: usize, default: &str| {
        digits
            .get(from..to)
            .map_or_else(|| default.to_string(), str::to_string)
    };
    let mut out = format!(
        "{}-{}-{} {}:{}:{}",
        part(0, 4, ""),
        part(4, 6, "01"),
        part(6, 8, "01"),
        part(8, 10, "00"),
        part(10, 12, "00"),
        part(12, 14, "00"),
    );
    let zone = &s[digits.len()..];
    if zone.starts_with('Z') {
        out.push_str(" UTC");
    } else if let Some(sign) = zone.chars().next().filter(|c| *c == '+' || *c == '-') {
        let z: String = zone[1..].chars().filter(char::is_ascii_digit).collect();
        if z.len() >= 2 {
            let m = z.get(2..4).unwrap_or("00");
            out.push_str(&format!(" {sign}{}:{m}", &z[..2]));
        }
    }
    out
}

fn text(doc: &Document, dict: &Dict, key: &[u8]) -> Option<String> {
    match doc.get_key(dict, key).ok()? {
        Object::String(s) => Some(s.to_text()),
        _ => None,
    }
}

fn name_tree(doc: &Document, node: &Dict, out: &mut Vec<(String, Object)>, depth: usize) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    if let Ok(Object::Array(names)) = doc.get_key(node, b"Names") {
        for pair in names.chunks_exact(2) {
            let key = match doc.resolve(&pair[0]) {
                Ok(Object::String(s)) => s.to_text(),
                _ => String::new(),
            };
            out.push((key, pair[1].clone()));
        }
    }
    if let Ok(Object::Array(kids)) = doc.get_key(node, b"Kids") {
        for k in kids {
            if let Ok(Object::Dict(kid)) = doc.resolve(&k) {
                name_tree(doc, &kid, out, depth + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_people_write_them() {
        assert_eq!(
            pdf_date("D:20231004123000+02'00'"),
            "2023-10-04 12:30:00 +02:00"
        );
        assert_eq!(pdf_date("D:2023"), "2023-01-01 00:00:00");
        assert_eq!(pdf_date("D:20231004123000Z"), "2023-10-04 12:30:00 UTC");
        assert_eq!(pdf_date("junk"), "junk");
    }

    #[test]
    fn sniffs_programs_and_documents() {
        assert_eq!(sniff(b"MZ\x90\x00"), ("Windows program", true));
        assert_eq!(sniff(b"%PDF-1.7\n"), ("PDF document", false));
        assert_eq!(sniff(b"hello, world\n"), ("text", false));
        assert_eq!(sniff(b"<!DOCTYPE html><p>"), ("HTML page", true));
        assert_eq!(sniff(&[0, 1, 2, 3]), ("binary data", false));
    }

    fn build(objects: &[&str]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn lists_checks_and_reads_embedded_files() {
        let doc = Document::open(build(&[
            "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(a) 4 0 R] >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Annots [<< /Subtype /FileAttachment /Rect [0 0 10 10] /Contents (Note) /FS << /UF (dir/run.exe) /EF << /F 6 0 R >> >> >> << /Subtype /FileAttachment /Rect [0 0 1 1] /FS 4 0 R >>] >>",
            "<< /Type /Filespec /UF (data.csv) /Desc (Figures) /EF << /F 5 0 R >> >>",
            "<< /Type /EmbeddedFile /Subtype /text#2Fcsv /Params << /Size 99 >> /Length 4 >>\nstream\na,b\n\nendstream",
            "<< /Type /EmbeddedFile /Length 4 >>\nstream\nMZ!!\nendstream",
        ]))
        .unwrap();
        let files = attachments(&doc);
        // The second annotation points at a file already listed.
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, "data.csv");
        assert_eq!(files[0].description.as_deref(), Some("Figures"));
        assert_eq!(files[0].mime.as_deref(), Some("text/csv"));
        assert_eq!(files[1].source, Source::Annotation { page: 0 });
        assert_eq!(files[1].description.as_deref(), Some("Note"));

        let data = attachment_data(&doc, &files[0]).unwrap();
        assert_eq!(data, b"a,b\n");
        let c = check(&files[0], &data);
        assert_eq!(c.kind, "text");
        assert_eq!(c.cautions, ["It says it is 99 bytes, but holds 4."]);

        let data = attachment_data(&doc, &files[1]).unwrap();
        let c = check(&files[1], &data);
        assert_eq!(c.kind, "Windows program");
        assert_eq!(c.cautions.len(), 2);
    }
}
