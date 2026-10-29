//! Saving: incremental updates, cross-reference streams, encryption and
//! full rewrites of repaired files.

use atelier_syntax::{Dict, Document, Name, ObjRef, Object, PdfString, Stream};

/// A small file with a classic cross-reference table.
fn classic() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
        "<< /Title (Before) >>",
    ];
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f\r\n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size 5 /Root 1 0 R /Info 4 0 R /ID [<00112233> <00112233>] >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

fn title(doc: &Document) -> Vec<u8> {
    let t = doc.get_key(&doc.info().unwrap(), b"Title").unwrap();
    t.as_string().unwrap().bytes.clone()
}

fn info_with_title(text: &[u8]) -> Object {
    let mut d = Dict::new();
    d.insert("Title", Object::String(PdfString::literal(text)));
    Object::Dict(d)
}

#[test]
fn incremental_update_keeps_the_original_bytes() {
    let original = classic();
    let doc = Document::open(original.clone()).unwrap();
    let mut up = doc.update();
    up.set(ObjRef::new(4, 0), info_with_title(b"After (edited)"));
    let mut d = Dict::new();
    d.insert("Note", Object::Name(Name::from("New")));
    let added = up.add(Object::Dict(d));
    assert_eq!(added, ObjRef::new(5, 0));
    let saved = up.to_bytes().unwrap();

    assert_eq!(&saved[..original.len()], &original[..]);
    let again = Document::open(saved).unwrap();
    assert!(!again.was_repaired(), "{:?}", again.warnings());
    assert_eq!(title(&again), b"After (edited)");
    assert_eq!(again.pages().unwrap().len(), 1);
    let note = again.get(added).unwrap();
    assert_eq!(
        note.as_dict().unwrap().get(b"Note"),
        Some(&Object::Name(Name::from("New")))
    );
    // The file ID keeps its first part and changes its second.
    let Some(Object::Array(id)) = again.trailer().get(b"ID") else {
        panic!("no ID")
    };
    assert_eq!(id[0].as_string().unwrap().bytes, [0x00, 0x11, 0x22, 0x33]);
    assert_ne!(id[1].as_string().unwrap().bytes, [0x00, 0x11, 0x22, 0x33]);

    // A second update on top of the first.
    let mut up = again.update();
    up.set(ObjRef::new(4, 0), info_with_title(b"Third"));
    let third = Document::open(up.to_bytes().unwrap()).unwrap();
    assert!(!third.was_repaired());
    assert_eq!(title(&third), b"Third");
    assert!(third.get(added).is_ok());
}

#[test]
fn files_with_xref_streams_get_an_xref_stream_update() {
    // Write the classic file, then convert it: a full rewrite through an
    // xref-stream trailer is what most modern producers emit.
    let base = classic();
    let xref_at = base.windows(5).rposition(|w| w == b"xref\n").unwrap();
    let mut data = base[..xref_at].to_vec();
    let offsets: Vec<usize> = (1..=4)
        .map(|n| {
            let tag = format!("{n} 0 obj");
            data.windows(tag.len())
                .position(|w| w == tag.as_bytes())
                .unwrap()
        })
        .collect();
    let stream_at = data.len();
    let mut rows = vec![0u8, 0, 0, 0, 0, 0xff, 0xff];
    for off in offsets.iter().chain([&stream_at]) {
        rows.push(1);
        rows.extend_from_slice(&(*off as u32).to_be_bytes());
        rows.extend_from_slice(&[0, 0]);
    }
    data.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /XRef /Size 6 /W [1 4 2] /Root 1 0 R /Info 4 0 R /Length {} >>\nstream\n",
            rows.len()
        )
        .as_bytes(),
    );
    data.extend_from_slice(&rows);
    data.extend_from_slice(
        format!("\nendstream\nendobj\nstartxref\n{stream_at}\n%%EOF\n").as_bytes(),
    );
    let doc = Document::open(data.clone()).unwrap();
    assert!(!doc.was_repaired(), "{:?}", doc.warnings());

    let mut up = doc.update();
    up.set(ObjRef::new(4, 0), info_with_title(b"Streamed"));
    let saved = up.to_bytes().unwrap();
    let tail = String::from_utf8_lossy(&saved[data.len()..]);
    assert!(tail.contains("/Type /XRef"), "{tail}");
    assert!(!tail.contains("\nxref\n"));
    let again = Document::open(saved).unwrap();
    assert!(!again.was_repaired(), "{:?}", again.warnings());
    assert_eq!(title(&again), b"Streamed");
}

