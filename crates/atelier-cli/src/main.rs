//! `atelier`: inspect and render PDF files with the Atelier engine.

mod png;

use std::io::Write;
use std::time::Instant;

use atelier_render::{RenderOptions, extract_text, render_page};
use std::process::ExitCode;

use atelier_syntax::{Access, Document, Error, ObjRef, Object, XrefEntry};

const USAGE: &str = "\
Usage:
  atelier info <file.pdf>                  version, structure and page count
  atelier pages <file.pdf>                 one line per page: boxes and rotation
  atelier objects <file.pdf>               every object number and where it lives
  atelier obj <file.pdf> <num> [gen]       print one object
  atelier stream <file.pdf> <num> [gen]    write a stream's decoded bytes to stdout
  atelier content <file.pdf> <page>        write a page's decoded content (page from 1)
  atelier render <file.pdf> <page|all> [--dpi N] [-o out.png]
                                           render pages to PNG (default 150 dpi; with
                                           `all`, -o is a prefix: out-1.png, out-2.png...)
  atelier bench <file.pdf> [--dpi N] [--pages N] [--threads N]
                                           time drawing the first pages (default 8) in
                                           tiles, the way the viewer draws them
  atelier text <file.pdf> [page|all] [--glyphs]
                                           print the text of pages (default all); with
                                           --glyphs, one line per glyph with its box
  atelier fields <file.pdf>                list form fields, their values and widgets
  atelier fill <file.pdf> -o <out.pdf> [name=value ...] [--refresh]
                                           set field values and save as an incremental
                                           update. Checkboxes take on, off or a state
                                           name; multi-select lists take a|b|c.
                                           --refresh redraws every field's appearance
  atelier detect <file.pdf> [-o out.pdf] [--tables]
                                           suggest form fields for the boxes, lines
                                           and tables of a printed form, page by page;
                                           -o saves a copy with a field made of each
                                           suggestion; --tables prints each table's
                                           text, tab-separated
  atelier actions <file.pdf>               list scripts and other actions (never run)
  atelier compat <file.pdf>                XFA (static or dynamic), scripts that depend
                                           on the viewer, and embedded files
  atelier attachments <file.pdf> [--save N -o out]
                                           list embedded files with their type and
                                           SHA-256; --save writes file N (from 1) out
  atelier orient <file.pdf>                which way up each page reads, judged from
                                           its text, or for scans from the image
  atelier ocr <file.pdf> -o <out.pdf> [--upright] [--models <dir>]
                                           read scanned pages and save a searchable
                                           copy (invisible text over each scan);
                                           --upright also turns pages the right way up.
                                           Models: Atelier's OCR models folder
  atelier arrange <file.pdf> <pages> -o <out.pdf>
                                           save the pages listed, in that order
                                           (e.g. 3,1-2); pages left out are removed
  atelier extract <file.pdf> <pages> -o <out.pdf>
                                           save the pages listed as a new file
  atelier merge <file.pdf> <more.pdf...> -o <out.pdf>
                                           add the other files' pages at the end
  atelier keygen <name> -o <base> [--curve p256|p384]
                                           make an OpenPGP key (NIST P-256 unless told
                                           otherwise) for `Name <address>`: writes
                                           <base>.cert.asc to give to others and
                                           <base>.key.asc to keep, which is NOT locked
  atelier protect <file.pdf> -o <out.pdf> [--to <cert.asc>]... [--passphrase <pw>]
                                           encrypt the whole file with OpenPGP for the
                                           owners of the certificates, a passphrase,
                                           or both; other viewers see a cover page
  atelier unprotect <file.pdf> -o <out.pdf>
                                           save the PDF inside a protected file

