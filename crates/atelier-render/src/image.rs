//! Image XObjects and inline images (ISO 32000-2, 8.9) to RGBA.

use atelier_filters::decode_dct;
use atelier_raster::{Color, RgbaImage};
use atelier_syntax::{Dict, Document, ImageFilter, Object, Stream};

use crate::colorspace::{self, ColorSpace};

/// Refuse images larger than this many pixels (hostile files).
const MAX_PIXELS: usize = 200_000_000;

/// Looks up a key that inline images may abbreviate.
fn get<'a>(dict: &'a Dict, long: &[u8], short: &[u8]) -> Option<&'a Object> {
    dict.get(long).or_else(|| dict.get(short))
}

pub struct ImageInfo {
    pub image: RgbaImage,
    pub interpolate: bool,
}

/// 8-bit samples from an image codec. Their size and component count
/// override what the image dictionary says.
struct CodecOutput {
    data: Vec<u8>,
    width: usize,
    height: usize,
    components: usize,
}

/// Runs the image codec named by `filter`, if any.
fn run_codec(
    data: &[u8],
    filter: Option<&(ImageFilter, Dict)>,
) -> Result<Option<CodecOutput>, String> {
    match filter {
        None => Ok(None),
        Some((ImageFilter::Dct, params)) => {
            let transform = params
                .get(b"ColorTransform")
                .and_then(Object::as_i64)
                .map(|v| v != 0);
            let j = decode_dct(data, transform).map_err(|e| format!("JPEG image: {e}"))?;
            Ok(Some(CodecOutput {
                width: j.width,
                height: j.height,
                components: j.components,
                data: j.data,
            }))
        }
        Some((f, _)) => Err(format!("{f:?} images are not decoded yet")),
    }
}

