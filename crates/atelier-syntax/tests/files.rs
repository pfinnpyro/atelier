//! End-to-end tests on small PDFs built byte by byte, including broken ones.

use atelier_syntax::{Access, Document, Error, ObjRef, Object, Rect};
use miniz_oxide::deflate::compress_to_vec_zlib;

/// Writes a PDF while tracking object offsets.
struct Builder {
    buf: Vec<u8>,
    offsets: Vec<(u32, usize)>,
}

impl Builder {
    fn new() -> Self {
        Self::with_prefix(b"")
    }

    fn with_prefix(prefix: &[u8]) -> Self {
        let mut buf = prefix.to_vec();
        buf.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        Self {
            buf,
            offsets: Vec::new(),
        }
    }

    fn obj(&mut self, num: u32, body: &str) -> &mut Self {
        self.offsets.push((num, self.buf.len()));
        self.buf
            .extend_from_slice(format!("{num} 0 obj\n{body}\nendobj\n").as_bytes());
        self
    }

    fn stream(&mut self, num: u32, dict: &str, data: &[u8]) -> &mut Self {
        self.offsets.push((num, self.buf.len()));
        self.buf.extend_from_slice(
            format!("{num} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(),
        );
        self.buf.extend_from_slice(data);
        self.buf.extend_from_slice(b"\nendstream\nendobj\n");
        self
    }

    fn offset(&self, num: u32) -> usize {
        self.offsets
            .iter()
            .rev()
            .find(|(n, _)| *n == num)
            .unwrap()
            .1
    }

    /// Writes an xref table for `nums` (all objects when `None`), the trailer and
    /// startxref. Returns the table's offset.
    fn xref(&mut self, nums: Option<&[u32]>, trailer: &str) -> usize {
        let start = self.buf.len();
        let mut nums: Vec<u32> = match nums {
            Some(n) => n.to_vec(),
            None => {
                let mut all: Vec<u32> = self.offsets.iter().map(|(n, _)| *n).collect();
                all.push(0);
                all
            }
        };
        nums.sort_unstable();
        nums.dedup();
        let size = nums.iter().max().unwrap() + 1;
        self.buf.extend_from_slice(b"xref\n");
        for &num in &nums {
            self.buf.extend_from_slice(format!("{num} 1\n").as_bytes());
            if num == 0 {
                self.buf.extend_from_slice(b"0000000000 65535 f\r\n");
            } else {
                let off = self.offset(num);
                self.buf
                    .extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
            }
        }
        self.buf.extend_from_slice(
            format!("trailer\n<< /Size {size} {trailer} >>\nstartxref\n{start}\n%%EOF\n")
                .as_bytes(),
        );
        start
    }

    fn open(&self) -> Document {
        Document::open(self.buf.clone()).unwrap()
    }
}

const CONTENT: &[u8] = b"BT /F1 24 Tf 72 700 Td (Hello, Atelier) Tj ET";

fn two_page_file() -> Builder {
    let mut b = Builder::new();
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 595 842] \
             /Resources << /Font << /F1 6 0 R >> >> >>",
        )
        .obj(3, "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>")
        .obj(
            4,
            "<< /Type /Page /Parent 2 0 R /Rotate -90 /CropBox [10 10 2000 400] \
             /Contents [5 0 R 5 0 R] >>",
        )
        .stream(5, "/Filter /FlateDecode", &compress_to_vec_zlib(CONTENT, 6))
        .obj(6, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(7, "<< /Title (Test file) >>");
    b.xref(None, "/Root 1 0 R /Info 7 0 R");
    b
}

#[test]
fn classic_xref_with_inherited_attributes() {
    let doc = two_page_file().open();
    assert!(!doc.was_repaired());
    assert_eq!(doc.version(), "1.7");
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 2);

    let a4 = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 595.0,
        y1: 842.0,
    };
    assert_eq!(pages[0].media_box, a4);
    assert_eq!(pages[0].crop_box, a4);
    assert_eq!(pages[0].rotate, 0);
    assert!(pages[0].resources.contains_key(b"Font"));
    assert_eq!(pages[0].contents(&doc).unwrap(), [CONTENT, b"\n"].concat());

    assert_eq!(pages[1].rotate, 270);
    assert_eq!(
        pages[1].crop_box,
        Rect {
            x0: 10.0,
            y0: 10.0,
            x1: 595.0,
            y1: 400.0
        }
    );
    assert_eq!(
        pages[1].contents(&doc).unwrap().len(),
        2 * (CONTENT.len() + 1)
    );

    let title = doc.info().unwrap().get(b"Title").cloned().unwrap();
    assert_eq!(title.as_string().unwrap().bytes, b"Test file");
}

