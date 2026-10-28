//! Stroking (ISO 32000-2, 8.5.3.2 and 8.4.3): turns a path into filled polygons.
//!
//! Work happens in user space so a non-uniform transform shapes the pen
//! correctly, then each piece is transformed to device space. Every piece is
//! given the same orientation, so filling the result with the non-zero rule
//! unions them.

use std::f64::consts::PI;

use crate::path::PathBuilder;
use crate::{Path, Point, Transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_phase: f64,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_phase: 0.0,
        }
    }
}

/// Device-space tolerance for curve flattening and round shapes.
const TOLERANCE: f64 = 0.2;

/// Below this turning angle a join is drawn as a bevel whatever the style;
/// the difference is far smaller than a pixel.
const SMOOTH_TURN: f64 = 0.1;

/// Returns device-space polygons covering the stroke of `path` (given in user
/// space) under `ctm`. Fill them with [`crate::FillRule::NonZero`].
pub fn stroke_to_path(path: &Path, style: &StrokeStyle, ctm: &Transform) -> Path {
    let scale = ctm.max_scale();
    if scale.is_nan() || scale <= 0.0 || !ctm.is_finite() {
        return Path::default();
    }
    // Width 0 means the thinnest line the device can show: one pixel.
    let width = if style.width > 0.0 {
        style.width
    } else {
        1.0 / scale
    };
    let mut s = Stroker {
        hw: width / 2.0,
        style,
        ctm,
        out: PathBuilder::new(),
    };
    for (pts, closed) in path.flatten(TOLERANCE / scale) {
        let pts = dedupe(pts, closed);
        for (piece, piece_closed) in dash(&pts, closed, &style.dash, style.dash_phase) {
            s.polyline(&piece, piece_closed);
        }
    }
    s.out.finish()
}

fn dedupe(pts: Vec<Point>, closed: bool) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(pts.len());
    for p in pts {
        if out.last().is_none_or(|q| (*q - p).length() > 1e-9) {
            out.push(p);
        }
    }
    if closed && out.len() > 1 && (out[0] - out[out.len() - 1]).length() <= 1e-9 {
        out.pop();
    }
    out
}

/// Splits a polyline into its "on" dashes. Without a usable pattern the line is
/// returned unchanged.
fn dash(pts: &[Point], closed: bool, pattern: &[f64], phase: f64) -> Vec<(Vec<Point>, bool)> {
    let total: f64 = pattern.iter().sum();
    if pattern.is_empty()
        || total.is_nan()
        || total <= 0.0
        || pattern.iter().any(|d| *d < 0.0 || !d.is_finite())
    {
        return vec![(pts.to_vec(), closed)];
    }
    let mut pts = pts.to_vec();
    if closed && pts.len() > 1 {
        pts.push(pts[0]);
    }
    if pts.len() == 1 {
        return vec![(pts, false)];
    }

    // Find where the phase lands in the pattern.
    let mut idx = 0;
    let mut left = phase.rem_euclid(total);
    // A zero-length dash exactly at the phase is kept: it draws a dot.
    while left > pattern[idx] || (left == pattern[idx] && pattern[idx] > 0.0) {
        left -= pattern[idx];
        idx = (idx + 1) % pattern.len();
    }
    let mut remaining = pattern[idx] - left;
    let mut on = idx % 2 == 0;

    let mut out = Vec::new();
    let mut current: Vec<Point> = if on { vec![pts[0]] } else { Vec::new() };
    let mut guard = 0usize;
    for seg in pts.windows(2) {
        let (mut a, b) = (seg[0], seg[1]);
        let mut seg_len = (b - a).length();
        while seg_len > remaining {
            guard += 1;
            if guard > 1_000_000 {
                return out; // absurdly fine dash pattern
            }
            let p = a.lerp(b, remaining / seg_len);
            if on {
                current.push(p);
                out.push((std::mem::take(&mut current), false));
            } else {
                current = vec![p];
            }
            seg_len -= remaining;
            a = p;
            on = !on;
            idx = (idx + 1) % pattern.len();
            remaining = pattern[idx];
        }
        remaining -= seg_len;
        if on {
            current.push(b);
        }
    }
    if on && !current.is_empty() {
        out.push((current, false));
    } else if remaining.abs() < 1e-6 {
        // The next dash starts exactly at the end of the line.
        if let Some(&end) = pts.last() {
            out.push((vec![end], false));
        }
    }
    out
}

struct Stroker<'a> {
    hw: f64,
    style: &'a StrokeStyle,
    ctm: &'a Transform,
    out: PathBuilder,
}

