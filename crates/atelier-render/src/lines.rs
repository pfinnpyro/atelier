//! The straight lines and boxes a page draws, for finding tables and the
//! blanks of a form. Curves, text and clipping are left out.

use atelier_raster::{Color, Path, Point, Verb};

/// More lines than this on one page are ignored.
const MAX_LINES: usize = 60_000;
/// Filled shapes thinner than this (in points) are lines drawn as boxes.
const THIN: f64 = 3.5;

/// One straight line, in default user space (PDF points, y up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageLine {
    pub from: Point,
    pub to: Point,
    /// How thick it is drawn; 0 for the edge of a filled box.
    pub width: f64,
}

/// What a page draws besides text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageGraphics {
    /// Stroked lines, thin filled bars and the edges of pale boxes.
    pub lines: Vec<PageLine>,
    /// Where each image sits: the corners of its unit square.
    pub images: Vec<[Point; 4]>,
}

impl PageGraphics {
    fn push(&mut self, from: Point, to: Point, width: f64) {
        let finite = [from.x, from.y, to.x, to.y].iter().all(|v| v.is_finite());
        if finite && self.lines.len() < MAX_LINES && (to - from).length() > 0.05 {
            self.lines.push(PageLine { from, to, width });
        }
    }

    /// Records a stroked path, already in user space.
    pub(crate) fn stroke(&mut self, path: &Path, width: f64, color: Option<Color>) {
        if !color.is_some_and(shows) {
            return;
        }
        let mut start = Point::default();
        let mut at = Point::default();
        for verb in &path.verbs {
            match *verb {
                Verb::MoveTo(p) => {
                    start = p;
                    at = p;
                }
                Verb::LineTo(p) => {
                    self.push(at, p, width);
                    at = p;
                }
                Verb::CubicTo(_, _, p) => at = p,
                Verb::Close => {
                    self.push(at, start, width);
                    at = start;
                }
            }
        }
    }

    /// Records a filled path, already in user space: thin bars as lines,
    /// pale rectangles by their edges.
    pub(crate) fn fill(&mut self, path: &Path, color: Option<Color>) {
        let Some(color) = color.filter(|c| shows(*c)) else {
            return;
        };
        let mut sub: Vec<Point> = Vec::new();
        let mut curved = false;
        let flush = |sub: &mut Vec<Point>, curved: &mut bool, out: &mut Self| {
            if !*curved {
                out.fill_subpath(sub, color);
            }
            sub.clear();
            *curved = false;
        };
        for verb in &path.verbs {
            match *verb {
                Verb::MoveTo(p) => {
                    flush(&mut sub, &mut curved, self);
                    sub.push(p);
                }
                Verb::LineTo(p) => sub.push(p),
                Verb::CubicTo(..) => curved = true,
                Verb::Close => {}
            }
        }
        flush(&mut sub, &mut curved, self);
    }

    fn fill_subpath(&mut self, points: &[Point], color: Color) {
        let mut corners: Vec<Point> = Vec::new();
        for &p in points {
            if corners.last().is_none_or(|q| (p - *q).length() > 0.01) {
                corners.push(p);
            }
        }
        if corners.len() > 1 && (corners[0] - corners[corners.len() - 1]).length() <= 0.01 {
            corners.pop();
        }
        if corners.len() != 4 {
            return;
        }
        // A rectangle: each side runs along x or y.
        let straight = (0..4).all(|i| {
            let d = corners[(i + 1) % 4] - corners[i];
            d.x.abs() <= 0.05 || d.y.abs() <= 0.05
        });
        if !straight {
            return;
        }
        let x0 = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let x1 = corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let y0 = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let y1 = corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let (w, h) = (x1 - x0, y1 - y0);
        if h <= THIN && w > h {
            let y = (y0 + y1) / 2.0;
            self.push(Point::new(x0, y), Point::new(x1, y), h);
        } else if w <= THIN && h > w {
            let x = (x0 + x1) / 2.0;
            self.push(Point::new(x, y0), Point::new(x, y1), w);
        } else if color.r.min(color.g).min(color.b) >= 0.55 {
            // A pale box, as forms draw behind their blanks. Dark and
            // strongly coloured blocks are decoration.
            for i in 0..4 {
                self.push(corners[i], corners[(i + 1) % 4], 0.0);
            }
        }
    }
}

/// Whether a paint can be seen on a white page.
fn shows(c: Color) -> bool {
    c.a > 0.1 && c.r.min(c.g).min(c.b) < 0.93
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_fills_are_lines_and_tinted_boxes_give_their_edges() {
        let mut g = PageGraphics::default();
        g.fill(&Path::rect(10.0, 50.0, 100.0, 0.5), Some(Color::BLACK));
        assert_eq!(g.lines.len(), 1);
        assert_eq!(g.lines[0].from.y, 50.25);
        g.fill(
            &Path::rect(10.0, 10.0, 80.0, 20.0),
            Some(Color::rgb(0.9, 0.9, 0.8)),
        );
        assert_eq!(g.lines.len(), 5);
        // White, dark and strongly coloured boxes are not blanks.
        g.fill(&Path::rect(10.0, 10.0, 80.0, 20.0), Some(Color::WHITE));
        g.fill(&Path::rect(10.0, 10.0, 80.0, 20.0), Some(Color::BLACK));
        g.fill(
            &Path::rect(10.0, 10.0, 80.0, 20.0),
            Some(Color::rgb(1.0, 0.9, 0.0)),
        );
        assert_eq!(g.lines.len(), 5);
    }

    #[test]
    fn stroked_boxes_give_four_lines() {
        let mut g = PageGraphics::default();
        g.stroke(&Path::rect(0.0, 0.0, 10.0, 10.0), 1.0, Some(Color::BLACK));
        assert_eq!(g.lines.len(), 4);
        g.stroke(&Path::rect(0.0, 0.0, 10.0, 10.0), 1.0, Some(Color::WHITE));
        assert_eq!(g.lines.len(), 4);
    }
}