Every command takes --password <pw> for files with PDF's own encryption, and
--key <key.asc> or --passphrase <pw> for files protected with OpenPGP.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let mut password = String::new();
    if let Some(i) = args.iter().position(|a| a == "--password") {
        password = args.get(i + 1).ok_or("--password needs a value")?.clone();
        args.drain(i..i + 2);
    }
    let key_files = take_all(&mut args, "--key")?;
    let cert_files = take_all(&mut args, "--to")?;
    let passphrase = take_all(&mut args, "--passphrase")?.pop();
    let args = &args[..];
    let (Some(command), Some(path)) = (args.first(), args.get(1)) else {
        return Err(format!("missing arguments\n\n{USAGE}"));
    };
    let mut out = std::io::stdout().lock();
    if command == "keygen" {
        return keygen(path, &args[2..], &mut out);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let open = |bytes: Vec<u8>| match Document::open_with_password(bytes, password.as_bytes()) {
        Err(Error::Password) if password.is_empty() => {
            Err("this PDF is protected by a password; pass --password".to_string())
        }
        Err(Error::Password) => Err("wrong password".to_string()),
        other => other.map_err(|e| e.to_string()),
    };
    let mut doc = open(bytes)?;
    // A file protected with OpenPGP is a cover page around the real one.
    if let Some(payload) = atelier_pgp::protected_payload(&doc).map_err(|e| e.to_string())? {
        let keys = key_files
            .iter()
            .map(|f| {
                let data = std::fs::read(f).map_err(|e| format!("cannot read {f}: {e}"))?;
                atelier_pgp::SecretKey::parse(atelier_pgp::provider(), &data)
                    .map_err(|e| format!("{f}: {e}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if keys.is_empty() && passphrase.is_none() {
            let said = atelier_pgp::payload_summary(&payload).map_err(|e| e.to_string())?;
            let mut ways = Vec::new();
            if !said.recipients.is_empty() {
                ways.push("--key <key.asc>");
            }
            if said.passphrase {
                ways.push("--passphrase <pw>");
            }
            let mut message = format!(
                "this PDF is protected with OpenPGP; pass {}",
                ways.join(" or ")
            );
            for fingerprint in said.recipients {
                let hex: String = fingerprint.iter().map(|b| format!("{b:02X}")).collect();
                message.push_str(&format!("\n  encrypted to key {hex}"));
            }
            return Err(message);
        }
        let keys: Vec<&atelier_pgp::SecretKey> = keys.iter().collect();
        let pdf = atelier_pgp::unprotect_pdf(
            atelier_pgp::provider(),
            &payload,
            &keys,
            passphrase.as_deref().map(str::as_bytes),
        )
        .map_err(|e| e.to_string())?;
        doc = open(pdf)?;
        if command == "unprotect" {
            return save(&doc, &args[2..], "unprotect", &mut out);
        }
    } else if command == "unprotect" {
        return Err("this PDF is not protected with OpenPGP".into());
    }
    if command == "protect" {
        return protect(
            &doc,
            &args[2..],
            &cert_files,
            passphrase.as_deref(),
            &mut out,
        );
    }

    let result = match command.as_str() {
        "info" => info(&doc, &mut out),
        "pages" => pages(&doc, &mut out),
        "objects" => objects(&doc, &mut out),
        "obj" => object(&doc, obj_ref(&args[2..])?, &mut out),
        "stream" => stream(&doc, obj_ref(&args[2..])?, &mut out),
        "content" => content(&doc, &args[2..], &mut out),
        "render" => render(&doc, path, &args[2..], &mut out),
        "bench" => bench(&doc, &args[2..], &mut out),
        "text" => text(&doc, &args[2..], &mut out),
        "fields" => fields(&doc, &mut out),
        "fill" => fill(&doc, &args[2..], &mut out),
        "detect" => detect(&doc, &args[2..], &mut out),
        "actions" => actions(&doc, &mut out),
        "compat" => compat(&doc, &mut out),
        "attachments" => attachments(&doc, &args[2..], &mut out),
        "orient" => orient(&doc, &mut out),
        "ocr" => ocr(&doc, &args[2..], &mut out),
        "arrange" | "extract" | "merge" => organize(command, &doc, &args[2..], &mut out),
        other => return Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    for warning in doc.warnings() {
        eprintln!("warning: {warning}");
    }
    result
}

type Out<'a> = std::io::StdoutLock<'a>;

/// Takes every `flag value` pair out of the arguments.
fn take_all(args: &mut Vec<String>, flag: &str) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    while let Some(i) = args.iter().position(|a| a == flag) {
        if i + 1 >= args.len() {
            return Err(format!("{flag} needs a value"));
        }
        found.push(args.remove(i + 1));
        args.remove(i);
    }
    Ok(found)
}

fn output_file(args: &[String], command: &str) -> Result<String, String> {
    match args {
        [flag, file] if flag == "-o" => Ok(file.clone()),
        _ => Err(format!("{command} needs -o <out> and nothing else")),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn keygen(name: &str, args: &[String], out: &mut Out) -> Result<(), String> {
    let mut curve = atelier_pgp::Curve::P256;
    let mut base = None;
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1);
        match (args[i].as_str(), value.map(String::as_str)) {
            ("-o", Some(v)) => base = Some(v.to_string()),
            ("--curve", Some("p256")) => curve = atelier_pgp::Curve::P256,
            ("--curve", Some("p384")) => curve = atelier_pgp::Curve::P384,
            ("--curve", _) => return Err("--curve takes p256 or p384".into()),
            (a, _) => return Err(format!("unexpected `{a}`")),
        }
        i += 2;
    }
    let base = base.ok_or("keygen needs -o <base>")?;
    let key = atelier_pgp::SecretKey::generate(atelier_pgp::provider(), curve, name, now())
        .map_err(|e| e.to_string())?;
    let (cert_file, key_file) = (format!("{base}.cert.asc"), format!("{base}.key.asc"));
    for file in [&cert_file, &key_file] {
        if std::path::Path::new(file).exists() {
            return Err(format!("{file} already exists"));
        }
    }
    std::fs::write(&cert_file, key.certificate().to_armored())
        .map_err(|e| format!("cannot write {cert_file}: {e}"))?;
    std::fs::write(&key_file, key.to_armored())
        .map_err(|e| format!("cannot write {key_file}: {e}"))?;
    writeln!(out, "Key {}", key.certificate().fingerprint_hex()).map_err(io)?;
    writeln!(
        out,
        "Wrote {cert_file}: give this to people who will encrypt for you"
    )
    .map_err(io)?;
    writeln!(
        out,
        "Wrote {key_file}: keep this to yourself. It is not locked with a passphrase,\n\
         so anyone who can read the file can read what is encrypted for you."
    )
    .map_err(io)
}

fn protect(
    doc: &Document,
    args: &[String],
    cert_files: &[String],
    passphrase: Option<&str>,
    out: &mut Out,
) -> Result<(), String> {
    let output = output_file(args, "protect")?;
    if cert_files.is_empty() && passphrase.is_none() {
        return Err("protect needs --to <cert.asc>, --passphrase <pw>, or both".into());
    }
    let certs = cert_files
        .iter()
        .map(|f| {
            let data = std::fs::read(f).map_err(|e| format!("cannot read {f}: {e}"))?;
            atelier_pgp::Certificate::parse(atelier_pgp::provider(), &data)
                .map_err(|e| format!("{f}: {e}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let to: Vec<&atelier_pgp::Certificate> = certs.iter().collect();
    let bytes = atelier_pgp::protect_pdf(
        atelier_pgp::provider(),
        doc.data(),
        &to,
        passphrase.map(str::as_bytes),
        now(),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(&output, &bytes).map_err(|e| format!("cannot write {output}: {e}"))?;
    for cert in &certs {
        let who = cert.user_ids().first().map_or("(no name)", String::as_str);
        writeln!(out, "Encrypted for {who} {}", cert.fingerprint_hex()).map_err(io)?;
    }
    if passphrase.is_some() {
        writeln!(out, "Encrypted under the passphrase").map_err(io)?;
    }
    writeln!(out, "Wrote {output} ({} bytes)", bytes.len()).map_err(io)
}

/// Writes the document's bytes as they are.
fn save(doc: &Document, args: &[String], command: &str, out: &mut Out) -> Result<(), String> {
    let output = output_file(args, command)?;
    std::fs::write(&output, doc.data()).map_err(|e| format!("cannot write {output}: {e}"))?;
    writeln!(
        out,
        "Wrote {output} ({} bytes), not encrypted",
        doc.data().len()
    )
    .map_err(io)
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

fn obj_ref(args: &[String]) -> Result<ObjRef, String> {
    let num = args
        .first()
        .ok_or("missing object number")?
        .parse()
        .map_err(|_| "object number must be a number")?;
    let generation = match args.get(1) {
        Some(g) => g.parse().map_err(|_| "generation must be a number")?,
        None => 0,
    };
    Ok(ObjRef::new(num, generation))
}

fn info(doc: &Document, out: &mut Out) -> Result<(), String> {
    writeln!(out, "PDF version:  {}", doc.version()).map_err(io)?;
    writeln!(out, "Objects:      {}", doc.object_numbers().len()).map_err(io)?;
    writeln!(
        out,
        "Repaired:     {}",
        if doc.was_repaired() { "yes" } else { "no" }
    )
    .map_err(io)?;
    let encrypted = match doc.access() {
        None => "no",
        Some(Access::User) => "yes (opened with the user password)",
        Some(Access::Owner) => "yes (opened with the owner password)",
    };
    writeln!(out, "Encrypted:    {encrypted}").map_err(io)?;
    match doc.pages() {
        Ok(p) => writeln!(out, "Pages:        {}", p.len()).map_err(io)?,
        Err(e) => writeln!(out, "Pages:        unreadable ({e})").map_err(io)?,
    }
    if let Some(info) = doc.info() {
        for (key, value) in info.iter() {
            let value = doc.resolve(value).unwrap_or(Object::Null);
            writeln!(out, "{:<13} {}", format!("{}:", key.as_str_lossy()), value).map_err(io)?;
        }
    }
    writeln!(out, "Trailer:      {}", doc.trailer()).map_err(io)
}

fn pages(doc: &Document, out: &mut Out) -> Result<(), String> {
    let pages = doc.pages().map_err(|e| e.to_string())?;
    for (i, page) in pages.iter().enumerate() {
        let m = page.media_box;
        let r = page.obj_ref.map_or("direct".to_string(), |r| r.to_string());
        writeln!(
            out,
            "{:>5}  {:<12} {:>8.2} x {:<8.2} rotate {:<3}  media [{} {} {} {}]",
            i + 1,
            r,
            page.crop_box.width(),
            page.crop_box.height(),
            page.rotate,
            m.x0,
            m.y0,
            m.x1,
            m.y1
        )
        .map_err(io)?;
    }
    Ok(())
}

fn orient(doc: &Document, out: &mut Out) -> Result<(), String> {
    let pages = doc.pages().map_err(|e| e.to_string())?;
    for (i, page) in pages.iter().enumerate() {
        let verdict = match atelier_view::upright_page(doc, page) {
            Some((r, basis)) if r == page.rotate => format!("upright ({basis:?})"),
            Some((r, basis)) => format!("turn to rotate {r} ({basis:?})"),
            None => "cannot tell".to_string(),
        };
        writeln!(out, "{:>5}  rotate {:<3}  {verdict}", i + 1, page.rotate).map_err(io)?;
    }
    Ok(())
}

/// Where the viewer keeps the OCR models.
fn models_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Atelier"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("atelier"))
    }
    .map(|d| d.join("ocr-models"))
}

fn ocr(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    use std::sync::Arc;
    let mut output = None;
    let mut models = models_dir();
    let mut upright = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                output = Some(args.get(i + 1).ok_or("-o needs a file name")?.clone());
                i += 1;
            }
            "--models" => {
                models = Some(args.get(i + 1).ok_or("--models needs a folder")?.into());
                i += 1;
            }
            "--upright" => upright = true,
            a => return Err(format!("unexpected `{a}`")),
        }
        i += 1;
    }
    let output = output.ok_or("ocr needs -o <out.pdf>")?;
    let models = models.ok_or("say where the OCR models are with --models")?;
    if !atelier_ocr::models::installed(&models) {
        return Err(format!(
            "the OCR models are not in {}; get them in the viewer (Settings) first",
            models.display()
        ));
    }
    let engine = atelier_ocr::Engine::load(&models)?;
    let base = Arc::new(doc.reopen(doc.data().to_vec()).map_err(|e| e.to_string())?);
    let mut session = atelier_form::Session::new(base.clone());
    let pages = doc.pages().map_err(|e| e.to_string())?;
    let mut read = 0;
    for (n, page) in pages.iter().enumerate() {
        let own = atelier_render::extract_text(doc, page).unwrap_or_default();
        let letters = own
            .glyphs
            .iter()
            .filter(|g| !g.text.trim().is_empty())
            .count();
        if letters >= 12 {
            writeln!(out, "{:>5}  has text", n + 1).map_err(io)?;
            continue;
        }
        match atelier_ocr::read_page(&engine, doc, page)? {
            Some(r) => {
                let words = r.text.plain_text().split_whitespace().count();
                if upright {
                    session.rotate(n, i32::from(r.rotate));
                }
                session.set_text_layer(n, Arc::new(r.text));
                read += 1;
                writeln!(
                    out,
                    "{:>5}  read {words} words, upright at rotate {}",
                    n + 1,
                    r.rotate
                )
                .map_err(io)?;
            }
            None => writeln!(out, "{:>5}  nothing readable", n + 1).map_err(io)?,
        }
    }
    let built = session.build().map_err(|e| e.to_string())?;
    std::fs::write(&output, built.doc.data()).map_err(io)?;
    writeln!(out, "Read {read} pages; wrote {output}").map_err(io)?;
    Ok(())
}

/// Page numbers as typed ("3,1-2"), from 1, as indices.
fn page_numbers(list: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let number = |s: &str| -> Result<usize, String> {
            match s.trim().parse::<usize>() {
                Ok(n) if (1..=count).contains(&n) => Ok(n - 1),
                _ => Err(format!("`{s}` is not a page from 1 to {count}")),
            }
        };
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (number(a)?, number(b)?);
                if a <= b {
                    out.extend(a..=b);
                } else {
                    out.extend((b..=a).rev());
                }
            }
            None => out.push(number(part)?),
        }
    }
    if out.is_empty() {
        return Err("no pages listed".into());
    }
    Ok(out)
}

/// `arrange`, `extract` and `merge`.
fn organize(command: &str, doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    use std::sync::Arc;
    let mut output = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" {
            output = Some(args.get(i + 1).ok_or("-o needs a file name")?.clone());
            i += 1;
        } else {
            rest.push(args[i].clone());
        }
        i += 1;
    }
    let output = output.ok_or(format!("{command} needs -o <out.pdf>"))?;
    let base = Arc::new(doc.reopen(doc.data().to_vec()).map_err(|e| e.to_string())?);
    let mut session = atelier_form::Session::new(base);
    let count = session.page_count();
    let bytes = match command {
        "extract" => {
            let [list] = rest.as_slice() else {
                return Err("extract takes one list of pages, such as 1,3-4".into());
            };
            session
                .extract(&page_numbers(list, count)?)
                .map_err(|e| e.to_string())?
        }
        "arrange" => {
            let [list] = rest.as_slice() else {
                return Err("arrange takes one list of pages, such as 3,1-2".into());
            };
            let wanted = page_numbers(list, count)?;
            let mut seen = std::collections::HashSet::new();
            if let Some(twice) = wanted.iter().find(|p| !seen.insert(**p)) {
                return Err(format!("page {} is listed twice", twice + 1));
            }
            let gone: Vec<usize> = (0..count).filter(|p| !seen.contains(p)).collect();
            session.delete_pages(&gone);
            // What is left keeps its old order; put each where it goes.
            let mut now: Vec<usize> = (0..count).filter(|p| seen.contains(p)).collect();
            for (k, page) in wanted.iter().enumerate() {
                let at = now.iter().position(|p| p == page).unwrap_or(k);
                let old = session.move_pages(&[at], k);
                now = old.iter().map(|&o| now[o]).collect();
            }
            session
                .build()
                .map_err(|e| e.to_string())?
                .doc
                .data()
                .to_vec()
        }
        _ => {
            if rest.is_empty() {
                return Err("merge needs at least one more file".into());
            }
            for path in &rest {
                let data = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
                let other = Document::open(data).map_err(|e| format!("{path}: {e}"))?;
                let end = session.page_count();
                session
                    .insert_file(Arc::new(other), end)
                    .map_err(|e| format!("{path}: {e}"))?;
            }
            session
                .build()
                .map_err(|e| e.to_string())?
                .doc
                .data()
                .to_vec()
        }
    };
    std::fs::write(&output, &bytes).map_err(io)?;
    let pages = Document::open(bytes)
        .and_then(|d| d.pages().map(<[_]>::len))
        .map_err(|e| format!("wrote {output}, but it does not read back: {e}"))?;
    writeln!(out, "Wrote {output} ({pages} pages)").map_err(io)?;
    Ok(())
}