/// Encodes xref stream rows with the PNG Up predictor.
fn png_up(rows: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut prev = vec![0u8; rows[0].len()];
    for row in rows {
        out.push(2);
        out.extend(row.iter().zip(&prev).map(|(a, b)| a.wrapping_sub(*b)));
        prev = row.clone();
    }
    out
}

#[test]
fn xref_stream_and_object_stream() {
    let mut b = Builder::new();
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>");
    b.stream(4, "/Filter /FlateDecode", &compress_to_vec_zlib(CONTENT, 6));

    // Objects 2 and 3 live inside object stream 5.
    let o2 = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    let o3 = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>";
    let header = format!("2 0 3 {} ", o2.len() + 1);
    let body = format!("{header}{o2}\n{o3}");
    b.stream(
        5,
        &format!(
            "/Type /ObjStm /N 2 /First {} /Filter /FlateDecode",
            header.len()
        ),
        &compress_to_vec_zlib(body.as_bytes(), 6),
    );

    let xref_offset = b.buf.len();
    let row = |kind: u8, a: usize, c: u16| {
        let mut r = vec![kind];
        r.extend_from_slice(&(a as u32).to_be_bytes());
        r.extend_from_slice(&c.to_be_bytes());
        r
    };
    let rows = vec![
        row(0, 0, 0xffff),
        row(1, b.offset(1), 0),
        row(2, 5, 0),
        row(2, 5, 1),
        row(1, b.offset(4), 0),
        row(1, b.offset(5), 0),
        row(1, xref_offset, 0),
    ];
    let data = compress_to_vec_zlib(&png_up(&rows), 6);
    b.stream(
        6,
        "/Type /XRef /Size 7 /W [1 4 2] /Root 1 0 R /Filter /FlateDecode \
         /DecodeParms << /Predictor 12 /Columns 7 >>",
        &data,
    );
    b.buf
        .extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());

    let doc = b.open();
    assert!(!doc.was_repaired(), "{:?}", doc.warnings());
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_box.width(), 300.0);
    assert_eq!(pages[0].contents(&doc).unwrap(), [CONTENT, b"\n"].concat());
}

#[test]
fn incremental_update_overrides_older_objects() {
    let mut b = two_page_file();
    let first_xref = b.xref(None, "/Root 1 0 R");
    b.obj(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] /Contents 5 0 R >>",
    );
    b.xref(Some(&[3]), &format!("/Root 1 0 R /Prev {first_xref}"));
    let doc = b.open();
    assert!(!doc.was_repaired());
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].media_box.height(), 200.0);
}

#[test]
fn bad_startxref_is_repaired() {
    let mut b = two_page_file();
    let pos = b.buf.windows(9).rposition(|w| w == b"startxref").unwrap();
    b.buf.truncate(pos);
    b.buf.extend_from_slice(b"startxref\n99999999\n%%EOF\n");
    let doc = b.open();
    assert!(doc.was_repaired());
    assert_eq!(doc.pages().unwrap().len(), 2);
}

#[test]
fn file_without_xref_or_trailer_is_repaired_from_catalog() {
    let mut b = two_page_file();
    let pos = b.buf.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    b.buf.truncate(pos);
    let doc = b.open();
    assert!(doc.was_repaired());
    assert_eq!(
        doc.trailer().get(b"Root"),
        Some(&Object::Reference(ObjRef::new(1, 0)))
    );
    assert_eq!(doc.pages().unwrap().len(), 2);
}

#[test]
fn wrong_offsets_are_found_by_scanning() {
    let mut b = two_page_file();
    // Shift every object by inserting bytes after the header without fixing the xref.
    let first_obj = b.offset(1);
    b.buf
        .splice(first_obj..first_obj, b"%junk junk junk\n".iter().copied());
    // Keep startxref correct so only the object offsets are wrong.
    let xref_at = b.buf.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    let pos = b.buf.windows(9).rposition(|w| w == b"startxref").unwrap();
    b.buf.truncate(pos);
    b.buf
        .extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());
    let doc = b.open();
    assert!(!doc.was_repaired());
    assert_eq!(doc.pages().unwrap().len(), 2);
    assert_eq!(
        doc.pages().unwrap()[0].contents(&doc).unwrap(),
        [CONTENT, b"\n"].concat()
    );
    assert!(doc.warnings().iter().any(|w| w.contains("wrong offset")));
}

#[test]
fn junk_before_header_shifts_offsets() {
    let mut b = Builder::with_prefix(&[b'#'; 100]);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>");
    // Offsets relative to %PDF-, as a tool that prepended junk would leave them.
    for (_, off) in &mut b.offsets {
        *off -= 100;
    }
    let start = b.buf.len();
    b.xref(None, "/Root 1 0 R");
    let pos = b.buf.windows(9).rposition(|w| w == b"startxref").unwrap();
    b.buf.truncate(pos);
    b.buf
        .extend_from_slice(format!("startxref\n{}\n%%EOF\n", start - 100).as_bytes());
    let doc = b.open();
    assert_eq!(doc.pages().unwrap().len(), 1);
    assert_eq!(doc.pages().unwrap()[0].media_box, Rect::LETTER);
}

