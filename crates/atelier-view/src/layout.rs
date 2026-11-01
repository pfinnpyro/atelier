use atelier_raster::{Point, Transform};
use atelier_syntax::{Page, Rect};

/// An axis-aligned rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl ViewRect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn intersect(&self, o: &ViewRect) -> Option<ViewRect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x0 < x1 && y0 < y1).then(|| ViewRect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// The parts of a page that decide where it sits and how it is turned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeom {
    pub crop: Rect,
    /// 0, 90, 180 or 270.
    pub rotate: u16,
}

impl PageGeom {
    pub fn of(page: &Page) -> Self {
        Self {
            crop: page.crop_box,
            rotate: page.rotate,
        }
    }

    /// Width and height in points as shown, after /Rotate.
    pub fn size(&self) -> (f64, f64) {
        let (w, h) = (self.crop.width(), self.crop.height());
        if self.rotate % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// User space to page coordinates at `zoom` logical pixels per point.
    /// This matches `atelier_render::page_transform` without its rounding.
    pub fn to_page(&self, zoom: f64) -> Transform {
        let c = self.crop;
        let (w, h) = (c.width() * zoom, c.height() * zoom);
        let base = Transform::new(zoom, 0.0, 0.0, -zoom, -c.x0 * zoom, c.y1 * zoom);
        let rot = match self.rotate {
            90 => Transform::new(0.0, 1.0, -1.0, 0.0, h, 0.0),
            180 => Transform::new(-1.0, 0.0, 0.0, -1.0, w, h),
            270 => Transform::new(0.0, -1.0, 1.0, 0.0, 0.0, w),
            _ => Transform::IDENTITY,
        };
        base.then(&rot)
    }
}

/// Pages stacked top to bottom, each centred across the widest page.
#[derive(Debug, Clone)]
pub struct Layout {
    pages: Vec<PageGeom>,
    zoom: f64,
    facing: bool,
    gap: f64,
    rects: Vec<ViewRect>,
    width: f64,
    height: f64,
}

impl Layout {
    /// Space between pages and around them, in logical pixels.
    pub const GAP: f64 = 12.0;

    pub fn new(pages: Vec<PageGeom>, zoom: f64) -> Self {
        let mut l = Self {
            pages,
            zoom,
            facing: false,
            gap: Self::GAP,
            rects: Vec::new(),
            width: 0.0,
            height: 0.0,
        };
        l.relayout();
        l
    }

    fn relayout(&mut self) {
        let z = self.zoom;
        if self.facing {
            let widths: Vec<f64> = self
                .pages
                .chunks(2)
                .map(|row| {
                    row.iter().map(|p| p.size().0 * z).sum::<f64>()
                        + if row.len() == 2 { self.gap } else { 0.0 }
                })
                .collect();
            let widest = widths.iter().copied().fold(0.0, f64::max);
            self.width = widest + 2.0 * self.gap;
            self.rects.clear();
            let mut y = self.gap;
            for (row, width) in self.pages.chunks(2).zip(widths) {
                let height = row.iter().map(|p| p.size().1 * z).fold(0.0, f64::max);
                let mut x = self.gap + (widest - width) / 2.0;
                for page in row {
                    let (w, h) = page.size();
                    self.rects.push(ViewRect::new(x, y, w * z, h * z));
                    x += w * z + self.gap;
                }
                y += height + self.gap;
            }
            self.height = y;
            return;
        }
        let widest = self
            .pages
            .iter()
            .map(|p| p.size().0 * z)
            .fold(0.0, f64::max);
        self.width = widest + 2.0 * self.gap;
        let mut y = self.gap;
        self.rects = self
            .pages
            .iter()
            .map(|p| {
                let (w, h) = p.size();
                let r = ViewRect::new(self.gap + (widest - w * z) / 2.0, y, w * z, h * z);
                y += h * z + self.gap;
                r
            })
            .collect();
        self.height = y;
    }

    pub fn facing_pages(&self) -> bool {
        self.facing
    }

