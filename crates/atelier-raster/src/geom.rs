use std::ops::{Add, Mul, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn normalize(self) -> Self {
        let len = self.length();
        if len > 0.0 {
            Self::new(self.x / len, self.y / len)
        } else {
            Self::default()
        }
    }

    /// Rotated 90 degrees counter-clockwise (in y-up coordinates).
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }

    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }

    pub fn lerp(self, o: Self, t: f64) -> Self {
        self + (o - self) * t
    }
}

impl Add for Point {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Point {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Point {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

/// An affine transform `[a b c d e f]` as PDF writes it: it maps (x, y) to
/// (a x + c y + e, b x + d y + f).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    pub fn from_array(v: [f64; 6]) -> Self {
        Self::new(v[0], v[1], v[2], v[3], v[4], v[5])
    }

    pub const fn translate(x: f64, y: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// `self` applied first, then `other`. In PDF terms, `cm` with matrix M
    /// sets CTM to `M.then(CTM)`.
    pub fn then(&self, o: &Self) -> Self {
        Self::new(
            self.a * o.a + self.b * o.c,
            self.a * o.b + self.b * o.d,
            self.c * o.a + self.d * o.c,
            self.c * o.b + self.d * o.d,
            self.e * o.a + self.f * o.c + o.e,
            self.e * o.b + self.f * o.d + o.f,
        )
    }

    pub fn apply(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// Applies only the linear part (for directions and offsets).
    pub fn apply_vector(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y, self.b * p.x + self.d * p.y)
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    pub fn invert(&self) -> Option<Self> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Self::new(
            self.d * inv,
            -self.b * inv,
            -self.c * inv,
            self.a * inv,
            (self.c * self.f - self.d * self.e) * inv,
            (self.b * self.e - self.a * self.f) * inv,
        ))
    }

    /// The largest factor by which the transform stretches any direction.
    pub fn max_scale(&self) -> f64 {
        let (a, b, c, d) = (self.a, self.b, self.c, self.d);
        let s1 = a * a + b * b + c * c + d * d;
        let s2 = ((a * a + b * b - c * c - d * d).powi(2) + 4.0 * (a * c + b * d).powi(2)).sqrt();
        ((s1 + s2) / 2.0).sqrt()
    }

    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .iter()
            .all(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn then_applies_left_first() {
        let t = Transform::scale(2.0, 2.0).then(&Transform::translate(10.0, 0.0));
        assert_eq!(t.apply(Point::new(1.0, 1.0)), Point::new(12.0, 2.0));
    }

    #[test]
    fn invert_round_trips() {
        let t = Transform::new(2.0, 1.0, -1.0, 3.0, 5.0, 7.0);
        let p = Point::new(3.0, -4.0);
        let back = t.invert().unwrap().apply(t.apply(p));
        assert!((back - p).length() < 1e-9);
    }

    #[test]
    fn max_scale_of_rotation_and_scale() {
        let t = Transform::new(0.0, 3.0, -2.0, 0.0, 0.0, 0.0);
        assert!((t.max_scale() - 3.0).abs() < 1e-9);
    }
}
