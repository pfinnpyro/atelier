//! Changes to the document as a whole: turning pages, by hand or by which
//! way their text runs, and swapping in the document rebuilt after any
//! change.

use std::collections::HashSet;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use atelier_view::{Basis, Layout, PageGeom};

use super::Viewer;

impl Viewer {
    /// Swaps in the document rebuilt from the changes. `pages` look
    /// different now; `turned` says they also changed shape, so their tiles
    /// are dropped and the pages laid out again. Otherwise old tiles stay
    /// up until fresh ones arrive.
    pub(super) fn rebuild(&mut self, pages: &HashSet<usize>, turned: bool) {
        let keep_page = self.current_page();
        let Some(o) = &mut self.open else { return };
        let built = match o.session.build() {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(format!("Could not make the change: {e}"));
                return;
            }
        };
        if let Some(w) = built.warnings.first() {
            self.error = Some(w.to_string());
        }
        if let (Some(fv), Some(form)) = (&mut o.form, built.form) {
            fv.tab = form.tab_order(&built.doc);
            fv.form = form;
        }
        o.doc = built.doc;
        if turned {
            if let Ok(p) = o.doc.pages() {
                let zoom = o.layout.zoom();
                o.layout = Layout::new(p.iter().map(PageGeom::of).collect(), zoom);
            }
            for &p in pages {
                o.tiles.remove_page(p);
                o.thumbs.remove(&p);
            }
            o.stale.retain(|k| !pages.contains(&k.page));
        } else {
            let stale: Vec<_> = pages
                .iter()
                .flat_map(|p| o.tiles.page_tiles(*p).map(|(k, _)| *k).collect::<Vec<_>>())
                .collect();
            o.stale.extend(stale);
            o.thumb_stale.extend(pages.iter().copied());
        }
        o.failed.retain(|k| !pages.contains(&k.page));
        // Work queued for the old document is dropped when it comes back.
        self.generation += 1;
        o.generation = self.generation;
        if turned {
            self.apply_fit();
            self.go_to_page(keep_page);
        }
        self.update_title();
    }

    /// Turns every page a quarter clockwise (`quarters` = 1) or
    /// counterclockwise (-1).
    pub(super) fn rotate_pages(&mut self, quarters: i32) {
        self.finish_editing(true);
        let Some(o) = &mut self.open else { return };
        let count = o.layout.page_count();
        for p in 0..count {
            let r = i32::from(o.session.rotation(p)) + 90 * quarters;
            o.session.rotate(p, r);
        }
        self.rebuild(&(0..count).collect(), true);
    }

    /// Turns one page a quarter clockwise (1) or counterclockwise (-1).
    pub(super) fn rotate_page(&mut self, page: usize, quarters: i32) {
        self.finish_editing(true);
        let Some(o) = &mut self.open else { return };
        let r = i32::from(o.session.rotation(page)) + 90 * quarters;
        o.session.rotate(page, r);
        self.rebuild(&HashSet::from([page]), true);
    }

    /// Starts working out which way up each page reads.
    pub(super) fn auto_orient(&mut self) {
        self.start_orient(false);
    }

    /// Starts auto-orient. A quiet run (when a file opens) shows nothing
    /// unless it turns pages, and its turns do not count as unsaved
    /// changes.
    pub(super) fn start_orient(&mut self, quiet: bool) {
        let Some(o) = &mut self.open else { return };
        if o.orienting.is_some() {
            return;
        }
        let done = Arc::new(AtomicUsize::new(0));
        o.orienting = Some(Orienting {
            done: done.clone(),
            total: o.layout.page_count(),
            quiet,
        });
        let wake = self.ctx.clone();
        self.pool.orient(o.doc.clone(), o.opened, done, move || {
            wake.request_repaint()
        });
        self.orient_progress();
    }

    /// Shows how far auto-orient has got.
    pub(super) fn orient_progress(&mut self) {
        let Some(run) = self.open.as_ref().and_then(|o| o.orienting.as_ref()) else {
            return;
        };
        if run.quiet {
            return;
        }
        let done = run.done.load(Ordering::SeqCst).min(run.total);
        self.notice = Some(format!(
            "Auto-orient: reading page {} of {}…",
            (done + 1).min(run.total),
            run.total
        ));
    }

    /// Turns the pages auto-orient found to be on their side or upside
    /// down.
    pub(super) fn oriented(&mut self, opened: u64, rotations: &[Option<(u16, Basis)>]) {
        let Some(o) = &mut self.open else { return };
        if o.opened != opened {
            return;
        }
        let Some(run) = o.orienting.take() else {
            return;
        };
        let mut turned = HashSet::new();
        let mut from_image = 0;
        for (p, r) in rotations.iter().enumerate() {
            if let Some((r, basis)) = *r
                && r != o.session.rotation(p)
            {
                o.session.rotate(p, i32::from(r));
                turned.insert(p);
                if basis == Basis::Image {
                    from_image += 1;
                }
            }
        }
        if run.quiet {
            o.session.quiet_rotations();
        }
        let unknown = rotations.iter().filter(|r| r.is_none()).count();
        let mut msg = match turned.len() {
            0 => "Auto-orient: every page is already upright.".to_string(),
            1 => "Auto-orient turned 1 page".to_string(),
            n => format!("Auto-orient turned {n} pages"),
        };
        if !turned.is_empty() {
            if from_image > 0 {
                msg.push_str(&format!(" ({from_image} judged from the scanned image)."));
            } else {
                msg.push('.');
            }
        }
        if run.quiet {
            if !turned.is_empty() {
                msg.push_str(" They stay that way in the file only if you save it.");
                self.notice = Some(msg);
            }
        } else {
            if unknown > 0 {
                let pages = if unknown == 1 { "page" } else { "pages" };
                msg.push_str(&format!(
                    " {unknown} {pages} had no text to go by; turn those by hand."
                ));
            }
            self.notice = Some(msg);
        }
        if !turned.is_empty() {
            self.rebuild(&turned, true);
        }
    }
}

/// An auto-orient run: pages done so far, how many there are, and
/// whether it runs quietly.
pub(super) struct Orienting {
    pub done: Arc<AtomicUsize>,
    pub total: usize,
    pub quiet: bool,
}