    pub fn set_facing_pages(&mut self, facing: bool) {
        if self.facing != facing {
            self.facing = facing;
            self.relayout();
        }
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    pub fn set_zoom(&mut self, zoom: f64) {
        if zoom.is_finite() && zoom > 0.0 && zoom != self.zoom {
            self.zoom = zoom;
            self.relayout();
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn geom(&self, page: usize) -> Option<&PageGeom> {
        self.pages.get(page)
    }

    pub fn page_rect(&self, page: usize) -> Option<ViewRect> {
        self.rects.get(page).copied()
    }

    /// The whole document's size in logical pixels.
    pub fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// Pages that overlap the rows `top..bottom`, in order.
    pub fn visible_pages(&self, top: f64, bottom: f64) -> std::ops::Range<usize> {
        let start = if self.facing {
            self.rects
                .chunks(2)
                .take_while(|row| row.iter().all(|r| r.bottom() <= top))
                .map(<[_]>::len)
                .sum()
        } else {
            self.rects.partition_point(|r| r.bottom() <= top)
        };
        let end = self.rects.partition_point(|r| r.y < bottom);
        start..end.max(start)
    }

    /// The page whose slot (the page plus half the gap around it) holds row
    /// `y`; used for "page n of N" and for paging.
    pub fn page_at_y(&self, y: f64) -> usize {
        let i = if self.facing {
            self.rects
                .chunks(2)
                .take_while(|row| {
                    row.iter().map(ViewRect::bottom).fold(0.0, f64::max) + self.gap / 2.0 <= y
                })
                .map(<[_]>::len)
                .sum()
        } else {
            self.rects
                .partition_point(|r| r.bottom() + self.gap / 2.0 <= y)
        };
        i.min(self.rects.len().saturating_sub(1))
    }

    /// The page under a layout point and the point in that page's
    /// coordinates.
    pub fn page_at(&self, x: f64, y: f64) -> Option<(usize, f64, f64)> {
        self.visible_pages(y, y + 1e-9).find_map(|i| {
            let r = self.rects.get(i)?;
            r.contains(x, y).then_some((i, x - r.x, y - r.y))
        })
    }

    /// A layout point turned into the user space of `page`, even when it
    /// lies outside the page (for dragging a selection past an edge).
    pub fn to_user(&self, page: usize, x: f64, y: f64) -> Option<Point> {
        let r = self.rects.get(page)?;
        let inv = self.pages[page].to_page(self.zoom).invert()?;
        Some(inv.apply(Point::new(x - r.x, y - r.y)))
    }

    /// User space of `page` to layout coordinates.
    pub fn user_to_layout(&self, page: usize) -> Option<Transform> {
        let r = self.rects.get(page)?;
        Some(
            self.pages[page]
                .to_page(self.zoom)
                .then(&Transform::translate(r.x, r.y)),
        )
    }

    /// The zoom at which the widest page fills `width` logical pixels.
    pub fn fit_width_zoom(&self, width: f64) -> f64 {
        if self.facing {
            let widest = self
                .pages
                .chunks(2)
                .map(|row| row.iter().map(|p| p.size().0).sum::<f64>())
                .fold(0.0, f64::max);
            return if widest > 0.0 {
                ((width - 3.0 * self.gap) / widest).max(0.01)
            } else {
                self.zoom
            };
        }
        let widest = self.pages.iter().map(|p| p.size().0).fold(0.0, f64::max);
        if widest <= 0.0 {
            return self.zoom;
        }
        ((width - 2.0 * self.gap) / widest).max(0.01)
    }

    /// The zoom at which `page` fits inside `width` by `height`.
    pub fn fit_page_zoom(&self, page: usize, width: f64, height: f64) -> f64 {
        if self.facing && page < self.pages.len() {
            let start = page / 2 * 2;
            let row = &self.pages[start..(start + 2).min(self.pages.len())];
            let w = row.iter().map(|p| p.size().0).sum::<f64>();
            let h = row.iter().map(|p| p.size().1).fold(0.0, f64::max);
            if w > 0.0 && h > 0.0 {
                return ((width - self.gap * (row.len() as f64 + 1.0)) / w)
                    .min((height - 2.0 * self.gap) / h)
                    .max(0.01);
            }
        }
        let Some(p) = self.pages.get(page) else {
            return self.zoom;
        };
        let (w, h) = p.size();
        if w <= 0.0 || h <= 0.0 {
            return self.zoom;
        }
        ((width - 2.0 * self.gap) / w)
            .min((height - 2.0 * self.gap) / h)
            .max(0.01)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(w: f64, h: f64, rotate: u16) -> PageGeom {
        PageGeom {
            crop: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: w,
                y1: h,
            },
            rotate,
        }
    }

    #[test]
    fn stacks_and_centres_pages() {
        let l = Layout::new(vec![geom(100.0, 200.0, 0), geom(200.0, 100.0, 0)], 2.0);
        let g = Layout::GAP;
        assert_eq!(l.size(), (400.0 + 2.0 * g, 3.0 * g + 600.0));
        assert_eq!(
            l.page_rect(0),
            Some(ViewRect::new(g + 100.0, g, 200.0, 400.0))
        );
        assert_eq!(
            l.page_rect(1),
            Some(ViewRect::new(g, 2.0 * g + 400.0, 400.0, 200.0))
        );
    }

    #[test]
    fn rotation_swaps_size() {
        let l = Layout::new(vec![geom(100.0, 200.0, 90)], 1.0);
        let r = l.page_rect(0).unwrap();
        assert_eq!((r.w, r.h), (200.0, 100.0));
        // The top left of the shown page is the bottom left of the crop box.
        let t = l.user_to_layout(0).unwrap();
        let p = t.apply(Point::new(0.0, 0.0));
        assert!((p.x - r.x).abs() < 1e-9 && (p.y - r.y).abs() < 1e-9);
    }

    #[test]
    fn finds_visible_pages() {
        let l = Layout::new(vec![geom(100.0, 100.0, 0); 10], 1.0);
        let g = Layout::GAP;
        // Page i spans g + i * (100 + g) .. + 100.
        assert_eq!(l.visible_pages(0.0, 5.0), 0..0);
        assert_eq!(l.visible_pages(0.0, g + 1.0), 0..1);
        assert_eq!(l.visible_pages(g + 150.0, g + 250.0), 1..3);
        assert_eq!(l.visible_pages(1e9, 2e9), 10..10);
        assert_eq!(l.page_at_y(g + 105.0), 0);
        assert_eq!(l.page_at_y(g + 110.0), 1);
        assert_eq!(l.page_at_y(1e9), 9);
        let (i, x, y) = l.page_at(g + 10.0, 2.0 * g + 120.0).unwrap();
        assert_eq!(i, 1);
        assert!((x - 10.0).abs() < 1e-9 && (y - 20.0).abs() < 1e-9);
        assert!(l.page_at(1.0, 1.0).is_none());
    }

    #[test]
    fn user_space_round_trip() {
        let mut l = Layout::new(vec![geom(612.0, 792.0, 0), geom(612.0, 792.0, 270)], 1.0);
        l.set_zoom(1.5);
        for page in 0..2 {
            let t = l.user_to_layout(page).unwrap();
            let p = t.apply(Point::new(100.0, 700.0));
            let back = l.to_user(page, p.x, p.y).unwrap();
            assert!((back.x - 100.0).abs() < 1e-9 && (back.y - 700.0).abs() < 1e-9);
        }
    }

    #[test]
    fn fit_zooms() {
        let l = Layout::new(vec![geom(600.0, 800.0, 0)], 1.0);
        let g = Layout::GAP;
        assert!((l.fit_width_zoom(1200.0 + 2.0 * g) - 2.0).abs() < 1e-9);
        assert!((l.fit_page_zoom(0, 2000.0, 400.0 + 2.0 * g) - 0.5).abs() < 1e-9);
    }
    #[test]
    fn facing_pages_hit_test_both_pages_and_handle_mixed_heights() {
        let mut layout = Layout::new(
            vec![
                geom(200.0, 100.0, 0),
                geom(100.0, 300.0, 90),
                geom(150.0, 200.0, 0),
            ],
            1.0,
        );
        layout.set_facing_pages(true);
        let left = layout.page_rect(0).unwrap();
        let right = layout.page_rect(1).unwrap();
        let last = layout.page_rect(2).unwrap();
        assert_eq!(left.y, right.y);
        assert!(right.x > left.right());
        assert!(last.y > left.bottom().max(right.bottom()));
        for (page, rect) in [(0, left), (1, right), (2, last)] {
            assert_eq!(
                layout.page_at(rect.x + 10.0, rect.y + 10.0).unwrap().0,
                page
            );
            let user = layout.to_user(page, rect.x + 10.0, rect.y + 10.0).unwrap();
            let back = layout.user_to_layout(page).unwrap().apply(user);
            assert!((back.x - (rect.x + 10.0)).abs() < 1e-8);
            assert!((back.y - (rect.y + 10.0)).abs() < 1e-8);
        }
        assert!(
            layout
                .page_at((left.right() + right.x) / 2.0, left.y + 10.0)
                .is_none()
        );
        assert_eq!(layout.visible_pages(left.y, left.y + 10.0), 0..2);
        assert_eq!(layout.page_at_y(last.y + 10.0), 2);
        layout.set_zoom(layout.fit_width_zoom(800.0));
        assert!((layout.size().0 - 800.0).abs() < 1e-8);
        layout.set_facing_pages(false);
        assert!(layout.page_rect(1).unwrap().y > layout.page_rect(0).unwrap().bottom());
    }

    #[test]
    fn facing_rows_keep_tall_right_pages_visible() {
        let mut layout = Layout::new(
            vec![
                geom(200.0, 100.0, 0),
                geom(200.0, 500.0, 0),
                geom(200.0, 100.0, 0),
            ],
            1.0,
        );
        layout.set_facing_pages(true);
        let right = layout.page_rect(1).unwrap();
        assert_eq!(layout.visible_pages(300.0, 400.0), 0..2);
        assert_eq!(layout.page_at(right.x + 10.0, 350.0).unwrap().0, 1);
        assert_eq!(layout.page_at_y(350.0), 0);
        let zoom = layout.fit_page_zoom(1, 800.0, 600.0);
        layout.set_zoom(zoom);
        assert!(layout.page_rect(1).unwrap().bottom() < 600.0);
        assert!(layout.size().0 <= 800.0 + 1e-8);
    }
}
