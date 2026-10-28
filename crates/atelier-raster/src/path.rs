use crate::{Point, Transform};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verb {
    MoveTo(Point),
    LineTo(Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// A path made of subpaths of lines and cubic Béziers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub verbs: Vec<Verb>,
}

impl Path {
    pub fn is_empty(&self) -> bool {
        self.verbs.is_empty()
    }

    pub fn transform(&self, t: &Transform) -> Path {
        let verbs = self
            .verbs
            .iter()
            .map(|v| match *v {
                Verb::MoveTo(p) => Verb::MoveTo(t.apply(p)),
                Verb::LineTo(p) => Verb::LineTo(t.apply(p)),
                Verb::CubicTo(a, b, c) => Verb::CubicTo(t.apply(a), t.apply(b), t.apply(c)),
                Verb::Close => Verb::Close,
            })
            .collect();
        Path { verbs }
    }

    /// A rectangle as PDF's `re` operator builds it.
    pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Path {
        let mut b = PathBuilder::new();
        b.move_to(Point::new(x, y));
        b.line_to(Point::new(x + w, y));
        b.line_to(Point::new(x + w, y + h));
        b.line_to(Point::new(x, y + h));
        b.close();
        b.finish()
    }

    /// Converts curves to line segments. Each polyline is one subpath; the flag
    /// says whether it was explicitly closed. `tolerance` is the largest allowed
    /// distance between the curve and its polyline, in path units.
    pub fn flatten(&self, tolerance: f64) -> Vec<(Vec<Point>, bool)> {
        let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
        let mut current: Vec<Point> = Vec::new();
        let mut start = Point::default();
        for verb in &self.verbs {
            match *verb {
                Verb::MoveTo(p) => {
                    if !current.is_empty() {
                        out.push((std::mem::take(&mut current), false));
                    }
                    start = p;
                    current.push(p);
                }
                Verb::LineTo(p) => {
                    if current.is_empty() {
                        current.push(start);
                    }
                    current.push(p);
                }
                Verb::CubicTo(c1, c2, p) => {
                    if current.is_empty() {
                        current.push(start);
                    }
                    let p0 = *current.last().unwrap_or(&start);
                    flatten_cubic(p0, c1, c2, p, tolerance, &mut current);
                }
                Verb::Close => {
                    if !current.is_empty() {
                        out.push((std::mem::take(&mut current), true));
                    }
                    // A later segment without moveto starts at the subpath start.
                    current.clear();
                }
            }
        }
        if !current.is_empty() {
            out.push((current, false));
        }
        out
    }

    /// The bounding box of the control points: (min, max).
    pub fn bounds(&self) -> Option<(Point, Point)> {
        let mut min = Point::new(f64::INFINITY, f64::INFINITY);
        let mut max = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut add = |p: Point| {
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        };
        for v in &self.verbs {
            match *v {
                Verb::MoveTo(p) | Verb::LineTo(p) => add(p),
                Verb::CubicTo(a, b, c) => {
                    add(a);
                    add(b);
                    add(c);
                }
                Verb::Close => {}
            }
        }
        (min.x <= max.x).then_some((min, max))
    }
}

fn flatten_cubic(p0: Point, p1: Point, p2: Point, p3: Point, tol: f64, out: &mut Vec<Point>) {
    // With L the largest second difference of the control points, n segments
    // keep the error under 3L / (4 n^2).
    let dd = (p0 - p1 * 2.0 + p2)
        .length()
        .max((p1 - p2 * 2.0 + p3).length());
    let n = ((3.0 * dd / (4.0 * tol.max(1e-6))).sqrt().ceil() as usize).clamp(1, 1000);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let mt = 1.0 - t;
        let p = p0 * (mt * mt * mt)
            + p1 * (3.0 * mt * mt * t)
            + p2 * (3.0 * mt * t * t)
            + p3 * (t * t * t);
        out.push(p);
    }
}

/// Builds paths with the current-point rules of PDF path construction.
#[derive(Debug, Default)]
pub struct PathBuilder {
    path: Path,
    current: Option<Point>,
    start: Point,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_point(&self) -> Option<Point> {
        self.current
    }

    pub fn move_to(&mut self, p: Point) {
        self.path.verbs.push(Verb::MoveTo(p));
        self.current = Some(p);
        self.start = p;
    }

    pub fn line_to(&mut self, p: Point) {
        if self.current.is_none() {
            self.move_to(p);
            return;
        }
        self.path.verbs.push(Verb::LineTo(p));
        self.current = Some(p);
    }

    pub fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) {
        if self.current.is_none() {
            self.move_to(c1);
        }
        self.path.verbs.push(Verb::CubicTo(c1, c2, p));
        self.current = Some(p);
    }

    pub fn close(&mut self) {
        if self.current.is_some() {
            self.path.verbs.push(Verb::Close);
            self.current = Some(self.start);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    pub fn finish(&mut self) -> Path {
        self.current = None;
        std::mem::take(&mut self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattened_circle_stays_within_tolerance() {
        // Quarter circle of radius 100 with the usual kappa control points.
        let k = 0.552_284_749_8 * 100.0;
        let mut b = PathBuilder::new();
        b.move_to(Point::new(100.0, 0.0));
        b.cubic_to(
            Point::new(100.0, k),
            Point::new(k, 100.0),
            Point::new(0.0, 100.0),
        );
        let lines = b.finish().flatten(0.1);
        let pts = &lines[0].0;
        assert!(pts.len() > 4);
        for w in pts.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            assert!((mid.length() - 100.0).abs() < 0.15, "{}", mid.length());
        }
    }

    #[test]
    fn close_returns_to_start() {
        let p = Path::rect(0.0, 0.0, 10.0, 10.0);
        let lines = p.flatten(0.1);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].1);
        assert_eq!(lines[0].0.len(), 4);
    }
}
