//! Exact-area scan conversion.
//!
//! For every edge we add, to each cell it passes through, the signed area that
//! the edge contributes to that cell and to the cells right of it (the same
//! technique as font-rs and stb_truetype v2). A prefix sum along each row then
//! gives the winding-weighted coverage of every pixel.

use crate::{Path, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

/// Coverage (0 to 255) over a rectangle of pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Coverage {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Coverage {
    /// This coverage moved so its top-left pixel is at (x, y), which may be
    /// off the device, and cut to the rectangle `(ax, ay, aw, ah)`. Used to
    /// stamp a cached glyph mask. `None` when nothing is left.
    pub fn placed(
        &self,
        x: i64,
        y: i64,
        (ax, ay, aw, ah): (usize, usize, usize, usize),
    ) -> Option<Coverage> {
        let (ax, ay) = (ax as i64, ay as i64);
        let x0 = x.max(ax);
        let y0 = y.max(ay);
        let x1 = (x + self.width as i64).min(ax + aw as i64);
        let y1 = (y + self.height as i64).min(ay + ah as i64);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let (sx, sy) = ((x0 - x) as usize, (y0 - y) as usize);
        let mut data = Vec::with_capacity(w * h);
        for row in sy..sy + h {
            let start = row * self.width + sx;
            data.extend_from_slice(&self.data[start..start + w]);
        }
        Some(Coverage {
            x: x0 as usize,
            y: y0 as usize,
            width: w,
            height: h,
            data,
        })
    }

    pub fn get(&self, x: usize, y: usize) -> u8 {
        if x < self.x || y < self.y || x >= self.x + self.width || y >= self.y + self.height {
            return 0;
        }
        self.data[(y - self.y) * self.width + (x - self.x)]
    }
}

/// Curves are flattened to within this many device pixels.
pub const TOLERANCE: f64 = 0.2;

/// Scan converts a device-space path, clipped to `(width, height)`. Every
/// subpath is implicitly closed, as fills require. Returns `None` when nothing
/// is visible.
pub fn rasterize(path: &Path, rule: FillRule, (width, height): (usize, usize)) -> Option<Coverage> {
    rasterize_in(path, rule, (0, 0, width, height))
}

/// Like [`rasterize`], but only computes coverage inside the device-pixel
/// rectangle `(x, y, width, height)`; the rest is treated as 0. Clipped
/// drawing uses the clip's bounds here, so a shape far bigger than what
/// shows costs only what shows.
pub fn rasterize_in(
    path: &Path,
    rule: FillRule,
    (rx, ry, rw, rh): (usize, usize, usize, usize),
) -> Option<Coverage> {
    let polys = path.flatten(TOLERANCE);
    let mut min = Point::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (pts, _) in &polys {
        for p in pts {
            if !p.x.is_finite() || !p.y.is_finite() {
                return None;
            }
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }
    }
    let x0 = min.x.floor().max(rx as f64);
    let y0 = min.y.floor().max(ry as f64);
    let x1 = max.x.ceil().min((rx + rw) as f64);
    let y1 = max.y.ceil().min((ry + rh) as f64);
    if !(x0 < x1 && y0 < y1) {
        return None;
    }
    let (x0, y0) = (x0 as usize, y0 as usize);
    let (bw, bh) = (x1 as usize - x0, y1 as usize - y0);

    let mut acc = Accumulator::new(bw, bh);
    let offset = Point::new(x0 as f64, y0 as f64);
    for (pts, _) in &polys {
        if pts.len() < 2 {
            continue;
        }
        for i in 0..pts.len() {
            let a = pts[i] - offset;
            let b = pts[(i + 1) % pts.len()] - offset;
            acc.add_line(a, b);
        }
    }
    let data = acc.resolve(rule);
    Some(Coverage {
        x: x0,
        y: y0,
        width: bw,
        height: bh,
        data,
    })
}

struct Accumulator {
    width: usize,
    height: usize,
    /// Each row has width + 2 cells so contributions just past the right edge
    /// have somewhere to go.
    cells: Vec<f32>,
}

