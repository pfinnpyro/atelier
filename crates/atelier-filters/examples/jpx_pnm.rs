//! Decodes a JPEG 2000 file to PNM (PGM or PPM, plus a PGM of any alpha),
//! for comparing with other decoders.
//!
//! Usage: cargo run --example jpx_pnm -- in.jp2 out.pnm

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("read input");
    let start = std::time::Instant::now();
    let img = atelier_filters::decode_jpx(&data).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    eprintln!(
        "{}x{} {} channel(s) {:?} in {:.1} ms",
        img.width,
        img.height,
        img.components,
        img.color,
        start.elapsed().as_secs_f64() * 1000.0
    );
    for w in &img.warnings {
        eprintln!("warning: {w}");
    }
    let (magic, n) = if img.components >= 3 {
        ("P6", 3)
    } else {
        ("P5", 1)
    };
    let mut out = format!("{magic}\n{} {}\n255\n", img.width, img.height).into_bytes();
    for px in img.data.chunks_exact(img.components) {
        out.extend_from_slice(&px[..n]);
    }
    std::fs::write(&args[2], out).expect("write output");
    if let Some(alpha) = img.alpha {
        let mut a = format!("P5\n{} {}\n255\n", img.width, img.height).into_bytes();
        a.extend_from_slice(&alpha);
        std::fs::write(format!("{}.alpha.pgm", args[2]), a).expect("write alpha");
    }
}
