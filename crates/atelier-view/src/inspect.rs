//! What the inspector shows: a summary of the file, one-line descriptions
//! of objects, the children of an object for the tree, and previews of
//! stream data.

use atelier_syntax::{Document, Object, XrefEntry};

/// A line of the file overview: a label and its value.
pub type Fact = (&'static str, String);

/// The file at a glance.
pub fn overview(doc: &Document) -> Vec<Fact> {
    let mut out: Vec<Fact> = Vec::new();
    out.push(("PDF version", doc.version()));
    out.push((
        "Pages",
        doc.pages()
            .map_or_else(|e| format!("unreadable ({e})"), |p| p.len().to_string()),
    ));
    let numbers = doc.object_numbers();
    let compressed = numbers
        .iter()
        .filter(|&&n| matches!(doc.xref_entry(n), Some(XrefEntry::Compressed { .. })))
        .count();
    out.push((
        "Objects",
        if compressed > 0 {
            format!("{} ({compressed} in object streams)", numbers.len())
        } else {
            numbers.len().to_string()
        },
    ));
    let xref = if doc.trailer().is_type(b"XRef") {
        "stream"
    } else {
        "table"
    };
    out.push(("Cross-reference", xref.to_string()));
    out.push(("Revisions", revisions(doc.data()).to_string()));
    out.push((
        "Repaired",
        if doc.was_repaired() {
            "yes: the cross-reference data was broken".into()
        } else {
            "no".into()
        },
    ));
    out.push((
        "Encrypted",
        match doc.access() {
            None => "no".into(),
            Some(atelier_syntax::Access::User) => "yes, opened with the user password".into(),
            Some(atelier_syntax::Access::Owner) => "yes, opened with the owner password".into(),
        },
    ));
    if let Ok(catalog) = doc.catalog() {
        if let Ok(Object::Dict(acro)) = doc.get_key(&catalog, b"AcroForm") {
            let fields = doc
                .get_key(&acro, b"Fields")
                .ok()
                .and_then(|f| f.as_array().map(<[Object]>::len))
                .unwrap_or(0);
            out.push(("Form", format!("AcroForm, {fields} top-level fields")));
            if acro.get(b"XFA").is_some_and(|x| !x.is_null()) {
                out.push(("XFA", "present (not used by Atelier)".into()));
            }
            let sig = doc
                .get_key(&acro, b"SigFlags")
                .ok()
                .and_then(|o| o.as_i64())
                .unwrap_or(0);
            if sig & 1 != 0 {
                out.push(("Signatures", "the file says it holds signatures".into()));
            }
        }
        for (key, label) in [
            (&b"OpenAction"[..], "Open action"),
            (b"AA", "Document actions"),
        ] {
            if catalog.get(key).is_some() {
                out.push((label, "present".into()));
            }
        }
        if let Ok(Object::Dict(names)) = doc.get_key(&catalog, b"Names") {
            for (key, label) in [
                (&b"JavaScript"[..], "Document scripts"),
                (b"EmbeddedFiles", "Embedded files"),
            ] {
                if names.get(key).is_some() {
                    out.push((label, "present".into()));
                }
            }
        }
    }
    if let Some(info) = doc.info() {
        for (key, label) in [
            (&b"Title"[..], "Title"),
            (b"Author", "Author"),
            (b"Producer", "Producer"),
            (b"Creator", "Creator"),
            (b"CreationDate", "Created"),
            (b"ModDate", "Modified"),
        ] {
            if let Ok(Object::String(s)) = doc.get_key(&info, key) {
                let text = s.to_text();
                let text = if key.ends_with(b"Date") {
                    pdf_date(&text).unwrap_or(text)
                } else {
                    text
                };
                out.push((label, text));
            }
        }
    }
    out
}

/// A PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`) as `YYYY-MM-DD HH:mm:SS +HH:mm`.
/// Missing parts default as the standard says.
pub fn pdf_date(s: &str) -> Option<String> {
    let s = s.strip_prefix("D:").unwrap_or(s);
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |from: usize, default: &'static str| digits.get(from..from + 2).unwrap_or(default);
    let mut out = format!(
        "{}-{}-{} {}:{}:{}",
        &digits[..4],
        part(4, "01"),
        part(6, "01"),
        part(8, "00"),
        part(10, "00"),
        part(12, "00")
    );
    let zone = &s[digits.len()..];
    match zone.chars().next() {
        Some('Z') => out.push_str(" UTC"),
        Some(sign @ ('+' | '-')) => {
            let z: String = zone[1..].chars().filter(char::is_ascii_digit).collect();
            if z.len() >= 2 {
                let minutes = z.get(2..4).unwrap_or("00");
                out.push_str(&format!(" {sign}{}:{minutes}", &z[..2]));
            }
        }
        _ => {}
    }
    Some(out)
}

/// How many times the file was saved: one plus each incremental update.
pub fn revisions(data: &[u8]) -> usize {
    data.windows(5).filter(|w| *w == b"%%EOF").count().max(1)
}

/// One line describing an object, without following references.
pub fn summary(obj: &Object) -> String {
    const MAX: usize = 80;
    let s = match obj {
        Object::Dict(d) => match d.type_name() {
            Some(t) => format!("<< /Type /{} … >> ({} keys)", t.as_str_lossy(), d.len()),
            None => format!("<< … >> ({} keys)", d.len()),
        },
        Object::Array(a) => format!("[ … ] ({} items)", a.len()),
        Object::Stream(s) => {
            let kind = s
                .dict
                .type_name()
                .or_else(|| s.dict.get(b"Subtype").and_then(Object::as_name))
                .map(|n| format!("/{} ", n.as_str_lossy()))
                .unwrap_or_default();
            format!("{kind}stream, {} bytes", s.data.len())
        }
        Object::String(s) => format!("({})", s.to_text()),
        other => other.to_string(),
    };
    if s.chars().count() > MAX {
        let cut: String = s.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        s
    }
}

/// The entries under an object in the tree: a dictionary's keys, an
/// array's items, a stream's dictionary.
pub fn children(obj: &Object) -> Vec<(String, Object)> {
    match obj {
        Object::Dict(d) => d
            .iter()
            .map(|(k, v)| (format!("/{}", k.as_str_lossy()), v.clone()))
            .collect(),
        Object::Stream(s) => s
            .dict
            .iter()
            .map(|(k, v)| (format!("/{}", k.as_str_lossy()), v.clone()))
            .collect(),
        Object::Array(a) => a
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("[{i}]"), v.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether an object has entries to expand.
pub fn has_children(obj: &Object) -> bool {
    match obj {
        Object::Dict(d) => !d.is_empty(),
        Object::Array(a) => !a.is_empty(),
        Object::Stream(_) => true,
        _ => false,
    }
}

/// Stream data as people can read it: text when it mostly is, otherwise
/// a hex dump. Only the first `limit` bytes are shown.
pub fn preview(data: &[u8], limit: usize) -> String {
    let shown = &data[..data.len().min(limit)];
    let printable = shown
        .iter()
        .filter(|&&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
        .count();
    let mut out = if shown.is_empty() || printable * 100 / shown.len() >= 90 {
        shown
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b.is_ascii_whitespace() {
                    b as char
                } else {
                    '·'
                }
            })
            .collect()
    } else {
        hex_dump(shown)
    };
    if data.len() > limit {
        out.push_str(&format!("\n… {} more bytes", data.len() - limit));
    }
    out
}

fn hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    for (i, row) in data.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match row.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.extend(row.iter().map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        }));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use atelier_syntax::{Dict, Name, PdfString};

    #[test]
    fn summaries_are_short_and_name_the_type() {
        let mut d = Dict::new();
        d.insert("Type", Object::Name(Name::new("Page")));
        d.insert("Rotate", Object::Integer(90));
        assert_eq!(summary(&Object::Dict(d)), "<< /Type /Page … >> (2 keys)");
        assert_eq!(summary(&Object::Integer(7)), "7");
        let long = Object::String(PdfString::literal(vec![b'a'; 200]));
        assert!(summary(&long).ends_with('…'));
        assert!(summary(&long).chars().count() <= 81);
    }

    #[test]
    fn children_of_dicts_and_arrays() {
        let a = Object::Array(vec![Object::Integer(1), Object::Bool(true)]);
        let c = children(&a);
        assert_eq!(c[1].0, "[1]");
        assert!(has_children(&a));
        assert!(!has_children(&Object::Null));
    }

    #[test]
    fn previews_text_or_hex() {
        assert_eq!(preview(b"BT /F1 12 Tf ET", 100), "BT /F1 12 Tf ET");
        let bin = preview(&[0u8, 1, 2, 255, 0x41], 100);
        assert!(bin.starts_with("00000000  00 01 02 ff 41"));
        assert!(preview(&[b'x'; 50], 10).ends_with("40 more bytes"));
    }

    #[test]
    fn formats_dates() {
        assert_eq!(
            pdf_date("D:20201223033351-08'00'").as_deref(),
            Some("2020-12-23 03:33:51 -08:00")
        );
        assert_eq!(pdf_date("D:2019").as_deref(), Some("2019-01-01 00:00:00"));
        assert_eq!(
            pdf_date("D:20190102Z").as_deref(),
            Some("2019-01-02 00:00:00 UTC")
        );
        assert_eq!(pdf_date("yesterday"), None);
    }

    #[test]
    fn counts_revisions() {
        assert_eq!(revisions(b"%PDF-1.7 ... %%EOF\n"), 1);
        assert_eq!(revisions(b"%PDF %%EOF more %%EOF"), 2);
        assert_eq!(revisions(b"broken"), 1);
    }
}