#[test]
fn encrypted_files_stay_encrypted() {
    for (name, password) in [
        ("rc4-40", ""),
        ("rc4-128", "user"),
        ("aes-128", "user"),
        ("aes-256", "pässwörd"),
    ] {
        let path = format!(
            "{}/tests/data/encrypted/{name}.pdf",
            env!("CARGO_MANIFEST_DIR")
        );
        let data = std::fs::read(path).unwrap();
        let doc = Document::open_with_password(data.clone(), password.as_bytes()).unwrap();
        let info = doc
            .trailer()
            .get(b"Info")
            .and_then(Object::as_reference)
            .unwrap();
        let mut up = doc.update();
        up.set(info, info_with_title(b"Edited secret"));
        let content = b"BT /F1 12 Tf (Added) Tj ET".to_vec();
        let added = up.add(Object::Stream(Stream {
            dict: Dict::new(),
            data: content.clone().into(),
        }));
        let saved = up.to_bytes().unwrap();
        // Nothing appended is readable without the key.
        let tail = &saved[data.len()..];
        assert!(!tail.windows(13).any(|w| w == b"Edited secret"), "{name}");
        assert!(!tail.windows(5).any(|w| w == b"Added"), "{name}");

        let again = Document::open_with_password(saved, password.as_bytes()).unwrap();
        assert!(!again.was_repaired(), "{name}: {:?}", again.warnings());
        assert_eq!(title(&again), b"Edited secret", "{name}");
        let Object::Stream(s) = again.get(added).unwrap() else {
            panic!("{name}: not a stream")
        };
        assert_eq!(again.decode_stream(&s).unwrap().data, content, "{name}");
    }
}

#[test]
fn repaired_files_are_rewritten_whole() {
    let mut data = classic();
    // Break startxref.
    let pos = data.windows(9).rposition(|w| w == b"startxref").unwrap();
    data.truncate(pos);
    data.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    let doc = Document::open(data).unwrap();
    assert!(doc.was_repaired());
    let mut up = doc.update();
    up.set(ObjRef::new(4, 0), info_with_title(b"Fixed"));
    let saved = up.to_bytes().unwrap();
    let again = Document::open(saved).unwrap();
    assert!(!again.was_repaired(), "{:?}", again.warnings());
    assert_eq!(title(&again), b"Fixed");
    assert_eq!(again.pages().unwrap().len(), 1);
}

#[test]
fn serialized_objects_parse_back() {
    let mut d = Dict::new();
    d.insert("Name", Object::Name(Name::new(b"A B#(x)".to_vec())));
    d.insert(
        "Text",
        Object::String(PdfString::literal(b"a (b) \\ c\n".to_vec())),
    );
    d.insert("Bin", Object::String(PdfString::literal(vec![0, 255, 10])));
    d.insert("Num", Object::Real(-0.000_001));
    d.insert("Big", Object::Real(12345678.5));
    d.insert(
        "List",
        Object::Array(vec![
            Object::Null,
            Object::Bool(true),
            ObjRef::new(3, 0).into(),
        ]),
    );
    let obj = Object::Dict(d);
    let mut out = Vec::new();
    atelier_syntax::serialize(&mut out, &obj);
    let back = atelier_syntax::Parser::new(&out).parse_object().unwrap();
    let Object::Dict(b) = back else { panic!() };
    let Object::Dict(a) = obj else { panic!() };
    for (k, v) in a.iter() {
        let got = b.get(k.as_bytes()).unwrap();
        match (v, got) {
            (Object::Real(x), Object::Real(y)) => assert!((x - y).abs() < 1e-5, "{k}"),
            (Object::Real(x), Object::Integer(y)) => assert!((x - *y as f64).abs() < 1e-5, "{k}"),
            (Object::String(x), Object::String(y)) => assert_eq!(x.bytes, y.bytes, "{k}"),
            _ => assert_eq!(v, got, "{k}"),
        }
    }
}
