//! `atelier`: inspect PDF files with the Atelier engine.

use std::io::Write;
use std::process::ExitCode;

use atelier_syntax::{Document, ObjRef, Object, XrefEntry};

const USAGE: &str = "\
Usage:
  atelier info <file.pdf>                  version, structure and page count
  atelier pages <file.pdf>                 one line per page: boxes and rotation
  atelier objects <file.pdf>               every object number and where it lives
  atelier obj <file.pdf> <num> [gen]       print one object
  atelier stream <file.pdf> <num> [gen]    write a stream's decoded bytes to stdout
  atelier content <file.pdf> <page>        write a page's decoded content (page from 1)";

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
    let (Some(command), Some(path)) = (args.first(), args.get(1)) else {
        return Err(format!("missing arguments\n\n{USAGE}"));
    };
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let doc = Document::open(bytes).map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();

    let result = match command.as_str() {
        "info" => info(&doc, &mut out),
        "pages" => pages(&doc, &mut out),
        "objects" => objects(&doc, &mut out),
        "obj" => object(&doc, obj_ref(&args[2..])?, &mut out),
        "stream" => stream(&doc, obj_ref(&args[2..])?, &mut out),
        "content" => content(&doc, &args[2..], &mut out),
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
    writeln!(
        out,
        "Encrypted:    {}",
        if doc.is_encrypted() { "yes" } else { "no" }
    )
    .map_err(io)?;
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
