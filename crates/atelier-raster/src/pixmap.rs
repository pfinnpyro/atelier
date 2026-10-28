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
        let (rows, cols) = visible_span(coverage, clip);
        for row in rows {
            let y = coverage.y + row;
            for col in cols.clone() {
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
/// The rows and columns of `coverage` that a clip can let through.
pub(crate) fn visible_span(
    coverage: &Coverage,
    clip: Option<&Mask>,
) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let Some(m) = clip else {
        return (0..coverage.height, 0..coverage.width);
    };
    let (mx, my, mw, mh) = m.bounds();
    let span = |start: usize, len: usize, m0: usize, mlen: usize| {
        let a = m0.saturating_sub(start).min(len);
        let b = (m0 + mlen).saturating_sub(start).min(len);
        a..b.max(a)
    };
    (
        span(coverage.y, coverage.height, my, mh),
        span(coverage.x, coverage.width, mx, mw),
    )
}

/// A clip: how much of each device pixel may be painted. Only the area
/// the clip covers is stored; everything outside it is 0. A clip that is a
/// pixel-aligned rectangle (the usual case) stores no pixels at all.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    width: usize,
    height: usize,
    /// The covered area: `x..x + bw`, `y..y + bh`.
    x: usize,
    y: usize,
    bw: usize,
    bh: usize,
    /// Coverage inside the area, row by row; `None` when it is all 255.
    data: Option<Vec<u8>>,
}

impl Mask {
    /// A mask that lets nothing through.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            x: 0,
            y: 0,
            bw: 0,
            bh: 0,
            data: None,
        }
    }

    pub fn from_coverage(c: Option<&Coverage>, width: usize, height: usize) -> Self {
        let Some(c) = c else {
            return Self::new(width, height);
        };
        // Coverage always lies inside the pixmap it was rasterized for, but
        // clip it anyway so a mismatch cannot index out of range.
        let bw = c.width.min(width.saturating_sub(c.x));
        let bh = c.height.min(height.saturating_sub(c.y));
        let mut data = Vec::with_capacity(bw * bh);
        for row in 0..bh {
            data.extend_from_slice(&c.data[row * c.width..row * c.width + bw]);
        }
        let solid = data.iter().all(|&v| v == 255);
        Self {
            width,
            height,
            x: c.x,
            y: c.y,
            bw,
            bh,
            data: (!solid).then_some(data),
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x < self.x || y < self.y || x >= self.x + self.bw || y >= self.y + self.bh {
            return 0;
        }
        match &self.data {
            Some(d) => d[(y - self.y) * self.bw + (x - self.x)],
            None => 255,
        }
    }

    /// The area outside which the mask is 0, as `(x, y, width, height)`.
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
        (self.x, self.y, self.bw, self.bh)
    }

    /// Multiplies this mask by `other`'s coverage; pixels outside it become 0.
    pub fn intersect(&mut self, other: Option<&Coverage>) {
        let other = Mask::from_coverage(other, self.width, self.height);
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.bw).min(other.x + other.bw);
        let y1 = (self.y + self.bh).min(other.y + other.bh);
        if x0 >= x1 || y0 >= y1 {
            *self = Self::new(self.width, self.height);
            return;
        }
        let data = if self.data.is_none() && other.data.is_none() {
            None
        } else {
            let mut d = Vec::with_capacity((x1 - x0) * (y1 - y0));
            for y in y0..y1 {
                for x in x0..x1 {
                    d.push(mul255(u32::from(self.get(x, y)), u32::from(other.get(x, y))) as u8);
                }
            }
            Some(d)
        };
        *self = Self {
            width: self.width,
            height: self.height,
            x: x0,
            y: y0,
            bw: x1 - x0,
            bh: y1 - y0,
            data,
        };
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

    fn cov(x: usize, y: usize, w: usize, h: usize, v: u8) -> Coverage {
        Coverage {
            x,
            y,
            width: w,
            height: h,
            data: vec![v; w * h],
        }
    }

    #[test]
    fn mask_keeps_only_its_area() {
        let mut m = Mask::from_coverage(Some(&cov(10, 20, 30, 40, 255)), 100, 100);
        assert_eq!(m.bounds(), (10, 20, 30, 40));
        assert!(m.data.is_none(), "a solid rectangle stores no pixels");
        assert_eq!(
            (m.get(10, 20), m.get(39, 59), m.get(40, 20), m.get(9, 20)),
            (255, 255, 0, 0)
        );
        m.intersect(Some(&cov(30, 0, 50, 30, 128)));
        assert_eq!(m.bounds(), (30, 20, 10, 10));
        assert_eq!((m.get(30, 20), m.get(29, 20), m.get(39, 29)), (128, 0, 128));
        m.intersect(Some(&cov(0, 0, 5, 5, 255)));
        assert_eq!(m.bounds().2 * m.bounds().3, 0);
        assert_eq!(m.get(30, 20), 0);
        let mut none = Mask::from_coverage(Some(&cov(0, 0, 10, 10, 255)), 100, 100);
        none.intersect(None);
        assert_eq!(none.get(0, 0), 0);
    }

    #[test]
    fn fill_skips_what_the_clip_hides() {
        let mut p = Pixmap::filled(20, 20, Color::WHITE);
        let clip = Mask::from_coverage(Some(&cov(5, 5, 5, 5, 255)), 20, 20);
        p.fill_coverage(&cov(0, 0, 20, 20, 255), Color::BLACK, Some(&clip));
        assert_eq!(p.pixel(4, 4), [255, 255, 255, 255]);
        assert_eq!(p.pixel(5, 5), [0, 0, 0, 255]);
        assert_eq!(p.pixel(9, 9), [0, 0, 0, 255]);
        assert_eq!(p.pixel(10, 9), [255, 255, 255, 255]);
    }

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
