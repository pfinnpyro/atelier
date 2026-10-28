//! CCITT fax data written by libtiff (through Pillow) from one 400 x 120
//! bitmap, decoded and compared with the bitmap.

use atelier_filters::{CcittParams, decode_ccitt};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn check(file: &str, k: i64) {
    let params = CcittParams {
        k,
        columns: 400,
        rows: 120,
        ..CcittParams::default()
    };
    let decoded = decode_ccitt(&data(file), &params).unwrap();
    // The TIFFs say 1 is white, so their "white" runs are our black ones.
    let expected: Vec<u8> = data("fax.raw").iter().map(|b| !b).collect();
    assert_eq!(decoded.len(), expected.len(), "{file}");
    let first_bad = decoded.iter().zip(&expected).position(|(a, b)| a != b);
    assert_eq!(
        first_bad,
        None,
        "{file}: first wrong row {:?}",
        first_bad.map(|i| i / 50)
    );
}

#[test]
fn group_4() {
    check("fax-g4.bin", -1);
}

#[test]
fn group_3_one_dimensional_with_fill() {
    check("fax-g3-1d.bin", 0);
}

#[test]
fn group_3_two_dimensional() {
    check("fax-g3-2d.bin", 1);
}

#[test]
fn black_is_1_inverts() {
    let params = CcittParams {
        k: -1,
        columns: 400,
        rows: 120,
        black_is_1: true,
        ..CcittParams::default()
    };
    assert_eq!(
        decode_ccitt(&data("fax-g4.bin"), &params).unwrap(),
        data("fax.raw")
    );
}

#[test]
fn damaged_data_keeps_the_rows_before_the_damage() {
    let mut bytes = data("fax-g4.bin");
    let half = bytes.len() / 2;
    bytes[half..].fill(0);
    let params = CcittParams {
        k: -1,
        columns: 400,
        rows: 120,
        ..CcittParams::default()
    };
    let err = decode_ccitt(&bytes, &params).unwrap_err();
    // Padded to the full height, with the top rows right.
    assert_eq!(err.partial.len(), 6000);
    let expected: Vec<u8> = data("fax.raw").iter().map(|b| !b).collect();
    assert_eq!(err.partial[..1000], expected[..1000]);
}

#[test]
fn garbage_never_panics() {
    let mut state = 0x1234_5678u32;
    for len in [0, 1, 7, 100, 3000] {
        for k in [-1, 0, 1, 4] {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            for rows in [0, 50] {
                let params = CcittParams {
                    k,
                    columns: 97,
                    rows,
                    encoded_byte_align: len % 2 == 0,
                    ..CcittParams::default()
                };
                let _ = decode_ccitt(&bytes, &params);
            }
        }
    }
}
