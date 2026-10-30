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
  atelier text <file.pdf> [page|all] [--glyphs]
                                           print the text of pages (default all); with
                                           --glyphs, one line per glyph with its box
  atelier fields <file.pdf>                list form fields, their values and widgets
  atelier fill <file.pdf> -o <out.pdf> [name=value ...] [--refresh]
                                           set field values and save as an incremental
                                           update. Checkboxes take on, off or a state
                                           name; multi-select lists take a|b|c.
                                           --refresh redraws every field's appearance
  atelier actions <file.pdf>               list scripts and other actions (never run)
  atelier compat <file.pdf>                XFA (static or dynamic), scripts that depend
                                           on the viewer, and embedded files
  atelier attachments <file.pdf> [--save N -o out]
                                           list embedded files with their type and
                                           SHA-256; --save writes file N (from 1) out
  atelier orient <file.pdf>                which way up each page reads, judged from
                                           its text, or for scans from the image

Every command takes --password <pw> for encrypted files.";

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
    let args = &args[..];
    let (Some(command), Some(path)) = (args.first(), args.get(1)) else {
        return Err(format!("missing arguments\n\n{USAGE}"));
    };
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let doc = match Document::open_with_password(bytes, password.as_bytes()) {
        Err(Error::Password) if password.is_empty() => {
            return Err("this PDF is protected by a password; pass --password".into());
        }
        Err(Error::Password) => return Err("wrong password".into()),
        other => other.map_err(|e| e.to_string())?,
    };
    let mut out = std::io::stdout().lock();

    let result = match command.as_str() {
        "info" => info(&doc, &mut out),
        "pages" => pages(&doc, &mut out),
        "objects" => objects(&doc, &mut out),
        "obj" => object(&doc, obj_ref(&args[2..])?, &mut out),
        "stream" => stream(&doc, obj_ref(&args[2..])?, &mut out),
        "content" => content(&doc, &args[2..], &mut out),
        "render" => render(&doc, path, &args[2..], &mut out),
        "text" => text(&doc, &args[2..], &mut out),
        "fields" => fields(&doc, &mut out),
        "fill" => fill(&doc, &args[2..], &mut out),
        "actions" => actions(&doc, &mut out),
        "compat" => compat(&doc, &mut out),
        "attachments" => attachments(&doc, &args[2..], &mut out),
        "orient" => orient(&doc, &mut out),
        other => return Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    for warning in doc.warnings() {
        eprintln!("warning: {warning}");
    }
    result
}

type Out<'a> = std::io::StdoutLock<'a>;

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