fn objects(doc: &Document, out: &mut Out) -> Result<(), String> {
    for num in doc.object_numbers() {
        let place = match doc.xref_entry(num) {
            Some(XrefEntry::Offset { offset, generation }) => {
                format!("gen {generation} at byte {offset}")
            }
            Some(XrefEntry::Compressed { stream, index }) => {
                format!("in object stream {stream}, index {index}")
            }
            _ => continue,
        };
        writeln!(out, "{num:>8}  {place}").map_err(io)?;
    }
    Ok(())
}

fn object(doc: &Document, r: ObjRef, out: &mut Out) -> Result<(), String> {
    let obj = doc.get(r).map_err(|e| e.to_string())?;
    match &obj {
        Object::Stream(s) => {
            writeln!(out, "{}", s.dict).map_err(io)?;
            let decoded = doc.decode_stream(s);
            let summary = match decoded {
                Ok(d) => match d.image_filter {
                    Some((f, _)) => format!("{} bytes, still {f:?} encoded", d.data.len()),
                    None => format!("{} bytes decoded", d.data.len()),
                },
                Err(e) => format!("cannot decode: {e}"),
            };
            writeln!(out, "stream: {} bytes encoded, {summary}", s.data.len()).map_err(io)
        }
        other => writeln!(out, "{other}").map_err(io),
    }
}

