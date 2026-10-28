//! A minimal PNG writer (8-bit RGBA), compressed with our own Flate encoder.

use atelier_filters::encode_flate;

pub fn encode_rgba(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, adaptive, no interlace
    chunk(&mut out, b"IHDR", &ihdr);

    // Each row uses the Up filter, which suits page renders with flat areas.
    let stride = width * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height);
    for y in 0..height {
        raw.push(2);
        let row = &rgba[y * stride..(y + 1) * stride];
        if y == 0 {
            raw.extend_from_slice(row);
        } else {
            let prev = &rgba[(y - 1) * stride..y * stride];
            raw.extend(row.iter().zip(prev).map(|(a, b)| a.wrapping_sub(*b)));
        }
    }
    chunk(&mut out, b"IDAT", &encode_flate(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    !data.iter().fold(!0u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn png_layout() {
        let png = encode_rgba(2, 2, &[255; 16]);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR"));
        assert!(png.ends_with(b"IEND\xAE\x42\x60\x82"));
    }
}
