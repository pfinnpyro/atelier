//! Predictor post-processing for FlateDecode and LZWDecode (ISO 32000-2, 7.4.4.4).

use crate::{FilterError, Result};

/// Values from a stream's /DecodeParms dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Predictor {
    /// 1 = none, 2 = TIFF, 10..=15 = PNG.
    pub predictor: u8,
    pub colors: u8,
    pub bits_per_component: u8,
    pub columns: u32,
}

impl Default for Predictor {
    fn default() -> Self {
        Self {
            predictor: 1,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

impl Predictor {
    fn bits_per_pixel(&self) -> usize {
        usize::from(self.colors) * usize::from(self.bits_per_component)
    }

    fn row_bytes(&self) -> usize {
        (self.bits_per_pixel() * self.columns as usize).div_ceil(8)
    }
}

/// Undoes the predictor described by `p`. Predictor 1 returns the data unchanged.
pub fn apply_predictor(data: Vec<u8>, p: &Predictor) -> Result<Vec<u8>> {
    if !matches!(p.bits_per_component, 1 | 2 | 4 | 8 | 16) {
        return Err(FilterError::new(
            "Predictor",
            format!("unsupported BitsPerComponent {}", p.bits_per_component),
            data,
        ));
    }
    if p.colors == 0 || p.columns == 0 {
        return Err(FilterError::new(
            "Predictor",
            "Colors and Columns must be positive",
            data,
        ));
    }
    match p.predictor {
        0 | 1 => Ok(data),
        2 => Ok(tiff(data, p)),
        10..=15 => png(&data, p),
        other => Err(FilterError::new(
            "Predictor",
            format!("unknown predictor {other}"),
            data,
        )),
    }
}

fn png(data: &[u8], p: &Predictor) -> Result<Vec<u8>> {
    let row = p.row_bytes();
    let bpp = p.bits_per_pixel().div_ceil(8).max(1);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    let mut cur = vec![0u8; row];

    for chunk in data.chunks(row + 1) {
        let filter = chunk[0];
        let src = &chunk[1..];
        // A short final row is decoded as far as it goes.
        let n = src.len();
        cur[..n].copy_from_slice(src);
        match filter {
            0 => {}
            1 => {
                for i in bpp..n {
                    cur[i] = cur[i].wrapping_add(cur[i - bpp]);
                }
            }
            2 => {
                for i in 0..n {
                    cur[i] = cur[i].wrapping_add(prev[i]);
                }
            }
            3 => {
                for i in 0..n {
                    let left = if i >= bpp { cur[i - bpp] } else { 0 };
                    let avg = ((u16::from(left) + u16::from(prev[i])) / 2) as u8;
                    cur[i] = cur[i].wrapping_add(avg);
                }
            }
            4 => {
                for i in 0..n {
                    let (left, up_left) = if i >= bpp {
                        (cur[i - bpp], prev[i - bpp])
                    } else {
                        (0, 0)
                    };
                    cur[i] = cur[i].wrapping_add(paeth(left, prev[i], up_left));
                }
            }
            other => {
                return Err(FilterError::new(
                    "Predictor",
                    format!("unknown PNG row filter {other}"),
                    out,
                ));
            }
        }
        out.extend_from_slice(&cur[..n]);
        std::mem::swap(&mut prev, &mut cur);
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let pa = (p - i16::from(a)).abs();
    let pb = (p - i16::from(b)).abs();
    let pc = (p - i16::from(c)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn tiff(mut data: Vec<u8>, p: &Predictor) -> Vec<u8> {
    let row = p.row_bytes();
    let colors = usize::from(p.colors);
    let bpc = p.bits_per_component;
    for line in data.chunks_mut(row) {
        match bpc {
            8 => {
                for i in colors..line.len() {
                    line[i] = line[i].wrapping_add(line[i - colors]);
                }
            }
            16 => {
                let stride = colors * 2;
                let mut i = stride;
                while i + 1 < line.len() {
                    let prev = u16::from_be_bytes([line[i - stride], line[i - stride + 1]]);
                    let cur = u16::from_be_bytes([line[i], line[i + 1]]);
                    line[i..i + 2].copy_from_slice(&cur.wrapping_add(prev).to_be_bytes());
                    i += 2;
                }
            }
            _ => tiff_sub_byte(line, colors, bpc, p.columns as usize),
        }
    }
    data
}

fn tiff_sub_byte(line: &mut [u8], colors: usize, bpc: u8, columns: usize) {
    let bpc = usize::from(bpc);
    let mask = (1u16 << bpc) - 1;
    let samples = (columns * colors).min(line.len() * 8 / bpc);
    let get = |line: &[u8], i: usize| -> u16 {
        let bit = i * bpc;
        let shift = 8 - bpc - bit % 8;
        (u16::from(line[bit / 8]) >> shift) & mask
    };
    for i in colors..samples {
        let v = (get(line, i) + get(line, i - colors)) & mask;
        let bit = i * bpc;
        let shift = 8 - bpc - bit % 8;
        let byte = &mut line[bit / 8];
        *byte = (*byte & !((mask as u8) << shift)) | ((v as u8) << shift);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(predictor: u8, colors: u8, bpc: u8, columns: u32) -> Predictor {
        Predictor {
            predictor,
            colors,
            bits_per_component: bpc,
            columns,
        }
    }

    #[test]
    fn png_up_filter_typical_xref_stream() {
        // Xref streams commonly use Predictor 12 with Columns = row width.
        let data = vec![2, 1, 0, 16, 2, 0, 1, 0];
        let out = apply_predictor(data, &params(12, 1, 8, 3)).unwrap();
        assert_eq!(out, vec![1, 0, 16, 1, 1, 16]);
    }

    #[test]
    fn png_all_filters() {
        // Two-pixel RGB rows, one row per filter type.
        let rows: [[u8; 6]; 5] = [
            [10, 20, 30, 40, 50, 60],
            [1, 2, 3, 4, 5, 6],
            [100, 100, 100, 7, 8, 9],
            [5, 5, 5, 200, 201, 202],
            [9, 8, 7, 6, 5, 4],
        ];
        let mut encoded = Vec::new();
        let mut prev = [0u8; 6];
        for (filter, row) in rows.iter().enumerate() {
            encoded.push(filter as u8);
            for i in 0..6 {
                let left = if i >= 3 { row[i - 3] } else { 0 };
                let up_left = if i >= 3 { prev[i - 3] } else { 0 };
                let pred = match filter {
                    0 => 0,
                    1 => left,
                    2 => prev[i],
                    3 => ((u16::from(left) + u16::from(prev[i])) / 2) as u8,
                    _ => paeth(left, prev[i], up_left),
                };
                encoded.push(row[i].wrapping_sub(pred));
            }
            prev = *row;
        }
        let out = apply_predictor(encoded, &params(15, 3, 8, 2)).unwrap();
        assert_eq!(out, rows.concat());
    }

    #[test]
    fn tiff_8_bit() {
        let out = apply_predictor(vec![10, 1, 1, 1], &params(2, 1, 8, 4)).unwrap();
        assert_eq!(out, vec![10, 11, 12, 13]);
    }

    #[test]
    fn tiff_4_bit() {
        // Samples 1, +1, +1, +1 packed as nibbles.
        let out = apply_predictor(vec![0x11, 0x11], &params(2, 1, 4, 4)).unwrap();
        assert_eq!(out, vec![0x12, 0x34]);
    }

    #[test]
    fn short_final_png_row_is_kept() {
        let out = apply_predictor(vec![0, 1, 2, 3, 0, 4], &params(12, 1, 8, 3)).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4]);
    }
}
