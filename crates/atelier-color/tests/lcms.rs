//! ICC profiles converted to sRGB, compared with Little CMS. The profiles
//! and references come from tools/icc_samples.py.

use atelier_color::{Intent, Profile};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Converts every sample in NAME.ref and returns the worst difference from
/// Little CMS, in 8-bit levels.
fn worst(name: &str) -> u8 {
    let profile = Profile::parse(&read(&format!("{name}.icc")), Intent::RelativeColorimetric)
        .unwrap_or_else(|| panic!("{name} did not parse"));
    let n = profile.components();
    let reference = read(&format!("{name}.ref"));
    let mut worst = 0;
    for row in reference.chunks_exact(n + 3) {
        let input: Vec<f32> = row[..n].iter().map(|&v| f32::from(v) / 255.0).collect();
        let got = profile.to_srgb(&input).map(|v| (v * 255.0).round() as u8);
        for (g, &e) in got.iter().zip(&row[n..]) {
            worst = worst.max(g.abs_diff(e));
        }
    }
    worst
}

#[test]
fn matrix_and_trc_profiles() {
    assert!(worst("srgb") <= 1, "srgb");
    assert!(worst("rgb-matrix") <= 1, "rgb-matrix");
    assert!(worst("gray-table") <= 1, "gray-table");
}

#[test]
fn lut_profiles() {
    for name in ["rgb-lut8", "cmyk-lut16", "cmyk-atob", "rgb-atob-matrix"] {
        let w = worst(name);
        assert!(w <= 2, "{name}: off by {w}");
    }
}

#[test]
fn damaged_profiles_are_refused_not_panicked_on() {
    let good = read("cmyk-atob.icc");
    let mut seed = 0x9e37_79b9u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..2000 {
        let mut bad = good.clone();
        for _ in 0..1 + rand() % 4 {
            let i = rand() as usize % bad.len();
            bad[i] = rand() as u8;
        }
        if let Some(p) = Profile::parse(&bad, Intent::Perceptual) {
            let _ = p.to_srgb(&[0.3, 0.5, 0.1, 0.9]);
        }
    }
    for cut in [0, 100, 131, 500, good.len() / 2] {
        assert!(Profile::parse(&good[..cut], Intent::Perceptual).is_none());
    }
}
