//! Compares glyph outlines with values fontTools computed for the same fonts
//! (tests/data/*.expected, written by tools/fontcheck.py --expect).

use atelier_font::{Cff, FontProgram, TrueType, Type1};

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/");

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{DATA}{name}")).unwrap()
}

/// Area and bounds of a glyph's outline, from a fine flattening.
fn measure(font: &FontProgram, gid: u32) -> Option<(f64, [f64; 4])> {
    let path = font.outline(gid)?;
    let polys = path.flatten(0.00002);
    if polys.is_empty() {
        return None;
    }
    let mut area = 0.0;
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for (poly, _) in &polys {
        for (i, p) in poly.iter().enumerate() {
            let q = poly[(i + 1) % poly.len()];
            area += p.x * q.y - q.x * p.y;
            b = [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)];
        }
    }
    Some((area / 2.0, b))
}

fn check(font: &FontProgram, expected: &str, gid_for: impl Fn(&str) -> u32) {
    let text = std::fs::read_to_string(format!("{DATA}{expected}")).unwrap();
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let gid = gid_for(f[0]);
        let adv: f64 = f[1].parse().unwrap();
        let got_adv = font.advance(gid).unwrap();
        assert!((got_adv - adv).abs() < 1e-3, "{expected} {}: advance {got_adv} vs {adv}", f[0]);
        let got = measure(font, gid);
        if f[2] == "empty" {
            assert!(got.is_none_or(|(a, _)| a.abs() < 1e-6), "{expected} {}: not empty", f[0]);
            continue;
        }
        let area: f64 = f[2].parse().unwrap();
        let bounds: Vec<f64> = f[3..7].iter().map(|v| v.parse().unwrap()).collect();
        let (a, b) = got.unwrap_or_else(|| panic!("{expected} {}: no outline", f[0]));
        assert!(
            (a.abs() - area.abs()).abs() < 1e-4 + 1e-3 * area.abs(),
            "{expected} {}: area {a} vs {area}",
            f[0]
        );
        for k in 0..4 {
            assert!((b[k] - bounds[k]).abs() < 2e-3, "{expected} {}: bounds {b:?} vs {bounds:?}", f[0]);
        }
        checked += 1;
    }
    assert!(checked > 5, "{expected}: only {checked} glyphs checked");
}

fn by_gid(label: &str) -> u32 {
    label.parse().unwrap()
}

#[test]
fn truetype_outlines_match_fonttools() {
    let font = FontProgram::parse(&read("liberation.ttf")).unwrap();
    assert!(matches!(font, FontProgram::TrueType(_)));
    check(&font, "liberation.expected", by_gid);
}

#[test]
fn opentype_cff_outlines_match_fonttools() {
    let font = FontProgram::parse(&read("inter.otf")).unwrap();
    assert!(matches!(font, FontProgram::Cff(_)));
    check(&font, "inter.expected", by_gid);
}

#[test]
fn bare_cff_outlines_match_fonttools() {
    let font = FontProgram::parse(&read("inter.cff")).unwrap();
    check(&font, "inter.expected", by_gid);
}

#[test]
fn cid_keyed_cff_outlines_match_fonttools() {
    let font = FontProgram::parse(&read("unifont_cid.otf")).unwrap();
    let FontProgram::Cff(cff) = &font else {
        panic!("not CFF")
    };
    assert!(cff.is_cid());
    check(&font, "unifont_cid.expected", by_gid);
}

#[test]
fn type1_pfb_outlines_match_fonttools() {
    let data = read("charter.pfb");
    let t1 = Type1::parse(&data).unwrap();
    let names: Vec<String> = (0..t1.glyph_count() as u32)
        .map(|g| t1.glyph_name(g).unwrap().to_string())
        .collect();
    let font = FontProgram::Type1(t1);
    check(&font, "charter.expected", |n| {
        names.iter().position(|x| x == n).unwrap() as u32
    });
}

/// The same font as PDF embeds it: clear text, then hex or binary eexec.
#[test]
fn type1_pfa_and_raw_forms_parse() {
    let pfb = read("charter.pfb");
    let mut segments = Vec::new();
    let mut pos = 0;
    while pos + 6 <= pfb.len() && pfb[pos] == 0x80 && pfb[pos + 1] != 3 {
        let len = u32::from_le_bytes(pfb[pos + 2..pos + 6].try_into().unwrap()) as usize;
        segments.push((pfb[pos + 1], pfb[pos + 6..pos + 6 + len].to_vec()));
        pos += 6 + len;
    }
    let clear = &segments[0].1;
    let binary = &segments[1].1;
    let mut raw = clear.clone();
    raw.extend_from_slice(binary);
    let mut pfa = clear.clone();
    for chunk in binary.chunks(32) {
        for b in chunk {
            pfa.extend_from_slice(format!("{b:02x}").as_bytes());
        }
        pfa.push(b'\n');
    }
    let reference = Type1::parse(&pfb).unwrap();
    for data in [raw, pfa] {
        let t = Type1::parse(&data).unwrap();
        assert_eq!(t.glyph_count(), reference.glyph_count());
        let a = t.glyph_by_name("a").unwrap();
        assert_eq!(t.outline(a), reference.outline(a));
    }
}

#[test]
fn character_maps() {
    let tt = TrueType::parse(&read("liberation.ttf")).unwrap();
    let a = tt.glyph_by_unicode('A').unwrap();
    assert_eq!(tt.glyph_by_name("A"), Some(a));
    assert!(tt.glyph_by_unicode('€').is_some());
    assert_eq!(tt.glyph_by_unicode('Z'), None);

    let cff = FontProgram::parse(&read("inter.cff")).unwrap();
    assert!(cff.glyph_by_name("A").is_some());
    // This subset's custom encoding is empty (fontTools agrees).
    assert_eq!(cff.builtin_encoding(b'A'), None);

    let cid = FontProgram::parse(&read("unifont_cid.otf")).unwrap();
    let FontProgram::Cff(c) = &cid else { panic!() };
    // The subset keeps CIDs: U+6F22 is CID 28451 (0x6F23) in unifont.
    let g = cid.glyph_by_unicode('漢').unwrap();
    assert_eq!(c.glyph_by_cid(28451), Some(g));
    assert_eq!(c.glyph_by_cid(28450), None);

    let t1 = Type1::parse(&read("charter.pfb")).unwrap();
    assert_eq!(t1.builtin_encoding(b'A'), Some("A"));
}

#[test]
fn damaged_fonts_do_not_panic() {
    for name in ["liberation.ttf", "inter.otf", "inter.cff", "unifont_cid.otf", "charter.pfb"] {
        let data = read(name);
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for round in 0..200 {
            let mut d = data.clone();
            if round % 4 == 0 {
                d.truncate(d.len() * (round + 1) / 201);
            }
            for _ in 0..8 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let i = (seed as usize) % d.len().max(1);
                if let Some(b) = d.get_mut(i) {
                    *b = (seed >> 32) as u8;
                }
            }
            if let Ok(f) = FontProgram::parse(&d) {
                for gid in 0..f.glyph_count().min(64) as u32 {
                    let _ = f.outline(gid);
                    let _ = f.advance(gid);
                }
            }
        }
    }
    let _ = Cff::parse(&[]);
}
