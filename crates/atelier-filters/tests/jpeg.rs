//! Compares our JPEG decoder with libjpeg (through Pillow) on fixtures in
//! tests/data. Each `.raw` file holds Pillow's decoded samples.

use atelier_filters::decode_dct;

fn compare(name: &str, max_mean: f64, max_diff: u8) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let jpg = std::fs::read(format!("{dir}{name}.jpg")).unwrap();
    let expected = std::fs::read(format!("{dir}{name}.raw")).unwrap();
    let img = decode_dct(&jpg, None).unwrap();
    assert_eq!((img.width, img.height), (37, 23), "{name}");
    let mut ours = img.data;
    if img.components == 4 {
        // Adobe CMYK JPEGs store inverted values; Pillow un-inverts them, we
        // keep the stored values as PDF expects.
        ours.iter_mut().for_each(|v| *v = 255 - *v);
    }
    assert_eq!(ours.len(), expected.len(), "{name}");
    let diffs: Vec<u8> = ours
        .iter()
        .zip(&expected)
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let mean = diffs.iter().map(|&d| f64::from(d)).sum::<f64>() / diffs.len() as f64;
    let max = *diffs.iter().max().unwrap();
    println!("{name}: mean {mean:.3}, max {max}");
    assert!(
        mean <= max_mean && max <= max_diff,
        "{name}: mean {mean:.3}, max {max}"
    );
}

#[test]
fn gray_baseline() {
    compare("gray_baseline", 0.3, 4);
}

#[test]
fn gray_progressive() {
    compare("gray_progressive", 0.3, 4);
}

#[test]
fn rgb_444_baseline() {
    compare("rgb444_baseline", 0.3, 4);
}

#[test]
fn rgb_444_progressive() {
    compare("rgb444_progressive", 0.3, 4);
}

#[test]
fn rgb_420_baseline() {
    compare("rgb420_baseline", 0.3, 4);
}

#[test]
fn rgb_422_baseline() {
    compare("rgb422_baseline", 0.3, 4);
}

#[test]
fn rgb_420_progressive() {
    compare("rgb420_progressive", 0.3, 4);
}

#[test]
fn restart_markers() {
    compare("rgb420_restart", 0.3, 4);
    compare("rgb420_progressive_restart", 0.3, 4);
}

#[test]
fn cmyk_adobe() {
    compare("cmyk_baseline", 0.3, 4);
}

#[test]
fn truncated_and_garbage_input_never_panics() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");
    let jpg = std::fs::read(format!("{dir}rgb420_progressive.jpg")).unwrap();
    for cut in (0..jpg.len()).step_by(7) {
        let _ = decode_dct(&jpg[..cut], None);
    }
    let mut state = 99u32;
    for _ in 0..500 {
        let mut data = jpg.clone();
        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let i = state as usize % data.len();
            data[i] = (state >> 8) as u8;
        }
        let _ = decode_dct(&data, None);
    }
}
