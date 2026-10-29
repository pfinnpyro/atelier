//! Sets a file's /Title as an incremental update: `retitle in.pdf out.pdf title`.

use atelier_syntax::{Document, Object, PdfString};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output, title] = &args[..] else {
        eprintln!("usage: retitle in.pdf out.pdf title");
        std::process::exit(2);
    };
    let doc = Document::open(std::fs::read(input).unwrap()).unwrap();
    let mut up = doc.update();
    let mut info = doc.info().unwrap_or_default();
    info.insert(
        "Title",
        Object::String(PdfString::literal(title.as_bytes())),
    );
    match doc.trailer().get(b"Info").and_then(Object::as_reference) {
        Some(r) => up.set(r, Object::Dict(info)),
        None => {
            // A new Info needs the trailer to point at it; the update copies
            // /Info from the old trailer, so this example only edits files
            // that have one.
            eprintln!("no /Info; nothing to do");
            std::process::exit(1);
        }
    }
    std::fs::write(output, up.to_bytes().unwrap()).unwrap();
}
