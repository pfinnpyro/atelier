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

#[test]
fn text_strings_round_trip() {
    use atelier_syntax::PdfString;
    for text in ["plain", "café €5 — “quoted”", "Ελληνικά", "日本語", ""] {
        assert_eq!(PdfString::from_text(text).to_text(), text);
    }
    // Latin text stays PDFDocEncoding; anything else becomes UTF-16BE.
    assert_eq!(PdfString::from_text("café").bytes, b"caf\xE9");
    assert!(
        PdfString::from_text("日本")
            .bytes
            .starts_with(&[0xFE, 0xFF])
    );
    assert_eq!(
        PdfString::literal(b"\xEF\xBB\xBFok".to_vec()).to_text(),
        "ok"
    );
}

#[test]
fn aes256_export_requires_password_and_randomizes_output() {
    let doc = Document::open(classic()).unwrap();
    let first = doc
        .update()
        .to_encrypted_bytes("read-password", "owner-password")
        .unwrap();
    let second = doc
        .update()
        .to_encrypted_bytes("read-password", "owner-password")
        .unwrap();
    assert_ne!(first, second);
    assert!(!first.windows(b"Before".len()).any(|w| w == b"Before"));
    assert!(matches!(
        Document::open(first.clone()),
        Err(atelier_syntax::Error::Password)
    ));
    assert!(matches!(
        Document::open_with_password(first.clone(), b"wrong"),
        Err(atelier_syntax::Error::Password)
    ));
    let user = Document::open_with_password(first.clone(), b"read-password").unwrap();
    assert_eq!(user.access(), Some(atelier_syntax::Access::User));
    assert_eq!(title(&user), b"Before");
    assert_eq!(user.pages().unwrap().len(), 1);
    let owner = Document::open_with_password(first, b"owner-password").unwrap();
    assert_eq!(owner.access(), Some(atelier_syntax::Access::Owner));
    assert_eq!(title(&owner), b"Before");
    assert!(doc.update().to_encrypted_bytes("", "owner").is_err());
    assert!(
        doc.update()
            .to_encrypted_bytes("nonascii-é", "owner")
            .is_err()
    );
}

#[test]
fn signature_byte_ranges_survive_embedding_and_reopening() {
    let doc = Document::open(classic()).unwrap();
    let prepared = doc.prepare_signature("Ada", 1024).unwrap();
    let signing = prepared.signing_bytes();
    let bytes = prepared.finish(&[0x30, 0x01, 0x00]).unwrap();
    let saved = Document::open(bytes.clone()).unwrap();
    let form = saved
        .resolve_dict(saved.catalog().unwrap().get(b"AcroForm").unwrap())
        .unwrap();
    let fields = saved.resolve(form.get(b"Fields").unwrap()).unwrap();
    let field = saved.resolve_dict(&fields.as_array().unwrap()[0]).unwrap();
    let signature = saved.resolve_dict(field.get(b"V").unwrap()).unwrap();
    let range = signature.get(b"ByteRange").unwrap().as_array().unwrap();
    let r: Vec<_> = range.iter().map(|n| n.as_i64().unwrap() as usize).collect();
    assert_eq!(r[0], 0);
    assert_eq!(r[2] + r[3], bytes.len());
    assert_eq!([&bytes[..r[1]], &bytes[r[2]..]].concat(), signing);
    assert_eq!(bytes[r[1]], b'<');
    assert_eq!(bytes[r[2] - 1], b'>');
    assert!(
        doc.prepare_signature("Ada", 1024)
            .unwrap()
            .finish(&vec![0; 1025])
            .is_err()
    );
    assert!(saved.update().to_encrypted_bytes("read", "owner").is_err());
}