/// Decodes an image. `data` is the stream's data with its non-image filters
/// already removed; `image_filter` is a remaining image codec and its
/// parameters, if any.
pub fn decode(
    doc: &Document,
    dict: &Dict,
    data: &[u8],
    image_filter: Option<&(ImageFilter, Dict)>,
    resources: &Dict,
    fill: Color,
    warn: &mut dyn FnMut(String),
) -> Option<ImageInfo> {
    let codec = match run_codec(data, image_filter) {
        Ok(c) => c,
        Err(e) => {
            warn(e);
            return None;
        }
    };
    let data = codec.as_ref().map_or(data, |c| c.data.as_slice());
    let resolve = |o: Option<&Object>| o.and_then(|o| doc.resolve(o).ok());
    let (width, height) = match &codec {
        Some(c) => (c.width, c.height),
        None => (
            usize::try_from(resolve(get(dict, b"Width", b"W"))?.as_i64()?).ok()?,
            usize::try_from(resolve(get(dict, b"Height", b"H"))?.as_i64()?).ok()?,
        ),
    };
    if width == 0 || height == 0 || width.saturating_mul(height) > MAX_PIXELS {
        return None;
    }
    let is_mask = resolve(get(dict, b"ImageMask", b"IM"))
        .and_then(|o| o.as_bool())
        .unwrap_or(false);
    let interpolate = resolve(get(dict, b"Interpolate", b"I"))
        .and_then(|o| o.as_bool())
        .unwrap_or(false);
    let decode_array: Option<Vec<f32>> = resolve(get(dict, b"Decode", b"D")).and_then(|d| {
        d.as_array()
            .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
    });

    let image = if is_mask {
        // Stencil mask: sample 0 paints the fill colour, unless Decode is [1 0].
        let invert = decode_array
            .as_ref()
            .is_some_and(|d| d.first() == Some(&1.0));
        let bits = unpack(data, width, height, 1, 1);
        let px = fill.to_premultiplied();
        let mut out = vec![0u8; width * height * 4];
        for (i, &v) in bits.iter().enumerate() {
            if (v == 0) != invert {
                out[i * 4..i * 4 + 4].copy_from_slice(&px);
            }
        }
        RgbaImage {
            width,
            height,
            data: out,
        }
    } else {
        let bpc = match &codec {
            Some(_) => 8,
            None => resolve(get(dict, b"BitsPerComponent", b"BPC"))
                .and_then(|o| o.as_i64())
                .unwrap_or(8),
        };
        let bpc = u32::try_from(bpc)
            .ok()
            .filter(|b| matches!(b, 1 | 2 | 4 | 8 | 16))?;
        let declared =
            get(dict, b"ColorSpace", b"CS").and_then(|o| colorspace::parse(doc, o, resources));
        let cs = match (&codec, declared) {
            // The codec's component count wins over a missing or
            // mismatched colour space.
            (Some(c), Some(cs)) if cs.components() == c.components => cs,
            (Some(c), _) => match c.components {
                1 => ColorSpace::Gray,
                4 => ColorSpace::Cmyk,
                _ => ColorSpace::Rgb,
            },
            (None, cs) => cs?,
        };
        if matches!(cs, ColorSpace::Pattern { .. }) {
            return None;
        }
        let n = cs.components();
        let samples = unpack(data, width, height, n, bpc);
        let max = ((1u32 << bpc) - 1) as f32;
        let ranges: Vec<(f32, f32)> = (0..n)
            .map(|i| match &decode_array {
                Some(d) if d.len() >= 2 * n => (d[2 * i], d[2 * i + 1]),
                _ => cs.decode_range(bpc),
            })
            .collect();

        let color_key: Option<Vec<(u32, u32)>> = resolve(dict.get(b"Mask")).and_then(|m| {
            m.as_array().map(|a| {
                a.chunks(2)
                    .map(|p| {
                        let v = |i: usize| p.get(i).and_then(Object::as_i64).unwrap_or(0) as u32;
                        (v(0), v(1))
                    })
                    .collect()
            })
        });

        let mut out = vec![0u8; width * height * 4];
        let mut comps = vec![0f32; n];
        // One-component images (gray, Indexed, Separation) convert every
        // possible sample once, so tint transforms run at most 2^bpc times.
        let table: Option<Vec<[u8; 4]>> = (n == 1).then(|| {
            (0..=max as u32)
                .map(|v| {
                    let (lo, hi) = ranges[0];
                    cs.to_rgb(&[lo + v as f32 * (hi - lo) / max])
                        .map_or([0; 4], Color::to_premultiplied)
                })
                .collect()
        });
        let mut cache: Option<(Vec<u32>, [u8; 4])> = None;
        for p in 0..width * height {
            let raw = &samples[p * n..(p + 1) * n];
            if let Some(key) = &color_key {
                if key.len() == n
                    && raw
                        .iter()
                        .zip(key)
                        .all(|(&v, &(lo, hi))| v >= lo && v <= hi)
                {
                    continue;
                }
            }
            let px = match (&table, &cache) {
                (Some(t), _) => t[(raw[0] as usize).min(t.len() - 1)],
                (None, Some((k, px))) if k.as_slice() == raw => *px,
                _ => {
                    for (i, &v) in raw.iter().enumerate() {
                        let (lo, hi) = ranges[i];
                        comps[i] = lo + v as f32 * (hi - lo) / max;
                    }
                    let px = cs.to_rgb(&comps).map_or([0; 4], Color::to_premultiplied);
                    cache = Some((raw.to_vec(), px));
                    px
                }
            };
            out[p * 4..p * 4 + 4].copy_from_slice(&px);
        }
        RgbaImage {
            width,
            height,
            data: out,
        }
    };

    let mut image = image;
    if let Some(Object::Stream(smask)) = resolve(dict.get(b"SMask")) {
        if let Some(alpha) = mask_alpha(doc, &smask, false) {
            apply_alpha(&mut image, &alpha);
        }
    } else if let Some(Object::Stream(mask)) = resolve(dict.get(b"Mask")) {
        if let Some(alpha) = mask_alpha(doc, &mask, true) {
            apply_alpha(&mut image, &alpha);
        }
    }
    Some(ImageInfo { image, interpolate })
}

