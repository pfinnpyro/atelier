//! JPEG 2000 streams compared with what OpenJPEG makes of them. The
//! fixtures come from tools/jpx_samples.py: OpenJPEG's encoder with most of
//! its options, plus palettes, sYCC and packed packet headers assembled by
//! the script.

use atelier_filters::{JpxColor, decode_jpx};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/data/jpx/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The samples of a binary PNM and its channel count.
fn pnm(name: &str) -> (Vec<u8>, usize) {
    let data = read(name);
    let channels = if data.starts_with(b"P6") { 3 } else { 1 };
    let mut newlines = 0;
    let start = data
        .iter()
        .position(|&b| {
            newlines += usize::from(b == b'\n');
            newlines == 3
        })
        .unwrap()
        + 1;
    (data[start..].to_vec(), channels)
}

/// Decodes `name` and checks it against OpenJPEG's output, allowing
/// `tolerance` levels of difference (rounding in the irreversible path).
fn check(name: &str, tolerance: u8) -> atelier_filters::DecodedJpx {
    let file = ["j2k", "jp2"]
        .iter()
        .map(|e| format!("{name}.{e}"))
        .find(|f| {
            std::path::Path::new(&format!(
                "{}/tests/data/jpx/{f}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .exists()
        })
        .unwrap();
    let img = decode_jpx(&read(&file)).unwrap();
    assert!(img.warnings.is_empty(), "{name}: {:?}", img.warnings);
    let (expected, channels) = pnm(&format!("{name}.pnm"));
    assert_eq!((img.width, img.height), (67, 45), "{name}");
    let got: Vec<u8> = img
        .data
        .chunks_exact(img.components)
        .flat_map(|px| px[..channels].to_vec())
        .collect();
    assert_eq!(got.len(), expected.len(), "{name}");
    let worst = got
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(worst <= tolerance, "{name}: off by up to {worst}");
    img
}

#[test]
fn reversible_and_layers() {
    assert_eq!(check("gray", 0).color, JpxColor::Unknown);
    check("rgb-layers", 0);
    check("tile-offsets", 0);
    check("subsampled", 0);
}

#[test]
fn irreversible() {
    check("rgb-97", 1);
    check("modes-97", 1);
}

#[test]
fn progression_orders() {
    for order in ["lrcp", "rlcp", "rpcl", "pcrl", "cprl"] {
        check(&format!("order-{order}"), 0);
    }
    check("poc", 0);
}

#[test]
fn code_block_modes_markers_and_roi() {
    check("modes", 0);
    check("sop-eph", 0);
    check("packed-ppm", 0);
    check("packed-ppt", 0);
    check("roi", 0);
}

#[test]
fn sample_depths() {
    check("gray12", 0);
    check("signed", 0);
}

#[test]
fn jp2_colour_alpha_and_palette() {
    let img = check("rgba", 0);
    assert_eq!(img.color, JpxColor::Rgb);
    let (alpha, _) = pnm("rgba.alpha.pgm");
    assert_eq!(img.alpha.unwrap(), alpha);
    assert_eq!(check("palette", 0).color, JpxColor::Rgb);
    // OpenJPEG upsamples chroma differently at odd pixels; compare it all
    // within rounding of the colour conversion.
    check("sycc", 1);
}

#[test]
fn damaged_streams_never_panic() {
    let mut seed = 0x1234_5678u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for name in [
        "rgb-layers.j2k",
        "order-pcrl.j2k",
        "modes.j2k",
        "rgba.jp2",
        "packed-ppt.j2k",
    ] {
        let good = read(name);
        // Truncations decode what is there.
        for cut in [good.len() / 3, good.len() / 2, good.len() - 10] {
            let _ = decode_jpx(&good[..cut]);
        }
        for _ in 0..120 {
            let mut bad = good.clone();
            for _ in 0..1 + rand() % 6 {
                let i = rand() as usize % bad.len();
                bad[i] = rand() as u8;
            }
            let _ = decode_jpx(&bad);
        }
    }
    // A truncated stream keeps the part decoded.
    let good = read("rgb-layers.j2k");
    let img = decode_jpx(&good[..good.len() * 2 / 3]).unwrap();
    assert!(!img.warnings.is_empty());
}
