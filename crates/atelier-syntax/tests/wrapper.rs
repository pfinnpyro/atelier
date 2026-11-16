//! Wrapper documents around an encrypted payload.

use atelier_syntax::{Cover, Document, OPENPGP_PAYLOAD, wrap_encrypted_payload};

#[test]
fn a_wrapper_is_a_readable_pdf_that_gives_its_payload_back() {
    // Bytes of every value, including ones that end a stream early if
    // the length is not honoured.
    let mut payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    payload.extend_from_slice(b"\nendstream\nendobj\n");
    let cover = Cover {
        title: r"Encrypted (with) parentheses \ and a backslash",
        lines: &["First line", "", "Caf\u{e9} is not in the font"],
    };
    let bytes = wrap_encrypted_payload(&payload, OPENPGP_PAYLOAD, "1", "document.pdf.pgp", &cover);
    assert!(bytes.starts_with(b"%PDF-2.0\n"));
    let doc = Document::open(bytes).unwrap();
    assert!(!doc.was_repaired());
    assert!(doc.warnings().is_empty(), "{:?}", doc.warnings());
    assert!(!doc.is_encrypted());
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 1);
    let content = String::from_utf8(pages[0].contents(&doc).unwrap()).unwrap();
    assert!(content.contains(r"(Encrypted \(with\) parentheses \\ and a backslash) Tj"));
    assert!(content.contains("(Caf? is not in the font) Tj"));

    let found = doc.encrypted_payload().unwrap().unwrap();
    assert_eq!(found.filter, OPENPGP_PAYLOAD);
    assert_eq!(found.version.as_deref(), Some("1"));
    assert_eq!(found.name, "document.pdf.pgp");
    assert_eq!(found.data, payload);
}

#[test]
fn an_ordinary_pdf_wraps_nothing() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n%%EOF";
    let doc = Document::open(pdf.to_vec()).unwrap();
    assert_eq!(doc.encrypted_payload().unwrap(), None);
}

#[test]
fn an_attachment_without_a_payload_dictionary_is_not_a_payload() {
    let pdf = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R \
        /Names << /EmbeddedFiles << /Kids [5 0 R] >> >> >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        4 0 obj << /Length 5 >> stream\nhello\nendstream endobj\n\
        5 0 obj << /Names [(a.txt) << /Type /Filespec /F (a.txt) /EF << /F 4 0 R >> >>] \
        /Kids [5 0 R] >> endobj\n%%EOF";
    let doc = Document::open(pdf.to_vec()).unwrap();
    assert_eq!(doc.encrypted_payload().unwrap(), None);
}
