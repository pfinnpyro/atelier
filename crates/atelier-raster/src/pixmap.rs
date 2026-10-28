use crate::Coverage;

/// A colour with straight (not premultiplied) alpha, components in 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// Premultiplied RGBA in 0..=255.
    pub fn to_premultiplied(self) -> [u8; 4] {
        let a = self.a.clamp(0.0, 1.0);
        let c = |v: f32| (v.clamp(0.0, 1.0) * a * 255.0 + 0.5) as u8;
        [c(self.r), c(self.g), c(self.b), (a * 255.0 + 0.5) as u8]
    }
}

/// An RGBA image with premultiplied alpha, 8 bits per channel, rows top down.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixmap {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl Pixmap {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0; width * height * 4],
        }
    }

    pub fn filled(width: usize, height: usize, color: Color) -> Self {
        let px = color.to_premultiplied();
        let mut data = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            data.extend_from_slice(&px);
        }
        Self {
            width,
            height,
            data,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn bounds(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Composites `color` source-over through `coverage`, optionally clipped.
    pub fn fill_coverage(&mut self, coverage: &Coverage, color: Color, clip: Option<&Mask>) {
        let src = color.to_premultiplied();
        for row in 0..coverage.height {
            let y = coverage.y + row;
            for col in 0..coverage.width {
                let x = coverage.x + col;
                let mut c = u32::from(coverage.data[row * coverage.width + col]);
                if let Some(m) = clip {
                    c = mul255(c, u32::from(m.get(x, y)));
                }
                if c == 0 {
                    continue;
                }
                let px = [
                    mul255(u32::from(src[0]), c),
                    mul255(u32::from(src[1]), c),
                    mul255(u32::from(src[2]), c),
                    mul255(u32::from(src[3]), c),
                ];
                self.blend_pixel(x, y, px);
            }
        }
    }

    /// Source-over of one premultiplied pixel.
    #[inline]
    pub fn blend_pixel(&mut self, x: usize, y: usize, src: [u32; 4]) {
        let i = (y * self.width + x) * 4;
        let inv = 255 - src[3];
        for (d, s) in self.data[i..i + 4].iter_mut().zip(src) {
            *d = (s + mul255(u32::from(*d), inv)).min(255) as u8;
        }
    }

    /// Straight-alpha RGBA, for writing image files.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = self.data.clone();
        for px in out.chunks_exact_mut(4) {
            let a = u32::from(px[3]);
            if a != 0 && a != 255 {
                for c in &mut px[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }
}

/// a * b / 255, rounded.
#[inline]
pub(crate) fn mul255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    (t + (t >> 8)) >> 8
}

/// A per-pixel alpha mask the size of the pixmap, used for clipping.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl Mask {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0; width * height],
        }
    }

    pub fn from_coverage(c: Option<&Coverage>, width: usize, height: usize) -> Self {
        let mut m = Self::new(width, height);
        if let Some(c) = c {
            for row in 0..c.height {
                let dst = (c.y + row) * width + c.x;
                m.data[dst..dst + c.width]
                    .copy_from_slice(&c.data[row * c.width..(row + 1) * c.width]);
            }
        }
        m
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.width + x]
    }

    /// Multiplies this mask by `other`'s coverage; pixels outside it become 0.
    pub fn intersect(&mut self, other: Option<&Coverage>) {
        let Some(c) = other else {
            self.data.fill(0);
            return;
        };
        for y in 0..self.height {
            for x in 0..self.width {
                let i = y * self.width + x;
                self.data[i] = mul255(u32::from(self.data[i]), u32::from(c.get(x, y))) as u8;
            }
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul255_is_exact_at_the_ends() {
        assert_eq!(mul255(255, 255), 255);
        assert_eq!(mul255(255, 0), 0);
        assert_eq!(mul255(200, 255), 200);
        assert_eq!(mul255(128, 128), 64);
    }

    #[test]
    fn half_alpha_over_white() {
        let mut p = Pixmap::filled(1, 1, Color::WHITE);
        let cov = Coverage {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            data: vec![255],
        };
        p.fill_coverage(&cov, Color::rgb(1.0, 0.0, 0.0).with_alpha(0.5), None);
        assert_eq!(p.pixel(0, 0), [255, 127, 127, 255]);
    }
}