#[test]
fn detached_pdf_signature_verifies_with_openssl() {
    use std::process::{Command, Stdio};
    if Command::new("openssl").arg("version").output().is_err() {
        eprintln!("OpenSSL unavailable; external CMS verification skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "atelier-cms-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let key = dir.join("key.pem");
    let cert = dir.join("cert.pem");
    let config = dir.join("openssl.cnf");
    std::fs::write(&config, "[req]\ndistinguished_name=dn\n[dn]\n").unwrap();
    let generated = Command::new("openssl")
        .arg("req")
        .arg("-config")
        .arg(&config)
        .args([
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-x509",
            "-subj",
            "/CN=Atelier test",
            "-days",
            "1",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let doc = Document::open(classic()).unwrap();
    let prepared = doc.prepare_signature("Atelier test", 32768).unwrap();
    let content = dir.join("content.bin");
    let cms = dir.join("signature.der");
    std::fs::write(&content, prepared.signing_bytes()).unwrap();
    let signed = Command::new("openssl")
        .args([
            "cms",
            "-sign",
            "-binary",
            "-outform",
            "DER",
            "-md",
            "sha256",
            "-nosmimecap",
            "-signer",
        ])
        .arg(&cert)
        .arg("-inkey")
        .arg(&key)
        .arg("-in")
        .arg(&content)
        .arg("-out")
        .arg(&cms)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let bytes = prepared.finish(&std::fs::read(&cms).unwrap()).unwrap();
    let saved = Document::open(bytes.clone()).unwrap();
    let inspection = saved.inspect_signatures().unwrap().remove(0).unwrap();
    assert!(!inspection.has_subsequent_bytes);
    std::fs::write(&content, &inspection.signed_bytes).unwrap();
    // Verify CMS read from the PDF, including its permitted zero padding.
    std::fs::write(&cms, &inspection.cms).unwrap();
    let verify = || {
        Command::new("openssl")
            .args([
                "cms",
                "-verify",
                "-binary",
                "-inform",
                "DER",
                "-noverify",
                "-in",
            ])
            .arg(&cms)
            .arg("-content")
            .arg(&content)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    };
    let result = verify();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut tampered = std::fs::read(&content).unwrap();
    tampered[10] ^= 1;
    std::fs::write(&content, tampered).unwrap();
    assert!(
        !verify().status.success(),
        "tampering must fail verification"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn encrypted_export_opens_in_apple_pdfkit() {
    use std::process::Command;
    let path = std::env::temp_dir().join(format!(
        "atelier-encrypted-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let doc = Document::open(classic()).unwrap();
    std::fs::write(
        &path,
        doc.update()
            .to_encrypted_bytes("read-password", "owner-password")
            .unwrap(),
    )
    .unwrap();
    let result = Command::new("/usr/bin/osascript").args(["-l", "JavaScript", "-e", r#"
        ObjC.import('PDFKit');
        function run(args) {
            let doc = $.PDFDocument.alloc.initWithURL($.NSURL.fileURLWithPath(args[0]));
            if (!doc || !doc.isLocked) throw Error('PDF must start locked');
            if (doc.unlockWithPassword('wrong')) throw Error('wrong password accepted');
            if (!doc.unlockWithPassword('read-password')) throw Error('correct password rejected');
            if (Number(doc.pageCount) !== 1) throw Error('page count changed');
            if (ObjC.unwrap(doc.documentAttributes.objectForKey('Title')) !== 'Before') throw Error('encrypted metadata changed');
            return 'verified';
        }
    "#]).arg(&path).output().unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "verified");
}

#[test]
fn signature_inspection_survives_incremental_edits() {
    let doc = Document::open(classic()).unwrap();
    assert!(doc.inspect_signatures().unwrap().is_empty());
    let prepared = doc.prepare_signature("Ada", 1024).unwrap();
    let signed_bytes = prepared.signing_bytes();
    let bytes = prepared.finish(&[0x30, 1, 0]).unwrap();
    let saved = Document::open(bytes.clone()).unwrap();
    let inspection = saved.inspect_signatures().unwrap().remove(0).unwrap();
    assert_eq!(inspection.signed_bytes, signed_bytes);
    assert_eq!(inspection.signer.as_deref(), Some("Ada"));
    assert_eq!(inspection.subfilter.as_deref(), Some("adbe.pkcs7.detached"));
    assert!(!inspection.has_subsequent_bytes);
    let mut update = saved.update();
    let mut info = Dict::new();
    info.insert("Title", Object::String(PdfString::from_text("After")));
    update.set(ObjRef::new(4, 0), Object::Dict(info));
    let changed = Document::open(update.to_bytes().unwrap()).unwrap();
    let inspection = changed.inspect_signatures().unwrap().remove(0).unwrap();
    assert!(inspection.has_subsequent_bytes);
    assert_eq!(inspection.signed_revision_len, bytes.len());
    assert_eq!(inspection.signed_bytes, signed_bytes);
}