fn stream(doc: &Document, r: ObjRef, out: &mut Out) -> Result<(), String> {
    let obj = doc.get(r).map_err(|e| e.to_string())?;
    let s = obj
        .as_stream()
        .ok_or(format!("object {r} is a {}, not a stream", obj.type_name()))?;
    let decoded = doc.decode_stream(s).map_err(|e| e.to_string())?;
    out.write_all(&decoded.data).map_err(io)
}

fn content(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    let n: usize = args
        .first()
        .ok_or("missing page number")?
        .parse()
        .map_err(|_| "page number must be a number")?;
    let pages = doc.pages().map_err(|e| e.to_string())?;
    let page = n.checked_sub(1).and_then(|i| pages.get(i)).ok_or(format!(
        "page {n} does not exist (the file has {})",
        pages.len()
    ))?;
    let data = page.contents(doc).map_err(|e| e.to_string())?;
    out.write_all(&data).map_err(io)
}

/// Times drawing pages in tiles, as the viewer does: every tile is its
/// own call to the renderer, spread over worker threads.
fn bench(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (mut dpi, mut count, mut threads) = (144.0, 8usize, 1usize);
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).ok_or("option needs a value")?;
        match args[i].as_str() {
            "--dpi" => {
                dpi = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| *v > 0.0 && *v <= 2400.0)
                    .ok_or("--dpi needs a number between 1 and 2400")?;
            }
            "--pages" => count = value.parse().map_err(|_| "--pages needs a number")?,
            "--threads" => {
                threads = value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| (1..=64).contains(n))
                    .ok_or("--threads needs a number from 1 to 64")?;
            }
            other => return Err(format!("unknown option `{other}`")),
        }
        i += 2;
    }
    let pages = doc.pages().map_err(|e| e.to_string())?;
    let scale = dpi / 72.0;
    let options = RenderOptions {
        scale,
        ..RenderOptions::default()
    };
    let tile = atelier_view::TILE_SIZE;
    let mut jobs = Vec::new();
    for (n, page) in pages.iter().enumerate().take(count) {
        let (_, w, h) = atelier_render::page_transform(page, scale);
        for y in (0..h).step_by(tile) {
            for x in (0..w).step_by(tile) {
                jobs.push((n, (x, y, tile.min(w - x), tile.min(h - y))));
            }
        }
    }
    let next = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let start = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                while let Some((n, region)) = jobs.get(next.fetch_add(1, Ordering::SeqCst)) {
                    if atelier_render::render_region(doc, &pages[*n], &options, *region).is_err() {
                        failed.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    let tiled = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for page in pages.iter().take(count) {
        let _ = render_page(doc, page, &options);
    }
    let whole = start.elapsed().as_secs_f64() * 1000.0;
    writeln!(
        out,
        "{} pages at {dpi} dpi: {} tiles in {tiled:.0} ms on {threads} threads \
         ({:.1} ms a tile, {} failed); whole pages in {whole:.0} ms",
        pages.len().min(count),
        jobs.len(),
        tiled / jobs.len().max(1) as f64,
        failed.load(Ordering::SeqCst),
    )
    .map_err(|e| e.to_string())
}

fn render(doc: &Document, path: &str, args: &[String], out: &mut Out) -> Result<(), String> {
    let which = args.first().ok_or("missing page number (or `all`)")?;
    let mut dpi = 150.0;
    let mut output: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--dpi" => {
                dpi = args
                    .get(i + 1)
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|v| *v > 0.0 && *v <= 2400.0)
                    .ok_or("--dpi needs a number between 1 and 2400")?;
                i += 2;
            }
            "-o" => {
                output = Some(args.get(i + 1).ok_or("-o needs a file name")?.clone());
                i += 2;
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    let pages = doc.pages().map_err(|e| e.to_string())?;
    let numbers: Vec<usize> = if which == "all" {
        (1..=pages.len()).collect()
    } else {
        vec![
            which
                .parse()
                .map_err(|_| "page must be a number or `all`")?,
        ]
    };
    let stem = std::path::Path::new(path)
        .file_stem()
        .map_or("page".to_string(), |s| s.to_string_lossy().into_owned());
    let options = RenderOptions {
        scale: dpi / 72.0,
        ..RenderOptions::default()
    };
    for n in numbers {
        let page = n.checked_sub(1).and_then(|i| pages.get(i)).ok_or(format!(
            "page {n} does not exist (the file has {})",
            pages.len()
        ))?;
        let start = Instant::now();
        let rendered = render_page(doc, page, &options).map_err(|e| e.to_string())?;
        let elapsed = start.elapsed();
        let file = match (&output, which == "all") {
            (Some(o), false) => o.clone(),
            (Some(o), true) => format!("{o}-{n}.png"),
            (None, _) => format!("{stem}-{n}.png"),
        };
        let pm = &rendered.pixmap;
        let png = png::encode_rgba(pm.width(), pm.height(), &pm.to_rgba());
        std::fs::write(&file, png).map_err(|e| format!("cannot write {file}: {e}"))?;
        writeln!(
            out,
            "page {n}: {}x{} px in {:.1} ms -> {file}",
            pm.width(),
            pm.height(),
            elapsed.as_secs_f64() * 1000.0
        )
        .map_err(io)?;
        for w in rendered.warnings {
            eprintln!("  page {n}: {w}");
        }
    }
    Ok(())
}

