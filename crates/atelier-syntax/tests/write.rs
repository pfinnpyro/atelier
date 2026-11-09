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
    let changes = again.changes_since_signed_revision(data.len());
    let changes = changes.as_ref().as_ref().unwrap();
    assert!(
        changes.revisions[0]
            .objects
            .iter()
            .any(|d| d.category == atelier_syntax::ChangeCategory::CrossReference)
    );
    assert!(
        changes.revisions[0]
            .objects
            .iter()
            .any(|d| d.category == atelier_syntax::ChangeCategory::Metadata)
    );
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

#[test]
fn historical_signatures_survive_replacement_and_removal() {
    let doc = Document::open(classic()).unwrap();
    let first = Document::open(
        doc.prepare_signature("First", 1024)
            .unwrap()
            .finish(&[0x30, 1, 0])
            .unwrap(),
    )
    .unwrap();
    let original = first.inspect_signatures().unwrap().remove(0).unwrap();
    let second = Document::open(
        first
            .prepare_signature("Second", 1024)
            .unwrap()
            .finish(&[0x30, 1, 1])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(second.inspect_signatures().unwrap().len(), 2);
    let mut replacement = second
        .resolve_dict(&Object::Reference(original.object))
        .unwrap();
    replacement.insert("Name", Object::String(PdfString::from_text("Replacement")));
    let mut update = second.update();
    update.set(original.object, Object::Dict(replacement));
    let replaced = Document::open(update.to_bytes().unwrap()).unwrap();
    let reports = replaced
        .inspect_signatures()
        .unwrap()
        .into_iter()
        .collect::<atelier_syntax::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(reports.len(), 3);
    assert!(
        reports
            .iter()
            .any(|s| s.signer.as_deref() == Some("Replacement"))
    );
    let mut update = replaced.update();
    update.set(original.object, Object::Null);
    let removed = Document::open(update.to_bytes().unwrap()).unwrap();
    let reports = removed
        .inspect_signatures()
        .unwrap()
        .into_iter()
        .collect::<atelier_syntax::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(reports.len(), 3);
    let recovered = reports
        .iter()
        .find(|s| s.signer.as_deref() == Some("First"))
        .unwrap();
    assert_eq!(recovered.signed_bytes, original.signed_bytes);
    assert_eq!(recovered.cms, original.cms);
    assert!(recovered.has_subsequent_bytes);
}

#[test]
fn signature_history_reports_cycles_without_losing_current_signatures() {
    let doc = Document::open(classic()).unwrap();
    let signed = Document::open(
        doc.prepare_signature("Ada", 1024)
            .unwrap()
            .finish(&[0x30, 1, 0])
            .unwrap(),
    )
    .unwrap();
    let mut bytes = signed.data().to_vec();
    let marker = b"/Prev ";
    let pos = bytes
        .windows(marker.len())
        .rposition(|w| w == marker)
        .unwrap()
        + marker.len();
    let end = pos
        + bytes[pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
    let offset = signed.startxref().unwrap().to_string();
    bytes.splice(pos..end, offset.bytes());
    let cyclic = Document::open(bytes).unwrap();
    let reports = cyclic.inspect_signatures().unwrap();
    assert!(reports.iter().any(Result::is_ok));
    assert!(reports.iter().any(Result::is_err));
}

// These fixtures declare restrictions without claiming cryptographic validity.
fn declared_signature(reference: &str) -> Document {
    let doc = Document::open(classic()).unwrap();
    let mut update = doc.update();
    let object =
        atelier_syntax::Parser::new(format!("<< /Type /Sig /Reference {reference} >>").as_bytes())
            .parse_object()
            .unwrap();
    update.add(object);
    Document::open(update.to_bytes().unwrap()).unwrap()
}

#[test]
fn docmdp_and_fieldmdp_block_unclassified_edits_and_rewrites() {
    for reference in [
        "[<< /TransformMethod /DocMDP /TransformParams << /P 1 >> >>]",
        "[<< /TransformMethod /DocMDP /TransformParams << /P 2 >> >>]",
        "[<< /TransformMethod /DocMDP /TransformParams << /P 3 >> >>]",
        "[<< /TransformMethod /DocMDP >>]",
        "[<< /TransformMethod /FieldMDP /TransformParams << /Action /All >> >>]",
        "[<< /TransformMethod /FieldMDP /TransformParams << /Action /Include /Fields [(Person.Name)] >> >>]",
        "[<< /TransformMethod /FieldMDP /TransformParams << /Action /Exclude /Fields [(Allowed)] >> >>]",
    ] {
        let doc = declared_signature(reference);
        let policy = doc.signature_write_policy();
        assert!(policy.incomplete.is_empty(), "{policy:?}");
        assert!(!policy.restrictions.is_empty());
        assert_eq!(doc.update().to_bytes().unwrap(), doc.data());
        let mut update = doc.update();
        update.set(ObjRef::new(4, 0), info_with_title(b"Forbidden"));
        assert!(update.to_bytes().is_err(), "{reference}");
        assert!(update.to_compact_bytes(&Default::default()).is_err());
        assert!(doc.update().to_encrypted_bytes("open", "owner").is_err());
    }
}

#[test]
fn malformed_or_unknown_signature_policy_fails_closed() {
    for reference in [
        "42",
        "[<< /TransformMethod /DocMDP /TransformParams << /P 0 >> >>]",
        "[<< /TransformMethod /FieldMDP /TransformParams << /Action /Include >> >>]",
        "[<< /TransformMethod /FieldMDP /TransformParams << /Action /Include /Fields [42] >> >>]",
        "[<< /TransformMethod /FieldMDP /TransformParams << /Action /Unknown >> >>]",
        "[<< /TransformMethod /Unknown >>]",
    ] {
        let doc = declared_signature(reference);
        assert!(!doc.signature_write_policy().incomplete.is_empty());
        let mut update = doc.update();
        update.add(Object::Integer(123));
        assert!(update.to_bytes().is_err());
        assert_eq!(doc.update().to_bytes().unwrap(), doc.data());
    }
}

#[test]
fn approval_signature_allows_append_but_not_rewrite_or_extraction() {
    let base = Document::open(classic()).unwrap();
    let doc = Document::open(
        base.prepare_signature("Ada", 1024)
            .unwrap()
            .finish(&[0x30, 1, 0])
            .unwrap(),
    )
    .unwrap();
    let mut update = doc.update();
    update.set(ObjRef::new(4, 0), info_with_title(b"Later revision"));
    let bytes = update.to_bytes().unwrap();
    assert!(bytes.starts_with(doc.data()));
    assert!(update.to_compact_bytes(&Default::default()).is_err());
    assert!(doc.update().to_encrypted_bytes("open", "owner").is_err());
}

/// Simulate a third-party incremental writer, bypassing Atelier's write policy.
fn external_revision(doc: &Document, number: u32, body: &str) -> Document {
    let mut bytes = doc.data().to_vec();
    let offset = bytes.len();
    bytes.extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    let xref = bytes.len();
    bytes.extend_from_slice(format!(
        "xref\n{number} 1\n{offset:010} 00000 n\r\ntrailer\n<< /Size {} /Root 1 0 R /Info 4 0 R /Prev {} >>\nstartxref\n{xref}\n%%EOF\n",
        doc.next_object_number().max(number + 1), doc.startxref().unwrap()
    ).as_bytes());
    Document::open(bytes).unwrap()
}

#[test]
fn historical_restrictions_survive_indirect_parameter_replacement_and_removal() {
    let base = Document::open(classic()).unwrap();
    let mut update = base.update();
    let params = update.add(
        atelier_syntax::Parser::new(b"<< /P 1 >>")
            .parse_object()
            .unwrap(),
    );
    assert_eq!(params.num, 5);
    update.add(
        atelier_syntax::Parser::new(
            b"<< /Type /Sig /Reference [<< /TransformMethod /DocMDP /TransformParams 5 0 R >>] >>",
        )
        .parse_object()
        .unwrap(),
    );
    let doc = Document::open(update.to_bytes().unwrap()).unwrap();
    let changed = external_revision(&doc, 5, "<< /P 3 >>");
    let policy = changed.signature_write_policy();
    assert!(policy.restrictions.iter().any(|r| r.contains("P=1")));
    assert!(policy.restrictions.iter().any(|r| r.contains("P=3")));
    let removed = external_revision(&changed, 6, "null");
    assert!(removed.signature_write_policy().check(false).is_err());
    assert!(removed.signature_write_policy().check(true).is_err());
}

#[test]
fn direct_catalog_certification_is_not_silently_ignored() {
    let base = Document::open(classic()).unwrap();
    let mut update = base.update();
    let mut catalog = base.catalog().unwrap();
    catalog.insert("Perms", atelier_syntax::Parser::new(b"<< /DocMDP << /Type /Sig /Reference [<< /TransformMethod /DocMDP /TransformParams << /P 1 >> >>] >> >>").parse_object().unwrap());
    update.set(ObjRef::new(1, 0), Object::Dict(catalog));
    let doc = Document::open(update.to_bytes().unwrap()).unwrap();
    assert!(doc.signature_write_policy().has_signatures);
    assert!(doc.signature_write_policy().check(false).is_err());
}

#[test]
fn direct_nested_field_signature_policy_is_enforced_without_type_marker() {
    let base = Document::open(classic()).unwrap();
    let mut update = base.update();
    let mut catalog = base.catalog().unwrap();
    catalog.insert("AcroForm", atelier_syntax::Parser::new(b"<< /Fields [<< /FT /Sig /T (Signature) /V << /Reference [<< /TransformMethod /FieldMDP /TransformParams << /Action /All >> >>] >> >>] >>").parse_object().unwrap());
    update.set(ObjRef::new(1, 0), Object::Dict(catalog));
    let doc = Document::open(update.to_bytes().unwrap()).unwrap();
    let policy = doc.signature_write_policy();
    assert!(policy.has_signatures);
    assert!(policy.restrictions.iter().any(|r| r.contains("FieldMDP")));
    assert!(policy.check(false).is_err());
}

fn analysis_fixture() -> Document {
    let doc = Document::open(classic()).unwrap();
    Document::open(
        doc.prepare_signature("Ada", 1024)
            .unwrap()
            .finish(&[0x30, 1, 0])
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn signed_revision_analysis_reports_metadata_and_keeps_reverted_steps() {
    use atelier_syntax::{ChangeCategory, ChangeKind};
    let signed = analysis_fixture();
    let end = signed.data().len();
    let unchanged = signed.changes_since_signed_revision(end);
    assert!(unchanged.as_ref().as_ref().unwrap().revisions.is_empty());
    assert!(std::sync::Arc::ptr_eq(
        &unchanged,
        &signed.changes_since_signed_revision(end)
    ));
    let changed = external_revision(&signed, 4, "<< /Title (Changed) >>");
    let reverted = external_revision(&changed, 4, "<< /Title (Before) >>");
    let analysis = reverted.changes_since_signed_revision(end);
    let analysis = analysis.as_ref().as_ref().unwrap();
    assert_eq!(analysis.revisions.len(), 2);
    for revision in &analysis.revisions {
        assert_eq!(revision.objects.len(), 1);
        let delta = &revision.objects[0];
        assert_eq!(delta.object, ObjRef::new(4, 0));
        assert_eq!(delta.kind, ChangeKind::Modified);
        assert_eq!(delta.category, ChangeCategory::Metadata);
        assert_eq!(delta.keys, ["/Title"]);
    }
    assert!(!analysis.unclassified_trailing_bytes);
}

#[test]
fn signed_revision_analysis_distinguishes_form_appearance_page_and_signature_changes() {
    use atelier_syntax::ChangeCategory;
    let signed = analysis_fixture();
    let mut doc = signed;
    let cases = [
        (
            "<< /FT /Tx /T (Name) /V (Before) >>",
            "<< /FT /Tx /T (Name) /V (After) >>",
            ChangeCategory::FormValue,
        ),
        (
            "<< /Type /Annot /Subtype /Widget /AS /Off >>",
            "<< /Type /Annot /Subtype /Widget /AS /Yes >>",
            ChangeCategory::FormAppearance,
        ),
        (
            "<< /Type /Annot /Subtype /Text /Contents (Before) >>",
            "<< /Type /Annot /Subtype /Text /Contents (After) >>",
            ChangeCategory::Annotation,
        ),
        (
            "<< /Type /Page /Contents 4 0 R >>",
            "<< /Type /Page /Contents 5 0 R >>",
            ChangeCategory::PageContent,
        ),
        (
            "<< /Type /Page /Rotate 0 >>",
            "<< /Type /Page /Rotate 90 >>",
            ChangeCategory::PageStructure,
        ),
        (
            "<< /Type /Sig /Name (Before) >>",
            "<< /Type /Sig /Name (After) >>",
            ChangeCategory::Signature,
        ),
    ];
    for (before, after, category) in cases {
        let number = doc.next_object_number();
        let base = external_revision(&doc, number, before);
        let changed = external_revision(&base, number, after);
        let analysis = changed.changes_since_signed_revision(base.data().len());
        let analysis = analysis.as_ref().as_ref().unwrap();
        assert_eq!(analysis.revisions.len(), 1);
        assert_eq!(analysis.revisions[0].objects[0].category, category);
        doc = changed;
    }
}

#[test]
fn signed_revision_analysis_reports_added_removed_objects_and_unclassified_tail() {
    use atelier_syntax::ChangeKind;
    let signed = analysis_fixture();
    let end = signed.data().len();
    let added = external_revision(&signed, 99, "42");
    let analysis = added.changes_since_signed_revision(end);
    assert_eq!(
        analysis.as_ref().as_ref().unwrap().revisions[0].objects[0].kind,
        ChangeKind::Added
    );
    // Free the new object in a subsequent xref table.
    let mut bytes = added.data().to_vec();
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n99 1\n0000000000 00001 f\r\ntrailer\n<< /Size 100 /Root 1 0 R /Info 4 0 R /Prev {} >>\nstartxref\n{xref}\n%%EOF\n", added.startxref().unwrap()).as_bytes());
    let removed = Document::open(bytes).unwrap();
    let analysis = removed.changes_since_signed_revision(end);
    let analysis = analysis.as_ref().as_ref().unwrap();
    assert_eq!(analysis.revisions[1].objects[0].kind, ChangeKind::Removed);
    let mut bytes = signed.data().to_vec();
    bytes.extend_from_slice(b"\nunsigned trailing bytes");
    let tail = Document::open(bytes).unwrap();
    assert!(
        tail.changes_since_signed_revision(end)
            .as_ref()
            .as_ref()
            .unwrap()
            .unclassified_trailing_bytes
    );
}

#[test]
fn signed_revision_analysis_rejects_invalid_endpoints_and_foreign_chains() {
    let signed = analysis_fixture();
    let end = signed.data().len();
    for invalid in [0, 10, end + 1] {
        assert!(signed.changes_since_signed_revision(invalid).is_err());
    }
    let changed = external_revision(&signed, 4, "<< /Title (Changed) >>");
    let mut bytes = changed.data().to_vec();
    let marker = b"/Prev ";
    let pos = bytes
        .windows(marker.len())
        .rposition(|w| w == marker)
        .unwrap()
        + marker.len();
    let len = bytes[pos..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    // Skip the signed revision in the chain.
    bytes.splice(
        pos..pos + len,
        Document::open(classic())
            .unwrap()
            .startxref()
            .unwrap()
            .to_string()
            .bytes(),
    );
    let foreign = Document::open(bytes).unwrap();
    assert!(foreign.changes_since_signed_revision(end).is_err());
}

#[test]
fn signed_revision_analysis_does_not_accept_an_arbitrary_trailing_eof_marker() {
    let signed = analysis_fixture();
    let end = signed.data().len();
    let changed = external_revision(&signed, 4, "<< /Title (Changed) >>");
    let mut bytes = changed.data().to_vec();
    bytes.extend_from_slice(b"unsigned junk\n%%EOF\n");
    let doc = Document::open(bytes).unwrap();
    assert!(
        doc.changes_since_signed_revision(end)
            .as_ref()
            .as_ref()
            .unwrap()
            .unclassified_trailing_bytes
    );
    assert!(doc.changes_since_signed_revision(doc.data().len()).is_err());
}

#[test]
fn signed_revision_analysis_detects_compressed_objects_when_the_container_changes() {
    use atelier_syntax::ChangeCategory;
    fn object_stream(title: &str) -> String {
        let bodies = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into(),
            format!("<< /Title ({title}) >>"),
        ];
        let mut header = String::new();
        let mut content = String::new();
        for (i, body) in bodies.iter().enumerate() {
            header.push_str(&format!("{} {} ", i + 1, content.len()));
            content.push_str(body);
            content.push(' ');
        }
        format!(
            "<< /Type /ObjStm /N 4 /First {} /Length {} >>\nstream\n{header}{content}\nendstream",
            header.len(),
            header.len() + content.len()
        )
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let stream_at = bytes.len();
    bytes.extend_from_slice(format!("5 0 obj\n{}\nendobj\n", object_stream("Before")).as_bytes());
    let xref = bytes.len();
    let mut rows = vec![0, 0, 0, 0, 0, 255, 255];
    for index in 0..4u16 {
        rows.push(2);
        rows.extend_from_slice(&5u32.to_be_bytes());
        rows.extend_from_slice(&index.to_be_bytes());
    }
    for offset in [stream_at, xref] {
        rows.push(1);
        rows.extend_from_slice(&(offset as u32).to_be_bytes());
        rows.extend_from_slice(&[0, 0]);
    }
    bytes.extend_from_slice(format!("6 0 obj\n<< /Type /XRef /Size 7 /W [1 4 2] /Root 1 0 R /Info 4 0 R /Length {} >>\nstream\n", rows.len()).as_bytes());
    bytes.extend_from_slice(&rows);
    bytes.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let base = Document::open(bytes).unwrap();
    assert!(!base.was_repaired());
    let signed = Document::open(
        base.prepare_signature("Ada", 1024)
            .unwrap()
            .finish(&[0x30, 1, 0])
            .unwrap(),
    )
    .unwrap();
    let changed = external_revision(&signed, 5, &object_stream("After"));
    assert_eq!(signed.xref_entry(4), changed.xref_entry(4));
    let analysis = changed.changes_since_signed_revision(signed.data().len());
    let changes = &analysis.as_ref().as_ref().unwrap().revisions[0].objects;
    assert!(
        changes
            .iter()
            .any(|c| c.object.num == 4 && c.category == ChangeCategory::Metadata)
    );
    assert!(
        changes
            .iter()
            .any(|c| c.object.num == 5 && c.category == ChangeCategory::Stream)
    );
}

#[test]
fn signed_revision_analysis_bounds_history_and_rejects_repaired_input() {
    let signed = analysis_fixture();
    let end = signed.data().len();
    let mut doc = signed;
    for index in 0..129 {
        doc = external_revision(&doc, 4, &format!("<< /Title (Step {index}) >>"));
    }
    let analysis = doc.changes_since_signed_revision(end);
    assert!(
        analysis
            .as_ref()
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
    let repaired = Document::open(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n%%EOF".to_vec()).unwrap();
    assert!(
        repaired
            .changes_since_signed_revision(repaired.data().len())
            .is_err()
    );
}

fn reference_fixture(objects: &[(u32, &str)]) -> Document {
    let mut doc = analysis_fixture();
    for (number, body) in objects {
        doc = external_revision(&doc, *number, body);
    }
    doc
}

#[test]
fn revision_reference_contexts_detect_shared_and_inherited_page_resources() {
    let base = reference_fixture(&[
        (20, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        (21, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>"),
        (22, "<< /Font << /Shared 20 0 R >> >>"),
        (
            23,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
        ),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /Resources << /Font << /Shared 20 0 R /Private 21 0 R >> >> >>",
        ),
        (
            2,
            "<< /Type /Pages /Kids [3 0 R 23 0 R] /Count 2 /Resources 22 0 R /MediaBox [0 0 200 200] >>",
        ),
    ]);
    for (number, pages) in [(20, vec![1, 2]), (21, vec![1])] {
        let changed = external_revision(
            &base,
            number,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>",
        );
        let result = changed.changes_since_signed_revision(base.data().len());
        let delta = &result.as_ref().as_ref().unwrap().revisions[0].objects[0];
        assert_eq!(delta.references_before.page_contexts, pages);
        assert_eq!(delta.references_after.page_contexts, pages);
        assert!(!delta.references_before.direct_referrers.is_empty());
    }
}

#[test]
fn revision_reference_contexts_keep_qualified_fields_and_before_after_owners() {
    let base = reference_fixture(&[
        (
            20,
            "<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream",
        ),
        (21, "<< /T (Name) /FT /Tx /Parent 23 0 R /Kids [25 0 R] >>"),
        (
            22,
            "<< /T (Other) /FT /Tx /Parent 23 0 R /AP << /N 20 0 R >> >>",
        ),
        (23, "<< /T (Person) /Kids [21 0 R 22 0 R] >>"),
        (
            25,
            "<< /Type /Annot /Subtype /Widget /Parent 21 0 R /P 3 0 R /AP << /N 20 0 R >> >>",
        ),
        (24, "<< /Fields [23 0 R] >>"),
        (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm 24 0 R >>"),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [25 0 R] >>",
        ),
    ]);
    let changed = external_revision(
        &base,
        20,
        "<< /Type /XObject /Subtype /Form /Length 1 >>\nstream\nx\nendstream",
    );
    let result = changed.changes_since_signed_revision(base.data().len());
    let delta = &result.as_ref().as_ref().unwrap().revisions[0].objects[0];
    assert_eq!(
        delta.references_before.field_contexts,
        ["Person.Name", "Person.Other"]
    );
    assert_eq!(delta.references_before.page_contexts, [1]);
    assert_eq!(
        delta.references_before.direct_referrers,
        [ObjRef::new(22, 0), ObjRef::new(25, 0)]
    );
    let detached = external_revision(
        &changed,
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    );
    let changed_again = external_revision(
        &detached,
        20,
        "<< /Type /XObject /Subtype /Form /Length 1 >>\nstream\ny\nendstream",
    );
    let result = changed_again.changes_since_signed_revision(changed.data().len());
    let last = &result.as_ref().as_ref().unwrap().revisions[1].objects[0];
    assert!(last.references_after.page_contexts.is_empty());
    assert_eq!(
        last.references_after.field_contexts,
        ["Person.Name", "Person.Other"]
    );
}

#[test]
fn revision_reference_contexts_reject_cyclic_or_duplicate_field_ownership() {
    for fields in ["[21 0 R 21 0 R]", "[23 0 R]"] {
        let base = reference_fixture(&[
            (21, "<< /FT /Tx /T (Name) >>"),
            (23, "<< /T (Parent) /Kids [23 0 R] >>"),
            (24, &format!("<< /Fields {fields} >>")),
            (1, "<< /Type /Catalog /Pages 2 0 R /AcroForm 24 0 R >>"),
        ]);
        let changed = external_revision(&base, 4, "<< /Title (After) >>");
        let result = changed.changes_since_signed_revision(base.data().len());
        assert!(result.is_err());
    }
}

#[test]
fn revision_reference_contexts_follow_resource_names_that_resemble_backlinks() {
    let base = reference_fixture(&[
        (20, "<< /Flag true /Next 21 0 R >>"),
        (21, "<< /Flag true /Next 20 0 R >>"),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /Resources << /Properties << /P 20 0 R /Parent 21 0 R >> >> /MediaBox [0 0 200 200] >>",
        ),
    ]);
    let changed = external_revision(&base, 20, "<< /Flag false /Next 21 0 R >>");
    let result = changed.changes_since_signed_revision(base.data().len());
    let delta = &result.as_ref().as_ref().unwrap().revisions[0].objects[0];
    assert_eq!(delta.references_before.page_contexts, [1]);
    assert_eq!(delta.references_after.page_contexts, [1]);
    assert!(
        delta
            .references_before
            .direct_referrers
            .contains(&ObjRef::new(21, 0))
    );
}

#[test]
fn revision_reference_contexts_show_detachment_in_the_same_transaction() {
    let base = reference_fixture(&[
        (20, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 20 0 R >> >> /MediaBox [0 0 200 200] >>",
        ),
    ]);
    let mut update = base.update();
    update.set(
        ObjRef::new(20, 0),
        atelier_syntax::Parser::new(b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>")
            .parse_object()
            .unwrap(),
    );
    update.set(
        ObjRef::new(3, 0),
        atelier_syntax::Parser::new(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>")
            .parse_object()
            .unwrap(),
    );
    let changed = Document::open(update.to_bytes().unwrap()).unwrap();
    let result = changed.changes_since_signed_revision(base.data().len());
    let delta = result.as_ref().as_ref().unwrap().revisions[0]
        .objects
        .iter()
        .find(|d| d.object.num == 20)
        .unwrap();
    assert_eq!(delta.references_before.page_contexts, [1]);
    assert!(delta.references_after.page_contexts.is_empty());
    assert_eq!(
        delta.references_before.direct_referrers,
        [ObjRef::new(3, 0)]
    );
    assert!(delta.references_after.direct_referrers.is_empty());
}