#[test]
fn page_tree_cycle_terminates() {
    let mut b = Builder::new();
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R 2 0 R] /Count 2 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R >>");
    b.xref(None, "/Root 1 0 R");
    assert_eq!(b.open().pages().unwrap().len(), 1);
}

#[test]
fn reference_to_missing_object_is_null() {
    let doc = two_page_file().open();
    assert_eq!(
        doc.resolve(&Object::Reference(ObjRef::new(99, 0))),
        Ok(Object::Null)
    );
    assert_eq!(
        doc.get(ObjRef::new(99, 0)),
        Err(Error::MissingObject(ObjRef::new(99, 0)))
    );
}

#[test]
fn encrypted_file_without_the_password() {
    let mut b = two_page_file();
    let pos = b.buf.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    b.buf.truncate(pos);
    b.obj(
        8,
        "<< /Filter /Standard /V 2 /R 3 /Length 128 /O <00> /U <00> /P -4 >>",
    );
    b.xref(None, "/Root 1 0 R /Encrypt 8 0 R");
    assert_eq!(Document::open(b.buf.clone()).err(), Some(Error::Password));

    // An /Encrypt entry pointing nowhere is ignored.
    let mut b = two_page_file();
    let pos = b.buf.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    b.buf.truncate(pos);
    b.xref(None, "/Root 1 0 R /Encrypt 9 0 R");
    let doc = b.open();
    assert!(!doc.is_encrypted());
    assert!(doc.pages().unwrap()[0].contents(&doc).is_ok());
}

fn encrypted(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/data/encrypted/{name}.pdf",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap()
}

/// Checks a decrypted test file: the title string and the page content.
fn check_decrypted(doc: &Document) {
    let title = doc.get_key(&doc.info().unwrap(), b"Title").unwrap();
    assert_eq!(title.as_string().unwrap().bytes, b"Secret title");
    let content = doc.pages().unwrap()[0].contents(doc).unwrap();
    let text = String::from_utf8_lossy(&content);
    assert!(text.contains("(Hidden) Tj"), "{text}");
}

#[test]
fn opens_every_standard_encryption() {
    // Written by pypdf: tools/make_encrypted.py.
    for (name, user) in [
        ("rc4-40", ""),
        ("rc4-128", "user"),
        ("aes-128", "user"),
        ("aes-256-r5", "user"),
        ("aes-256", "pässwörd"),
    ] {
        let data = encrypted(name);
        let doc = Document::open_with_password(data.clone(), user.as_bytes())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(doc.is_encrypted());
        assert_eq!(doc.access(), Some(Access::User), "{name}");
        check_decrypted(&doc);

        let doc = Document::open_with_password(data.clone(), b"owner").unwrap();
        assert_eq!(doc.access(), Some(Access::Owner), "{name}");
        check_decrypted(&doc);

        assert_eq!(
            Document::open_with_password(data, b"wrong").err(),
            Some(Error::Password),
            "{name}"
        );
    }
}

#[test]
fn empty_user_password_opens_without_asking() {
    let doc = Document::open(encrypted("rc4-40")).unwrap();
    assert_eq!(doc.access(), Some(Access::User));
    check_decrypted(&doc);
    assert_eq!(
        Document::open(encrypted("aes-256")).err(),
        Some(Error::Password)
    );
}

/// Damaged files must never panic. This mutates a valid file many ways.
#[test]
fn mutated_files_never_panic() {
    let original = two_page_file().buf;
    let mut state = 0x2545_f491_u32;
    let mut rand = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as usize
    };
    for _ in 0..3000 {
        let mut data = original.clone();
        match rand() % 4 {
            0 => data.truncate(rand() % data.len()),
            1 => {
                for _ in 0..1 + rand() % 8 {
                    let i = rand() % data.len();
                    data[i] = rand() as u8;
                }
            }
            2 => {
                let i = rand() % data.len();
                let n = (rand() % 64).min(data.len() - i);
                data.drain(i..i + n);
            }
            _ => {
                let i = rand() % data.len();
                let tokens: [&[u8]; 6] = [b"<<", b">>", b"[", b"(", b" 0 R", b" obj "];
                data.splice(i..i, tokens[rand() % tokens.len()].iter().copied());
            }
        }
        if let Ok(doc) = Document::open(data) {
            if let Ok(pages) = doc.pages() {
                for page in pages {
                    let _ = page.contents(&doc);
                }
            }
            for num in doc.object_numbers() {
                let _ = doc.get(ObjRef::new(num, 0));
            }
        }
    }
}
