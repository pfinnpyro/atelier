//! `cargo run --release --example ocr -- <models dir> <file.pdf>`: reads
//! every page with OCR and prints which way up it is and its text.

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, models, file] = &args[..] else {
        eprintln!("usage: ocr <models dir> <file.pdf>");
        std::process::exit(2);
    };
    let t = Instant::now();
    let engine = atelier_ocr::Engine::load(models.as_ref()).expect("models");
    eprintln!("loaded in {:?}", t.elapsed());
    let bytes = std::fs::read(file).expect("read");
    let doc = atelier_syntax::Document::open(bytes).expect("open");
    for (i, page) in doc.pages().expect("pages").iter().enumerate() {
        let t = Instant::now();
        match atelier_ocr::read_page(&engine, &doc, page) {
            Ok(Some(r)) => println!(
                "page {}: rotate {} in {:?}\n{}",
                i + 1,
                r.rotate,
                t.elapsed(),
                r.text.plain_text()
            ),
            Ok(None) => println!("page {}: nothing read in {:?}", i + 1, t.elapsed()),
            Err(e) => println!("page {}: {e}", i + 1),
        }
    }
}