/// A soft mask (gray samples are alpha) or a stencil /Mask (1-bit, where 1
/// means masked out unless Decode says otherwise), as an alpha plane.
fn mask_alpha(doc: &Document, stream: &Stream, stencil: bool) -> Option<(usize, usize, Vec<u8>)> {
    let decoded = doc.decode_stream(stream).ok()?;
    let codec = run_codec(&decoded.data, decoded.image_filter.as_ref()).ok()?;
    // A JPEG mask must be gray to be a usable alpha plane.
    if codec.as_ref().is_some_and(|c| c.components != 1 || stencil) {
        return None;
    }
    let data = codec.as_ref().map_or(&decoded.data[..], |c| &c.data[..]);
    let dict = &stream.dict;
    let (w, h) = match &codec {
        Some(c) => (c.width, c.height),
        None => (
            usize::try_from(doc.get_key(dict, b"Width").ok()?.as_i64()?).ok()?,
            usize::try_from(doc.get_key(dict, b"Height").ok()?.as_i64()?).ok()?,
        ),
    };
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
        return None;
    }
    let bpc = if stencil {
        1
    } else if codec.is_some() {
        8
    } else {
        doc.get_key(dict, b"BitsPerComponent").ok()?.as_i64()? as u32
    };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    let invert = doc.get_key(dict, b"Decode").ok().and_then(|d| {
        d.as_array()
            .and_then(|a| a.first().and_then(Object::as_f64))
    }) == Some(1.0);
    let max = (1u32 << bpc) - 1;
    let alpha = unpack(data, w, h, 1, bpc)
        .into_iter()
        .map(|v| {
            let a = (v * 255 / max) as u8;
            let a = if invert { 255 - a } else { a };
            if stencil { 255 - a } else { a }
        })
        .collect();
    Some((w, h, alpha))
}

/// Multiplies the image by an alpha plane, scaled to fit with nearest sampling.
fn apply_alpha(image: &mut RgbaImage, (mw, mh, alpha): &(usize, usize, Vec<u8>)) {
    for y in 0..image.height {
        let my = y * mh / image.height;
        for x in 0..image.width {
            let mx = x * mw / image.width;
            let a = u32::from(alpha[my * mw + mx]);
            let i = (y * image.width + x) * 4;
            for k in 0..4 {
                image.data[i + k] = ((u32::from(image.data[i + k]) * a + 127) / 255) as u8;
            }
        }
    }
}

/// Unpacks samples; rows start on byte boundaries. Short data reads as zero.
fn unpack(data: &[u8], width: usize, height: usize, comps: usize, bpc: u32) -> Vec<u32> {
    let per_row = width * comps;
    let row_bytes = (per_row * bpc as usize).div_ceil(8);
    let mut out = Vec::with_capacity(per_row * height);
    for y in 0..height {
        let row = data.get(y * row_bytes..).unwrap_or(&[]);
        for i in 0..per_row {
            let v = match bpc {
                8 => u32::from(row.get(i).copied().unwrap_or(0)),
                16 => {
                    let hi = u32::from(row.get(2 * i).copied().unwrap_or(0));
                    let lo = u32::from(row.get(2 * i + 1).copied().unwrap_or(0));
                    hi << 8 | lo
                }
                _ => {
                    let bit = i * bpc as usize;
                    let byte = u32::from(row.get(bit / 8).copied().unwrap_or(0));
                    let shift = 8 - bpc - (bit % 8) as u32;
                    (byte >> shift) & ((1 << bpc) - 1)
                }
            };
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpack_sub_byte_rows_are_byte_aligned() {
        // 3 pixels of 1 bit per row: 101 then 010, each row padded to a byte.
        assert_eq!(
            unpack(&[0b1010_0000, 0b0100_0000], 3, 2, 1, 1),
            vec![1, 0, 1, 0, 1, 0]
        );
        assert_eq!(unpack(&[0x12, 0x34], 1, 1, 1, 16), vec![0x1234]);
        assert_eq!(unpack(&[0xAB], 2, 1, 1, 4), vec![0xA, 0xB]);
    }
}
