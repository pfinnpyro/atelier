//! JBIG2 streams in PDF's embedded form, compared with what jbig2dec (the
//! reference decoder from Ghostscript) makes of them.
//!
//! generic, generic-tpgd and symbol(-globals) come from jbig2enc; mmr wraps
//! libtiff's Group 4 coding; huffman and halftone come from the small
//! encoders in tools/jbig2_*_sample.py.

use atelier_filters::decode_jbig2;

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/data/jbig2/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The pixels of a binary PBM, 1 = black.
fn pbm(name: &str) -> Vec<u8> {
    let data = read(name);
    let mut newlines = 0;
    let start = data
        .iter()
        .position(|&b| {
            newlines += usize::from(b == b'\n');
            newlines == 2
        })
        .unwrap()
        + 1;
    data[start..].to_vec()
}

fn check(stream: &str, globals: Option<&str>, expected: &str) {
    let globals = globals.map(read);
    let (decoded, warnings) = decode_jbig2(&read(stream), globals.as_deref()).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    // PDF wants 0 for black; PBM has 1.
    let decoded: Vec<u8> = decoded.iter().map(|b| !b).collect();
    let expected = pbm(expected);
    assert_eq!(decoded.len(), expected.len(), "{stream}");
    let first_bad = decoded.iter().zip(&expected).position(|(a, b)| a != b);
    assert_eq!(first_bad, None, "{stream}");
}

#[test]
fn generic_region() {
    check("generic.jb2", None, "generic.pbm");
}

#[test]
fn generic_region_with_typical_prediction() {
    check("generic-tpgd.jb2", None, "generic-tpgd.pbm");
}

#[test]
fn mmr_generic_region() {
    check("mmr.jb2", None, "mmr.pbm");
}

#[test]
fn symbol_dictionary_in_globals_and_text_region() {
    check("symbol.jb2", Some("symbol-globals.jb2"), "symbol.pbm");
}

#[test]
fn huffman_symbol_dictionary_and_text_region() {
    check("huffman.jb2", None, "huffman.pbm");
}

#[test]
fn halftone_region() {
    check("halftone.jb2", None, "halftone.pbm");
}

#[test]
fn damaged_streams_never_panic() {
    let mut state = 0x9e37_79b9u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for name in [
        "generic.jb2",
        "mmr.jb2",
        "huffman.jb2",
        "halftone.jb2",
        "symbol.jb2",
    ] {
        let original = read(name);
        let globals = (name == "symbol.jb2").then(|| read("symbol-globals.jb2"));
        for _ in 0..300 {
            let mut data = original.clone();
            match next() % 3 {
                0 => {
                    let n = next() as usize % data.len();
                    data.truncate(n);
                }
                1 => {
                    for _ in 0..1 + next() % 4 {
                        let i = next() as usize % data.len();
                        data[i] = next() as u8;
                    }
                }
                _ => {
                    let i = next() as usize % data.len();
                    data[i] ^= 1 << (next() % 8);
                }
            }
            let _ = decode_jbig2(&data, globals.as_deref());
        }
    }
}
