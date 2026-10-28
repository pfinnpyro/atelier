//! Drawing images through an arbitrary affine transform.

use crate::pixmap::mul255;
use crate::{FillRule, Mask, Path, Pixmap, Point, Transform, rasterize};

/// A decoded image: premultiplied RGBA8, rows top down.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl RgbaImage {
    fn pixel(&self, x: usize, y: usize) -> [u32; 4] {
        let i = (y * self.width + x) * 4;
        [
            u32::from(self.data[i]),
            u32::from(self.data[i + 1]),
            u32::from(self.data[i + 2]),
            u32::from(self.data[i + 3]),
        ]
    }

    /// Box-filters the image down by an integer factor in each direction.
    fn downsample(&self, fx: usize, fy: usize) -> RgbaImage {
        let w = self.width.div_ceil(fx);
        let h = self.height.div_ceil(fy);
        let mut data = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 4];
                let mut n = 0;
                for sy in y * fy..((y + 1) * fy).min(self.height) {
                    for sx in x * fx..((x + 1) * fx).min(self.width) {
                        let p = self.pixel(sx, sy);
                        for k in 0..4 {
                            sum[k] += p[k];
                        }
                        n += 1;
                    }
                }
                let i = (y * w + x) * 4;
                for k in 0..4 {
                    data[i + k] = ((sum[k] + n / 2) / n.max(1)) as u8;
                }
            }
        }
        RgbaImage {
            width: w,
            height: h,
            data,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSampling {
    Nearest,
    Bilinear,
}

/// Draws `image` into `pixmap`. `ctm` maps the PDF unit square (image row 0 at
/// the top, y = 1) to device space, exactly as for the `Do` operator.
pub fn draw_image(
    pixmap: &mut Pixmap,
    image: &RgbaImage,
    ctm: &Transform,
    sampling: ImageSampling,
    alpha: f32,
    clip: Option<&Mask>,
) {
    if image.width == 0 || image.height == 0 {
        return;
    }
    // Edges anti-aliased by rasterizing the unit square itself.
    let square = Path::rect(0.0, 0.0, 1.0, 1.0).transform(ctm);
    let Some(coverage) = rasterize(&square, FillRule::NonZero, pixmap.bounds()) else {
        return;
    };
    let Some(inverse) = ctm.invert() else { return };

    // Shrink first when many image pixels land on one device pixel, so
    // sampling does not alias.
    let px_per_device_x = inverse.apply_vector(Point::new(1.0, 0.0)).length() * image.width as f64;
    let px_per_device_y = inverse.apply_vector(Point::new(0.0, 1.0)).length() * image.height as f64;
    let shrink = |v: f64| if v >= 2.0 { v.floor() as usize } else { 1 };
    let (fx, fy) = (
        shrink(px_per_device_x.min(px_per_device_y)),
        shrink(px_per_device_y.min(px_per_device_x)),
    );
    let reduced;
    let img = if fx > 1 || fy > 1 {
        reduced = image.downsample(fx, fy);
        &reduced
    } else {
        image
    };
    let (w, h) = (img.width as f64, img.height as f64);
    let global = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;

    for row in 0..coverage.height {
        let y = coverage.y + row;
        for col in 0..coverage.width {
            let x = coverage.x + col;
            let mut c = u32::from(coverage.data[row * coverage.width + col]);
            if let Some(m) = clip {
                c = mul255(c, u32::from(m.get(x, y)));
            }
            c = mul255(c, global);
            if c == 0 {
                continue;
            }
            let u = inverse.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
            // Unit square to image pixels: x right, y down from the top.
            let ix = u.x * w;
            let iy = (1.0 - u.y) * h;
            let px = match sampling {
                ImageSampling::Nearest => {
                    let sx = (ix.floor().max(0.0) as usize).min(img.width - 1);
                    let sy = (iy.floor().max(0.0) as usize).min(img.height - 1);
                    img.pixel(sx, sy)
                }
                ImageSampling::Bilinear => bilinear(img, ix - 0.5, iy - 0.5),
            };
            let src = [
                mul255(px[0], c),
                mul255(px[1], c),
                mul255(px[2], c),
                mul255(px[3], c),
            ];
            pixmap.blend_pixel(x, y, src);
        }
    }
}

fn bilinear(img: &RgbaImage, x: f64, y: f64) -> [u32; 4] {
    let max_x = (img.width - 1) as f64;
    let max_y = (img.height - 1) as f64;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = (x - x0 as f64, y - y0 as f64);
    let (a, b, c, d) = (
        img.pixel(x0, y0),
        img.pixel(x1, y0),
        img.pixel(x0, y1),
        img.pixel(x1, y1),
    );
    let mut out = [0u32; 4];
    for k in 0..4 {
        let top = f64::from(a[k]) * (1.0 - tx) + f64::from(b[k]) * tx;
        let bottom = f64::from(c[k]) * (1.0 - tx) + f64::from(d[k]) * tx;
        out[k] = (top * (1.0 - ty) + bottom * ty + 0.5) as u32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Color;

    fn checker() -> RgbaImage {
        // 2x2: red, green / blue, white (top row first).
        RgbaImage {
            width: 2,
            height: 2,
            data: vec![
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 255, 255, 255, 255,
            ],
        }
    }

    #[test]
    fn image_row_zero_lands_at_the_top() {
        let mut p = Pixmap::filled(20, 20, Color::BLACK);
        // Unit square to the device rectangle (0,0)-(20,20) with y flipped,
        // as a page transform does.
        let ctm = Transform::new(20.0, 0.0, 0.0, -20.0, 0.0, 20.0);
        draw_image(&mut p, &checker(), &ctm, ImageSampling::Nearest, 1.0, None);
        assert_eq!(p.pixel(2, 2), [255, 0, 0, 255]);
        assert_eq!(p.pixel(17, 2), [0, 255, 0, 255]);
        assert_eq!(p.pixel(2, 17), [0, 0, 255, 255]);
        assert_eq!(p.pixel(17, 17), [255, 255, 255, 255]);
    }

    #[test]
    fn large_image_is_averaged_when_shrunk() {
        // 100x100 stripes of black and white, drawn into 10x10.
        let mut data = Vec::new();
        for _y in 0..100 {
            for x in 0..100 {
                let v = if x % 2 == 0 { 0 } else { 255 };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let img = RgbaImage {
            width: 100,
            height: 100,
            data,
        };
        let mut p = Pixmap::filled(10, 10, Color::BLACK);
        let ctm = Transform::new(10.0, 0.0, 0.0, -10.0, 0.0, 10.0);
        draw_image(&mut p, &img, &ctm, ImageSampling::Bilinear, 1.0, None);
        let v = p.pixel(5, 5)[0];
        assert!((120..=135).contains(&v), "{v}");
    }
}