fn text(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    let glyphs = args.iter().any(|a| a == "--glyphs");
    let which = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map_or("all", String::as_str);
    let pages = doc.pages().map_err(|e| e.to_string())?;
    let numbers: Vec<usize> = if which == "all" {
        (1..=pages.len()).collect()
    } else {
        vec![
            which
                .parse()
                .map_err(|_| "page must be a number or `all`")?,
        ]
    };
    for n in numbers {
        let page = n.checked_sub(1).and_then(|i| pages.get(i)).ok_or(format!(
            "page {n} does not exist (the file has {})",
            pages.len()
        ))?;
        let text = extract_text(doc, page).map_err(|e| e.to_string())?;
        if glyphs {
            for g in &text.glyphs {
                let [a, _, c, _] = g.quad;
                writeln!(
                    out,
                    "{n} {:?} {:.2} {:.2} {:.2} {:.2} {:.2} {}",
                    g.text, a.x, a.y, c.x, c.y, g.size, g.font
                )
                .map_err(io)?;
            }
        } else {
            writeln!(out, "{}", text.plain_text()).map_err(io)?;
            if n != pages.len() {
                out.write_all(b"\x0c").map_err(io)?;
            }
        }
    }
    Ok(())
}

fn load_form(doc: &Document) -> Result<atelier_form::Form, String> {
    atelier_form::Form::load(doc)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "this PDF has no form fields".to_string())
}