impl Accumulator {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0.0; (width + 2) * height],
        }
    }

    /// Adds an edge, splitting it where it crosses the left and right borders.
    /// Parts left of the buffer are pushed onto x = 0 (they still change the
    /// winding of every pixel in their rows); parts right of it do nothing.
    fn add_line(&mut self, a: Point, b: Point) {
        if a.y == b.y {
            return;
        }
        let w = self.width as f64;
        let mut cuts = [0.0f64, 1.0, 1.0, 1.0];
        let mut n = 1;
        for edge in [0.0, w] {
            if (a.x - edge) * (b.x - edge) < 0.0 {
                cuts[n] = (edge - a.x) / (b.x - a.x);
                n += 1;
            }
        }
        cuts[n] = 1.0;
        cuts[..=n].sort_by(f64::total_cmp);
        for k in 0..n {
            let p = a.lerp(b, cuts[k]);
            let q = a.lerp(b, cuts[k + 1]);
            let mid = (p.x + q.x) * 0.5;
            if mid >= w {
                continue;
            }
            if mid <= 0.0 {
                self.line(Point::new(0.0, p.y), Point::new(0.0, q.y));
            } else {
                let clamp = |v: f64| v.clamp(0.0, w);
                self.line(Point::new(clamp(p.x), p.y), Point::new(clamp(q.x), q.y));
            }
        }
    }

    fn line(&mut self, p0: Point, p1: Point) {
        if p0.y == p1.y {
            return;
        }
        let (dir, p0, p1) = if p0.y < p1.y {
            (1.0f32, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let h = self.height as f64;
        if p1.y <= 0.0 || p0.y >= h {
            return;
        }
        let stride = self.width + 2;
        let dxdy = (p1.x - p0.x) / (p1.y - p0.y);
        let mut x = p0.x;
        if p0.y < 0.0 {
            x = (x - p0.y * dxdy).clamp(0.0, self.width as f64);
        }
        let y_start = p0.y.max(0.0) as usize;
        let y_end = (p1.y.ceil().min(h)) as usize;
        for y in y_start..y_end {
            let row = y * stride;
            let yf = y as f64;
            let dy = ((yf + 1.0).min(p1.y) - yf.max(p0.y)) as f32;
            // Clamped so rounding can never step outside the row.
            let x_next = (x + dxdy * f64::from(dy)).clamp(0.0, self.width as f64);
            let d = dy * dir;
            let (xa, xb) = if x < x_next { (x, x_next) } else { (x_next, x) };
            let (xa, xb) = (xa as f32, xb as f32);
            let xa_floor = xa.floor();
            let xa_i = xa_floor as usize;
            let xb_ceil = xb.ceil();
            let xb_i = xb_ceil as usize;
            if xb_i <= xa_i + 1 {
                // The edge stays inside one cell on this row.
                let xmf = 0.5 * (x + x_next) as f32 - xa_floor;
                self.cells[row + xa_i] += d - d * xmf;
                self.cells[row + xa_i + 1] += d * xmf;
            } else {
                let s = (xb - xa).recip();
                let xa_f = xa - xa_floor;
                let a0 = 0.5 * s * (1.0 - xa_f) * (1.0 - xa_f);
                let xb_f = xb - xb_ceil + 1.0;
                let am = 0.5 * s * xb_f * xb_f;
                self.cells[row + xa_i] += d * a0;
                if xb_i == xa_i + 2 {
                    self.cells[row + xa_i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - xa_f);
                    self.cells[row + xa_i + 1] += d * (a1 - a0);
                    for xi in xa_i + 2..xb_i - 1 {
                        self.cells[row + xi] += d * s;
                    }
                    let a2 = a1 + (xb_i - xa_i - 3) as f32 * s;
                    self.cells[row + xb_i - 1] += d * (1.0 - a2 - am);
                }
                self.cells[row + xb_i] += d * am;
            }
            x = x_next;
        }
    }

    fn resolve(&self, rule: FillRule) -> Vec<u8> {
        let stride = self.width + 2;
        let mut out = vec![0u8; self.width * self.height];
        for y in 0..self.height {
            let mut acc = 0.0f32;
            let row = &self.cells[y * stride..y * stride + self.width];
            for (x, &c) in row.iter().enumerate() {
                acc += c;
                let a = acc.abs();
                let cov = match rule {
                    FillRule::NonZero => a.min(1.0),
                    FillRule::EvenOdd => {
                        let m = a - 2.0 * (a * 0.5).floor();
                        if m > 1.0 { 2.0 - m } else { m }
                    }
                };
                out[y * self.width + x] = (cov * 255.0 + 0.5) as u8;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_cuts_to_the_area() {
        let c = Coverage {
            x: 0,
            y: 0,
            width: 3,
            height: 2,
            data: vec![1, 2, 3, 4, 5, 6],
        };
        let p = c.placed(-1, 4, (0, 0, 10, 5)).unwrap();
        assert_eq!((p.x, p.y, p.width, p.height), (0, 4, 2, 1));
        assert_eq!(p.data, vec![2, 3]);
        assert!(c.placed(10, 0, (0, 0, 10, 5)).is_none());
        assert_eq!(c.placed(2, 1, (0, 0, 10, 5)).unwrap().data, c.data);
    }
    use crate::PathBuilder;

    fn poly(points: &[(f64, f64)]) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Point::new(points[0].0, points[0].1));
        for &(x, y) in &points[1..] {
            b.line_to(Point::new(x, y));
        }
        b.close();
        b.finish()
    }

    fn total(c: &Coverage) -> f64 {
        c.data.iter().map(|&v| f64::from(v) / 255.0).sum()
    }

    #[test]
    fn pixel_aligned_square_is_solid() {
        let c = rasterize(
            &poly(&[(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)]),
            FillRule::NonZero,
            (10, 10),
        )
        .unwrap();
        assert_eq!((c.x, c.y, c.width, c.height), (2, 2, 4, 4));
        assert!(c.data.iter().all(|&v| v == 255));
    }

    #[test]
    fn half_pixel_offset_gives_half_coverage_at_edges() {
        let c = rasterize(
            &poly(&[(1.5, 1.0), (4.5, 1.0), (4.5, 2.0), (1.5, 2.0)]),
            FillRule::NonZero,
            (10, 10),
        )
        .unwrap();
        assert_eq!(c.get(1, 1), 128);
        assert_eq!(c.get(2, 1), 255);
        assert_eq!(c.get(4, 1), 128);
    }

    #[test]
    fn area_is_exact_for_a_triangle() {
        let c = rasterize(
            &poly(&[(0.3, 0.7), (17.9, 3.1), (5.2, 12.6)]),
            FillRule::NonZero,
            (20, 20),
        )
        .unwrap();
        let expected = 0.5f64 * ((17.9f64 - 0.3) * (12.6 - 0.7) - (5.2 - 0.3) * (3.1 - 0.7)).abs();
        assert!(
            (total(&c) - expected).abs() < 0.2,
            "{} vs {expected}",
            total(&c)
        );
    }

    #[test]
    fn orientation_does_not_matter() {
        let a = rasterize(
            &poly(&[(1.2, 1.0), (8.0, 2.0), (3.0, 9.0)]),
            FillRule::NonZero,
            (10, 10),
        )
        .unwrap();
        let b = rasterize(
            &poly(&[(3.0, 9.0), (8.0, 2.0), (1.2, 1.0)]),
            FillRule::NonZero,
            (10, 10),
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn even_odd_punches_holes_nonzero_does_not() {
        let mut b = PathBuilder::new();
        for (x0, x1) in [(0.0, 10.0), (3.0, 7.0)] {
            b.move_to(Point::new(x0, x0));
            b.line_to(Point::new(x1, x0));
            b.line_to(Point::new(x1, x1));
            b.line_to(Point::new(x0, x1));
            b.close();
        }
        let path = b.finish();
        let eo = rasterize(&path, FillRule::EvenOdd, (10, 10)).unwrap();
        // Rasterizing a window gives the same pixels as the whole.
        let part = rasterize_in(&path, FillRule::EvenOdd, (3, 2, 5, 6)).unwrap();
        for y in 0..10 {
            for x in 0..10 {
                let inside = (3..8).contains(&x) && (2..8).contains(&y);
                assert_eq!(part.get(x, y), if inside { eo.get(x, y) } else { 0 });
            }
        }
        let nz = rasterize(&path, FillRule::NonZero, (10, 10)).unwrap();
        assert_eq!(eo.get(5, 5), 0);
        assert_eq!(eo.get(1, 1), 255);
        assert_eq!(nz.get(5, 5), 255);
    }

    #[test]
    fn shapes_past_the_left_edge_still_fill_rows() {
        let c = rasterize(
            &poly(&[(-50.0, 0.0), (5.0, 0.0), (5.0, 3.0), (-50.0, 3.0)]),
            FillRule::NonZero,
            (10, 10),
        )
        .unwrap();
        assert_eq!((c.x, c.width), (0, 5));
        assert!(c.data.iter().all(|&v| v == 255));
    }

    #[test]
    fn shapes_past_every_edge_are_clipped() {
        let c = rasterize(
            &poly(&[(-5.0, -5.0), (50.0, -5.0), (50.0, 50.0), (-5.0, 50.0)]),
            FillRule::NonZero,
            (8, 6),
        )
        .unwrap();
        assert_eq!((c.width, c.height), (8, 6));
        assert!(c.data.iter().all(|&v| v == 255));
    }

    #[test]
    fn offscreen_and_degenerate_paths_are_none() {
        assert!(
            rasterize(
                &poly(&[(20.0, 20.0), (30.0, 20.0), (30.0, 30.0)]),
                FillRule::NonZero,
                (10, 10)
            )
            .is_none()
        );
        assert!(
            rasterize(
                &poly(&[(1.0, 1.0), (f64::NAN, 2.0), (3.0, 3.0)]),
                FillRule::NonZero,
                (10, 10)
            )
            .is_none()
        );
    }
}
