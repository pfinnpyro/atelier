//! Times JPEG decoding: `cargo run --release --example jpegbench -- file.jpg`.

fn main() {
    let path = std::env::args().nth(1).expect("usage: jpegbench file.jpg");
    let data = std::fs::read(&path).expect("cannot read file");
    let start = std::time::Instant::now();
    let img = atelier_filters::decode_dct(&data, None).expect("decode failed");
    println!(
        "{}x{}x{} in {:.1} ms",
        img.width,
        img.height,
        img.components,
        start.elapsed().as_secs_f64() * 1000.0
    );
}