fn fields(doc: &Document, out: &mut Out) -> Result<(), String> {
    use atelier_form::flags;
    let form = load_form(doc)?;
    if form.has_xfa {
        writeln!(out, "(also has an XFA form, which Atelier ignores)").map_err(io)?;
    }
    if form.need_appearances {
        writeln!(out, "(NeedAppearances is set: viewers redraw fields)").map_err(io)?;
    }
    for f in &form.fields {
        let mut notes = Vec::new();
        for (flag, label) in [
            (flags::READ_ONLY, "read-only"),
            (flags::REQUIRED, "required"),
        ] {
            if f.has_flag(flag) {
                notes.push(label);
            }
        }
        if f.kind == atelier_form::FieldKind::Text {
            for (flag, label) in [
                (flags::MULTILINE, "multiline"),
                (flags::PASSWORD, "password"),
                (flags::COMB, "comb"),
            ] {
                if f.has_flag(flag) {
                    notes.push(label);
                }
            }
        }
        if f.kind == atelier_form::FieldKind::ListBox && f.has_flag(flags::MULTI_SELECT) {
            notes.push("multi-select");
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" [{}]", notes.join(", "))
        };
        writeln!(
            out,
            "{} ({}){notes} = {:?}",
            f.name,
            f.kind.label(),
            f.display_value()
        )
        .map_err(io)?;
        if let Some(t) = &f.tooltip {
            writeln!(out, "    tooltip: {t}").map_err(io)?;
        }
        if !f.options.is_empty() {
            let opts: Vec<String> = f
                .options
                .iter()
                .map(|o| {
                    if o.export == o.display {
                        o.display.clone()
                    } else {
                        format!("{} ({})", o.display, o.export)
                    }
                })
                .collect();
            writeln!(out, "    options: {}", opts.join(" | ")).map_err(io)?;
        }
        let states = f.states();
        if !states.is_empty() {
            writeln!(out, "    states: {}", states.join(", ")).map_err(io)?;
        }
        for w in &f.widgets {
            let page = w.page.map_or("?".to_string(), |p| (p + 1).to_string());
            writeln!(
                out,
                "    widget {} on page {page} at [{:.1} {:.1} {:.1} {:.1}]{}",
                w.obj,
                w.rect.x0,
                w.rect.y0,
                w.rect.x1,
                w.rect.y1,
                if w.is_hidden() { " hidden" } else { "" }
            )
            .map_err(io)?;
        }
    }
    Ok(())
}