impl Stroker<'_> {
    fn polyline(&mut self, pts: &[Point], closed: bool) {
        match pts.len() {
            0 => {}
            1 => self.dot(pts[0]),
            _ => {
                let n = pts.len();
                let segs = if closed { n } else { n - 1 };
                for i in 0..segs {
                    let a = pts[i];
                    let b = pts[(i + 1) % n];
                    let nrm = (b - a).normalize().perp() * self.hw;
                    self.polygon(&[a + nrm, b + nrm, b - nrm, a - nrm]);
                }
                let joins = if closed { 0..n } else { 1..n - 1 };
                for i in joins {
                    let prev = pts[(i + n - 1) % n];
                    let next = pts[(i + 1) % n];
                    self.join(prev, pts[i], next);
                }
                if !closed {
                    self.cap(pts[0], (pts[0] - pts[1]).normalize());
                    self.cap(pts[n - 1], (pts[n - 1] - pts[n - 2]).normalize());
                }
            }
        }
    }

    /// A zero-length subpath: round caps draw a dot, square caps a square.
    fn dot(&mut self, p: Point) {
        match self.style.cap {
            LineCap::Butt => {}
            LineCap::Round => self.circle(p),
            LineCap::Square => {
                let h = self.hw;
                self.polygon(&[
                    p + Point::new(-h, -h),
                    p + Point::new(h, -h),
                    p + Point::new(h, h),
                    p + Point::new(-h, h),
                ]);
            }
        }
    }

    /// Cap at endpoint `p`; `out` points away from the line.
    fn cap(&mut self, p: Point, out: Point) {
        match self.style.cap {
            LineCap::Butt => {}
            LineCap::Round => self.circle(p),
            LineCap::Square => {
                let n = out.perp() * self.hw;
                let e = out * self.hw;
                self.polygon(&[p + n, p + n + e, p - n + e, p - n]);
            }
        }
    }

    fn join(&mut self, prev: Point, v: Point, next: Point) {
        let d0 = (v - prev).normalize();
        let d1 = (next - v).normalize();
        let cross = d0.cross(d1);
        let dot = d0.dot(d1);
        let turn = cross.atan2(dot).abs();
        if turn < 1e-9 {
            return;
        }
        // The outer side of the turn.
        let side = if cross > 0.0 { -1.0 } else { 1.0 };
        let n0 = d0.perp() * (self.hw * side);
        let n1 = d1.perp() * (self.hw * side);
        let bevel = [v, v + n0, v + n1];

        if turn < SMOOTH_TURN {
            self.polygon(&bevel);
            return;
        }
        match self.style.join {
            LineJoin::Bevel => self.polygon(&bevel),
            LineJoin::Round => self.circle(v),
            LineJoin::Miter => {
                // Miter length over line width is 1 / sin(theta / 2), with theta
                // the angle between the segments: pi - turn.
                let half = (PI - turn) / 2.0;
                let ratio = 1.0 / half.sin();
                if ratio <= self.style.miter_limit && half.sin() > 1e-6 {
                    let tip = v + (n0 + n1).normalize() * (self.hw * ratio);
                    self.polygon(&[v, v + n0, tip, v + n1]);
                } else {
                    self.polygon(&bevel);
                }
            }
        }
    }

    fn circle(&mut self, c: Point) {
        // Round shapes get a tighter tolerance: their area error shows as dots
        // and caps that look slightly too thin.
        let tol = TOLERANCE / 4.0;
        let device_r = self.hw * self.ctm.max_scale();
        let steps = if device_r <= tol {
            8
        } else {
            let a = (1.0 - tol / device_r).clamp(-1.0, 1.0).acos();
            ((PI / a).ceil() as usize).clamp(8, 512)
        };
        let pts: Vec<Point> = (0..steps)
            .map(|i| {
                let t = 2.0 * PI * i as f64 / steps as f64;
                c + Point::new(t.cos(), t.sin()) * self.hw
            })
            .collect();
        self.polygon(&pts);
    }

    /// Adds a user-space polygon in device space, oriented consistently.
    fn polygon(&mut self, pts: &[Point]) {
        let dev: Vec<Point> = pts.iter().map(|p| self.ctm.apply(*p)).collect();
        let area: f64 = (0..dev.len())
            .map(|i| dev[i].cross(dev[(i + 1) % dev.len()]))
            .sum();
        if area.abs() < 1e-12 {
            return;
        }
        let ordered: Box<dyn Iterator<Item = &Point>> = if area > 0.0 {
            Box::new(dev.iter())
        } else {
            Box::new(dev.iter().rev())
        };
        let mut first = true;
        for p in ordered {
            if first {
                self.out.move_to(*p);
                first = false;
            } else {
                self.out.line_to(*p);
            }
        }
        self.out.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FillRule, rasterize};

    fn line(points: &[(f64, f64)], closed: bool) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Point::new(points[0].0, points[0].1));
        for &(x, y) in &points[1..] {
            b.line_to(Point::new(x, y));
        }
        if closed {
            b.close();
        }
        b.finish()
    }

    fn area(path: &Path, size: usize) -> f64 {
        rasterize(path, FillRule::NonZero, (size, size))
            .map(|c| c.data.iter().map(|&v| f64::from(v) / 255.0).sum())
            .unwrap_or(0.0)
    }

    #[test]
    fn butt_line_area_is_length_times_width() {
        let s = StrokeStyle {
            width: 4.0,
            ..Default::default()
        };
        let p = stroke_to_path(
            &line(&[(10.0, 10.0), (60.0, 10.0)], false),
            &s,
            &Transform::IDENTITY,
        );
        assert!((area(&p, 100) - 200.0).abs() < 1.0);
    }

    #[test]
    fn square_caps_add_half_width_each_end() {
        let s = StrokeStyle {
            width: 4.0,
            cap: LineCap::Square,
            ..Default::default()
        };
        let p = stroke_to_path(
            &line(&[(10.0, 10.0), (60.0, 10.0)], false),
            &s,
            &Transform::IDENTITY,
        );
        assert!((area(&p, 100) - 216.0).abs() < 1.0);
    }

    #[test]
    fn closed_square_with_miter_joins_is_a_frame() {
        let s = StrokeStyle {
            width: 2.0,
            ..Default::default()
        };
        let path = line(
            &[(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)],
            true,
        );
        let p = stroke_to_path(&path, &s, &Transform::IDENTITY);
        // Outer 22 x 22 minus inner 18 x 18.
        assert!(
            (area(&p, 50) - (484.0 - 324.0)).abs() < 1.0,
            "{}",
            area(&p, 50)
        );
    }

    #[test]
    fn sharp_miter_falls_back_to_bevel() {
        let s = StrokeStyle {
            width: 2.0,
            miter_limit: 1.5,
            ..Default::default()
        };
        let path = line(&[(0.0, 0.0), (40.0, 20.0), (0.0, 40.0)], false);
        let mitered = StrokeStyle {
            miter_limit: 100.0,
            ..s.clone()
        };
        let t = Transform::translate(5.0, 5.0);
        assert!(
            area(&stroke_to_path(&path, &s, &t), 60)
                < area(&stroke_to_path(&path, &mitered, &t), 60)
        );
    }

    #[test]
    fn dashes_cover_half_the_line() {
        let s = StrokeStyle {
            width: 2.0,
            dash: vec![5.0, 5.0],
            ..Default::default()
        };
        let p = stroke_to_path(
            &line(&[(0.0, 10.0), (80.0, 10.0)], false),
            &s,
            &Transform::IDENTITY,
        );
        assert!((area(&p, 100) - 80.0).abs() < 1.0);
    }

    #[test]
    fn dash_phase_shifts_the_pattern() {
        assert_eq!(
            dash(
                &[Point::new(0.0, 0.0), Point::new(10.0, 0.0)],
                false,
                &[4.0, 2.0],
                3.0
            )
            .iter()
            .map(|(p, _)| (p[0].x, p[p.len() - 1].x))
            .collect::<Vec<_>>(),
            vec![(0.0, 1.0), (3.0, 7.0), (9.0, 10.0)]
        );
    }

    #[test]
    fn zero_length_dashes_include_both_ends() {
        let dots = dash(
            &[Point::new(0.0, 0.0), Point::new(30.0, 0.0)],
            false,
            &[0.0, 10.0],
            0.0,
        );
        let xs: Vec<f64> = dots.iter().map(|(p, _)| p[0].x).collect();
        assert_eq!(xs, vec![0.0, 10.0, 20.0, 30.0]);
    }

    #[test]
    fn round_cap_dot() {
        let s = StrokeStyle {
            width: 10.0,
            cap: LineCap::Round,
            ..Default::default()
        };
        let p = stroke_to_path(
            &line(&[(20.0, 20.0), (20.0, 20.0)], false),
            &s,
            &Transform::IDENTITY,
        );
        assert!((area(&p, 50) - PI * 25.0).abs() < 1.5);
    }

    #[test]
    fn stroke_follows_anisotropic_transform() {
        let s = StrokeStyle {
            width: 2.0,
            ..Default::default()
        };
        let t = Transform::scale(1.0, 5.0);
        let p = stroke_to_path(&line(&[(10.0, 2.0), (50.0, 2.0)], false), &s, &t);
        // A 2-unit wide horizontal line becomes 10 device pixels tall.
        assert!((area(&p, 60) - 400.0).abs() < 1.0);
    }
}
