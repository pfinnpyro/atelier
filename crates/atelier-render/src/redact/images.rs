use atelier_filters::encode_flate;
use atelier_raster::{Color, RgbaImage};

use super::*;

const MAX_PIXELS: usize = 24_000_000;

pub(super) fn rewrite(
    doc: &Document,
    page: &Page,
    source: &Stream,
    matrix: Transform,
    fill: Color,
    regions: impl Iterator<Item = Rect>,
    assets: &mut Assets<'_, '_>,
) -> Result<Object> {
    if source.dict.contains_key(b"OC") {
        return Err(unsupported("Optional-content images require raster export"));
    }
    let dimension = |key| -> Result<usize> {
        doc.get_key(&source.dict, key)?
            .as_i64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|&v| v != 0)
            .ok_or_else(|| unsupported("Invalid image dimensions"))
    };
    let (width, height) = (dimension(b"Width")?, dimension(b"Height")?);
    if width.saturating_mul(height) > MAX_PIXELS {
        return Err(unsupported("Image exceeds the redaction pixel limit"));
    }
    if let Some(mask) = source.dict.get(b"SMask") {
        match doc.resolve(mask)? {
            Object::Stream(mask)
                if !mask.dict.contains_key(b"Matte") && valid_mask(doc, &mask, false) => {}
            Object::Name(name) if name.as_bytes() == b"None" => {}
            _ => return Err(unsupported("Unsupported image soft mask")),
        }
    } else if let Some(mask) = source.dict.get(b"Mask") {
        match doc.resolve(mask)? {
            Object::Array(_) => {}
            Object::Stream(mask) if valid_mask(doc, &mask, true) => {}
            _ => return Err(unsupported("Unsupported image mask")),
        }
    }
    let decoded = doc.decode_stream(source)?;
    let mut warned = false;
    let mut info = crate::image::decode(
        doc,
        &source.dict,
        &decoded.data,
        decoded.image_filter.as_ref(),
        &page.resources,
        fill,
        &mut |_| warned = true,
    )
    .filter(|i| i.image.width == width && i.image.height == height)
    .ok_or_else(|| unsupported("An image could not be decoded for redaction"))?;
    if warned {
        return Err(unsupported("Image decoding reported warnings"));
    }
    let inverse = matrix
        .invert()
        .ok_or_else(|| unsupported("Invalid image transform"))?;
    for region in regions {
        remove_pixels(&mut info.image, matrix, inverse, region, info.interpolate)?;
    }
    encode(&info.image, info.interpolate, assets)
}

fn valid_mask(doc: &Document, mask: &Stream, stencil: bool) -> bool {
    let size = |key| {
        doc.get_key(&mask.dict, key)
            .ok()
            .and_then(|o| usize::try_from(o.as_i64()?).ok())
            .unwrap_or_default()
    };
    let pixels = size(b"Width").saturating_mul(size(b"Height"));
    (1..=MAX_PIXELS).contains(&pixels)
        && crate::image::mask_alpha(doc, mask, stencil)
            .is_some_and(|(w, h, _)| w == size(b"Width") && h == size(b"Height"))
}

fn remove_pixels(
    image: &mut RgbaImage,
    matrix: Transform,
    inverse: Transform,
    region: Rect,
    interpolate: bool,
) -> Result<()> {
    let (width, height) = (image.width, image.height);
    let mut bounds = None;
    add_box(&mut bounds, inverse, region)?;
    let bounds = bounds.unwrap();
    // A bilinear sample can extend half a pixel past the nominal cell.
    let padding = if interpolate { 0.5 } else { 0.0 };
    let first = |v: f64, size| (v * size as f64 - padding).floor().max(0.0) as usize;
    let end = |v: f64, size| ((v * size as f64 + padding).ceil() as usize).min(size);
    let x_range = first(bounds.x0, width)..end(bounds.x1, width);
    let y_range = first(1.0 - bounds.y1, height)..end(1.0 - bounds.y0, height);
    let half_x = (0.5 + padding) * (matrix.a.abs() / width as f64 + matrix.c.abs() / height as f64);
    let half_y = (0.5 + padding) * (matrix.b.abs() / width as f64 + matrix.d.abs() / height as f64);
    for y in y_range {
        for x in x_range.clone() {
            // Candidate bounds test the two image axes. For rotated or sheared
            // images also test the page axes, avoiding cuts outside the mark.
            let center = matrix.apply(Point::new(
                (x as f64 + 0.5) / width as f64,
                1.0 - (y as f64 + 0.5) / height as f64,
            ));
            if center.x + half_x >= region.x0
                && center.x - half_x <= region.x1
                && center.y + half_y >= region.y0
                && center.y - half_y <= region.y1
            {
                let index = (y * width + x) * 4;
                image.data[index..index + 4].fill(0);
            }
        }
    }
    Ok(())
}

fn encode(image: &RgbaImage, interpolate: bool, assets: &mut Assets<'_, '_>) -> Result<Object> {
    let mut rgb = Vec::with_capacity(image.width * image.height * 3);
    let mut alpha = Vec::with_capacity(image.width * image.height);
    let mut has_alpha = false;
    for px in image.data.chunks_exact(4) {
        let a = u32::from(px[3]);
        alpha.push(px[3]);
        has_alpha |= a != 255;
        // Transparent pixels have no recoverable color payload.
        for &c in &px[..3] {
            rgb.push(
                (u32::from(c) * 255 + a / 2)
                    .checked_div(a)
                    .unwrap_or(0)
                    .min(255) as u8,
            );
        }
    }
    let mut dict = image_dict(image, interpolate, "DeviceRGB");
    if has_alpha {
        let mask = image_dict(image, interpolate, "DeviceGray");
        dict.insert("SMask", assets.add(mask, encode_flate(&alpha))?);
    }
    assets.add(dict, encode_flate(&rgb))
}

fn image_dict(image: &RgbaImage, interpolate: bool, space: &str) -> Dict {
    let mut dict = Dict::new();
    for (key, value) in [
        ("Type", "XObject"),
        ("Subtype", "Image"),
        ("ColorSpace", space),
        ("Filter", "FlateDecode"),
    ] {
        dict.insert(key, Object::Name(Name::from(value)));
    }
    dict.insert("Width", Object::Integer(image.width as i64));
    dict.insert("Height", Object::Integer(image.height as i64));
    dict.insert("BitsPerComponent", Object::Integer(8));
    dict.insert("Interpolate", Object::Bool(interpolate));
    dict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_image_cells_outside_the_mark_are_preserved() {
        let mut image = RgbaImage {
            width: 4,
            height: 4,
            data: vec![255; 4 * 4 * 4],
        };
        let matrix = Transform::new(40.0, 40.0, -40.0, 40.0, 40.0, 0.0);
        let mark = Rect {
            x0: 25.0,
            y0: 25.0,
            x1: 55.0,
            y1: 55.0,
        };
        remove_pixels(&mut image, matrix, matrix.invert().unwrap(), mark, false).unwrap();
        let alpha: Vec<_> = image.data.chunks_exact(4).map(|px| px[3]).collect();
        assert_eq!(
            alpha,
            [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 255,]
        );
    }
}