fn fill(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    use atelier_form::{FieldKind, Value};
    let mut form = load_form(doc)?;
    let mut output = None;
    let mut refresh = false;
    let mut sets = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                output = Some(args.get(i + 1).ok_or("-o needs a file name")?.clone());
                i += 1;
            }
            "--refresh" => refresh = true,
            a => {
                let (name, value) = a
                    .split_once('=')
                    .ok_or_else(|| format!("expected name=value, got `{a}`"))?;
                sets.push((name.to_string(), value.to_string()));
            }
        }
        i += 1;
    }
    let output = output.ok_or("fill needs -o <out.pdf>")?;
    let mut update = doc.update();
    if refresh {
        for index in 0..form.fields.len() {
            for w in form
                .refresh_appearance(&mut update, index)
                .map_err(|e| e.to_string())?
            {
                eprintln!("warning: {w}");
            }
        }
    }
    for (name, text) in sets {
        let index = form
            .fields
            .iter()
            .position(|f| f.name == name)
            .ok_or_else(|| format!("no field named `{name}`"))?;
        let field = &form.fields[index];
        let value = match field.kind {
            FieldKind::Checkbox | FieldKind::Radio => match text.as_str() {
                "off" | "Off" | "" => Value::None,
                "on" => Value::State(
                    field
                        .states()
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "Yes".to_string()),
                ),
                s => Value::State(s.to_string()),
            },
            FieldKind::ListBox => Value::Choices(text.split('|').map(str::to_string).collect()),
            _ => Value::Text(text),
        };
        for w in form
            .set_value(&mut update, index, value)
            .map_err(|e| e.to_string())?
        {
            eprintln!("warning: {w}");
        }
    }
    let bytes = update.to_bytes().map_err(|e| e.to_string())?;
    std::fs::write(&output, &bytes).map_err(|e| format!("cannot write {output}: {e}"))?;
    writeln!(out, "wrote {output} ({} bytes)", bytes.len()).map_err(io)?;
    Ok(())
}

/// Lists the tables and suggested fields of each page; with `-o`, saves
/// a copy that has them as fields.
fn detect(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    let mut output = None;
    let mut tables = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                output = Some(args.get(i + 1).ok_or("-o needs a file name")?.clone());
                i += 1;
            }
            "--tables" => tables = true,
            a => return Err(format!("unknown option `{a}`")),
        }
        i += 1;
    }
    let pages = doc.pages().map_err(|e| e.to_string())?;
    let form = atelier_form::Form::load(doc).ok().flatten();
    let mut suggested = Vec::new();
    for (n, page) in pages.iter().enumerate() {
        let text = extract_text(doc, page).unwrap_or_default();
        // Places that already have a field are left alone.
        let taken: Vec<_> = form
            .iter()
            .flat_map(|f| &f.fields)
            .flat_map(|f| &f.widgets)
            .filter(|w| w.page == Some(n))
            .map(|w| w.rect)
            .collect();
        let found = atelier_form::detect_page(doc, page, &text, &taken);
        for t in &found.tables {
            writeln!(
                out,
                "page {} table {} x {} [{:.1} {:.1} {:.1} {:.1}]",
                n + 1,
                t.rows,
                t.cols,
                t.rect.x0,
                t.rect.y0,
                t.rect.x1,
                t.rect.y1
            )
            .map_err(io)?;
            if tables {
                writeln!(out, "{}", t.to_tsv()).map_err(io)?;
            }
        }
        for s in found.fields {
            let (f, r) = (&s.field, s.field.rect);
            writeln!(
                out,
                "page {} {:?} {} from a {} [{:.1} {:.1} {:.1} {:.1}]",
                n + 1,
                f.name,
                f.kind.label().to_lowercase(),
                s.source.label(),
                r.x0,
                r.y0,
                r.x1,
                r.y1
            )
            .map_err(io)?;
            suggested.push((n, s.field));
        }
    }
    let Some(output) = output else {
        return Ok(());
    };
    let count = suggested.len();
    let base = doc.reopen(doc.data().to_vec()).map_err(|e| e.to_string())?;
    let mut session = atelier_form::Session::new(std::sync::Arc::new(base));
    for (page, field) in suggested {
        session.add_field(page, field).map_err(|e| e.to_string())?;
    }
    let built = session.build().map_err(|e| e.to_string())?;
    std::fs::write(&output, built.doc.data()).map_err(|e| format!("cannot write {output}: {e}"))?;
    writeln!(
        out,
        "wrote {output} with {count} new {}",
        if count == 1 { "field" } else { "fields" }
    )
    .map_err(io)?;
    Ok(())
}

fn actions(doc: &Document, out: &mut Out) -> Result<(), String> {
    let found = atelier_form::document_actions(doc);
    if found.is_empty() {
        writeln!(out, "no actions").map_err(io)?;
    }
    for a in found {
        let owner = a.owner.map(|r| format!(" ({r})")).unwrap_or_default();
        writeln!(out, "{}{owner}: {}", a.location, a.kind).map_err(io)?;
        if let Some(t) = &a.target {
            writeln!(out, "    target: {t}").map_err(io)?;
        }
        if a.on_click {
            writeln!(out, "    runs on a click").map_err(io)?;
        }
        if let Some(js) = &a.javascript {
            for (t, name) in atelier_form::script_traits(js) {
                writeln!(out, "    note: {} ({name})", t.label()).map_err(io)?;
            }
            for line in js.lines() {
                writeln!(out, "    | {line}").map_err(io)?;
            }
        }
    }
    Ok(())
}

fn compat(doc: &Document, out: &mut Out) -> Result<(), String> {
    match atelier_form::xfa(doc) {
        None => writeln!(out, "XFA: none").map_err(io)?,
        Some(x) => {
            let names: Vec<&str> = x.packets.iter().map(|(n, _)| n.as_str()).collect();
            let kind = if x.dynamic { "dynamic" } else { "static" };
            writeln!(out, "XFA: {kind} (packets: {})", names.join(", ")).map_err(io)?;
            for r in &x.reasons {
                writeln!(out, "    {r}").map_err(io)?;
            }
            writeln!(out, "XFA scripts: {}", x.scripts.len()).map_err(io)?;
            for s in &x.scripts {
                writeln!(out, "    {} ({})", s.location, s.language).map_err(io)?;
                for (t, name) in atelier_form::script_traits(&s.source) {
                    writeln!(out, "        note: {} ({name})", t.label()).map_err(io)?;
                }
            }
        }
    }
    let actions = atelier_form::document_actions(doc);
    let scripts: Vec<_> = actions.iter().filter(|a| a.is_javascript()).collect();
    writeln!(out, "JavaScript actions: {}", scripts.len()).map_err(io)?;
    for a in scripts {
        let traits = atelier_form::script_traits(a.javascript.as_deref().unwrap_or(""));
        if traits.iter().any(|(t, _)| t.is_viewer_dependent()) {
            let labels: Vec<&str> = traits.iter().map(|(t, _)| t.label()).collect();
            writeln!(out, "    {}: {}", a.location, labels.join("; ")).map_err(io)?;
        }
    }
    let files = atelier_view::attachments(doc);
    writeln!(out, "Embedded files: {}", files.len()).map_err(io)?;
    Ok(())
}

fn attachments(doc: &Document, args: &[String], out: &mut Out) -> Result<(), String> {
    let files = atelier_view::attachments(doc);
    if let Some(i) = args.iter().position(|a| a == "--save") {
        let n: usize = args
            .get(i + 1)
            .and_then(|n| n.parse().ok())
            .ok_or("--save needs a file number")?;
        let o = args
            .iter()
            .position(|a| a == "-o")
            .ok_or("--save needs -o <out>")?;
        let output = args.get(o + 1).ok_or("-o needs a value")?;
        let a = n
            .checked_sub(1)
            .and_then(|n| files.get(n))
            .ok_or(format!("there is no file {n}"))?;
        let data = atelier_view::attachment_data(doc, a).map_err(|e| e.to_string())?;
        std::fs::write(output, &data).map_err(|e| format!("cannot write {output}: {e}"))?;
        writeln!(out, "wrote {output} ({} bytes)", data.len()).map_err(io)?;
        return Ok(());
    }
    if files.is_empty() {
        writeln!(out, "no embedded files").map_err(io)?;
    }
    for (i, a) in files.iter().enumerate() {
        writeln!(out, "{}. {} ({})", i + 1, a.name, a.source.label()).map_err(io)?;
        if let Some(d) = &a.description {
            writeln!(out, "    description: {d}").map_err(io)?;
        }
        if let Some(m) = &a.mime {
            writeln!(out, "    declared type: {m}").map_err(io)?;
        }
        match atelier_view::attachment_data(doc, a) {
            Ok(data) => {
                let c = atelier_view::check_attachment(a, &data);
                writeln!(out, "    {} bytes, {}", c.size, c.kind).map_err(io)?;
                writeln!(out, "    sha256: {}", c.sha256).map_err(io)?;
                for w in &c.cautions {
                    writeln!(out, "    caution: {w}").map_err(io)?;
                }
            }
            Err(e) => writeln!(out, "    cannot read: {e}").map_err(io)?,
        }
    }
    Ok(())
}
