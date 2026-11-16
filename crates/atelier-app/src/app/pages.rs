//! Changes to the document as a whole: turning pages, by hand or by which
//! way their text runs, and swapping in the document rebuilt after any
//! change.

use std::collections::{HashMap, HashSet};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use atelier_ocr::models;
use atelier_view::{Basis, Layout, PageGeom};

use super::Viewer;
use crate::settings::Settings;
use crate::worker::OcrRun;

impl Viewer {
    /// Swaps in the document rebuilt from the changes. `pages` look
    /// different now; `turned` says they also changed shape, so their tiles
    /// are dropped and the pages laid out again. Otherwise old tiles stay
    /// up until fresh ones arrive.
    pub(super) fn rebuild(&mut self, pages: &HashSet<usize>, turned: bool) {
        self.try_rebuild(pages, turned);
    }

    /// [`Viewer::rebuild`], saying whether the document could be built.
    /// When it could not, the error is shown and the old document stays.
    pub(super) fn try_rebuild(&mut self, pages: &HashSet<usize>, turned: bool) -> bool {
        let keep_page = self.current_page();
        let Some(o) = &mut self.open else {
            return false;
        };
        o.session.set_revision_author(&self.settings.review_author);
        let built = match o.session.build_tracked(&o.doc) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(format!("Could not make the change: {e}"));
                return false;
            }
        };
        if let Some(w) = built.warnings.first() {
            self.error = Some(w.to_string());
        }
        match (&mut o.form, built.form) {
            (Some(fv), Some(form)) => {
                fv.tab = form.tab_order(&built.doc);
                fv.form = form;
            }
            // The first field added gives the file a form; taking the
            // last one away leaves it without.
            (None, Some(form)) => o.form = Some(super::form::FormView::new(form, &built.doc)),
            (form, None) => *form = None,
        }
        o.doc = built.doc;
        o.safety.refresh(o.doc.clone());
        Self::refresh_script_state(o);
        if turned {
            if let Ok(p) = o.doc.pages() {
                let zoom = o.layout.zoom();
                let facing = o.layout.facing_pages();
                o.layout = Layout::new(p.iter().map(PageGeom::of).collect(), zoom);
                o.layout.set_facing_pages(facing);
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
        true
    }

    /// Turns every page a quarter clockwise (`quarters` = 1) or
    /// counterclockwise (-1).
    pub(super) fn rotate_pages(&mut self, quarters: i32) {
        let count = self.open.as_ref().map_or(0, |o| o.layout.page_count());
        self.rotate_some(&(0..count).collect::<Vec<_>>(), quarters);
    }

    /// Turns some pages a quarter clockwise (1) or counterclockwise (-1).
    pub(super) fn rotate_some(&mut self, pages: &[usize], quarters: i32) {
        self.finish_editing(true);
        let Some(o) = &mut self.open else { return };
        for &p in pages {
            let r = i32::from(o.session.rotation(p)) + 90 * quarters;
            o.session.rotate(p, r);
        }
        self.rebuild(&pages.iter().copied().collect(), true);
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
    /// unless it turns pages or reads scans, and its turns do not count as
    /// unsaved changes. Scans are read with OCR when it is on.
    pub(super) fn start_orient(&mut self, quiet: bool) {
        self.start_run(quiet, true);
    }

    /// Reads the scanned pages with OCR without turning any page.
    pub(super) fn start_ocr(&mut self) {
        self.start_run(true, false);
    }

    /// Whether OCR is on and its models are in place.
    pub(super) fn ocr_ready(&self) -> bool {
        self.settings.ocr && Settings::ocr_models_dir().is_some_and(|d| models::installed(&d))
    }

    pub(super) fn start_run(&mut self, quiet: bool, turn: bool) {
        let ocr = if self.ocr_ready() {
            Settings::ocr_models_dir()
        } else {
            None
        };
        let Some(o) = &mut self.open else { return };
        if o.orienting.is_some() {
            return;
        }
        let done = Arc::new(AtomicUsize::new(0));
        let ocr = ocr.map(|dir| OcrRun {
            dir,
            texts: o.ocr_texts.clone(),
            read: Arc::new(AtomicUsize::new(0)),
        });
        o.orienting = Some(Orienting {
            done: done.clone(),
            total: o.layout.page_count(),
            quiet,
            turn,
            read: ocr.as_ref().map(|r| r.read.clone()),
            turned: HashSet::new(),
            from_image: 0,
            from_ocr: 0,
        });
        let wake = self.ctx.clone();
        self.pool
            .orient(o.doc.clone(), o.opened, done, ocr, move || {
                wake.request_repaint()
            });
        self.orient_progress();
    }

    /// Shows how far auto-orient has got.
    pub(super) fn orient_progress(&mut self) {
        let Some(run) = self.open.as_ref().and_then(|o| o.orienting.as_ref()) else {
            return;
        };
        let done = run.done.load(Ordering::SeqCst).min(run.total);
        let page = (done + 1).min(run.total);
        if run.read_count() > 0 {
            self.notice = Some(format!(
                "Reading scanned pages with OCR: page {page} of {}…",
                run.total
            ));
        } else if !run.quiet {
            self.notice = Some(format!(
                "Auto-orient: reading page {page} of {}…",
                run.total
            ));
        }
    }

    /// Turns pages auto-orient has just judged to be on their side or
    /// upside down, while it goes on reading the rest. The view stays on
    /// what was being read: the same page, as far down it as before.
    pub(super) fn turn_oriented(&mut self, judged: &[(usize, Option<(u16, Basis)>)]) {
        let Some(o) = &mut self.open else { return };
        let Some(run) = &mut o.orienting else { return };
        if !run.turn {
            return;
        }
        let mut now = HashSet::new();
        for &(p, r) in judged {
            if let Some((r, basis)) = r
                && r != o.session.rotation(p)
            {
                o.session.rotate(p, i32::from(r));
                now.insert(p);
                if run.turned.insert(p) {
                    match basis {
                        Basis::Image => run.from_image += 1,
                        Basis::Ocr => run.from_ocr += 1,
                        Basis::Text => {}
                    }
                }
            }
        }
        if now.is_empty() {
            return;
        }
        if run.quiet {
            o.session.quiet_rotations();
        }
        // Where the view is: which page, and how far down it.
        let top = f64::from(self.scroll_to.unwrap_or(self.offset).y);
        let page = o.layout.page_at_y(top + f64::from(self.view_size.y) / 2.0);
        let place = o
            .layout
            .page_rect(page)
            .filter(|r| r.h > 0.0)
            .map(|r| ((top - r.y) / r.h, now.contains(&page)));
        self.rebuild(&now, true);
        // A page that turned under the reader starts again from its top,
        // which is what `rebuild` leaves; otherwise go back to the place.
        if let Some((down, false)) = place
            && let Some(r) = self.open.as_ref().and_then(|o| o.layout.page_rect(page))
        {
            self.scroll_to = Some(eframe::egui::Vec2::new(
                self.offset.x,
                (r.y + down * r.h).max(0.0) as f32,
            ));
        }
    }

    /// Auto-orient has finished: turns whatever is left to turn and says
    /// what was done.
    pub(super) fn oriented(&mut self, opened: u64, rotations: &[Option<(u16, Basis)>]) {
        if self.open.as_ref().is_none_or(|o| o.opened != opened) {
            return;
        }
        // Most pages turned as they were judged; this takes in any that
        // were turned back by hand since, or judged before the run was
        // being watched.
        let all: Vec<_> = rotations.iter().copied().enumerate().collect();
        self.turn_oriented(&all);
        let Some(o) = &mut self.open else { return };
        let Some(run) = o.orienting.take() else {
            return;
        };
        let read = run.read_count();
        let scans = rotations
            .iter()
            .filter(|r| !matches!(r, Some((_, Basis::Text | Basis::Ocr))))
            .count();
        let (turned, from_image, from_ocr) = (&run.turned, run.from_image, run.from_ocr);
        let unknown = rotations.iter().filter(|r| r.is_none()).count();
        let mut msg = match turned.len() {
            0 => "Auto-orient: every page is already upright.".to_string(),
            1 => "Auto-orient turned 1 page".to_string(),
            n => format!("Auto-orient turned {n} pages"),
        };
        if !turned.is_empty() {
            let how: Vec<String> = [
                (from_ocr, "by the words OCR read"),
                (from_image, "by the look of the scan"),
            ]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, how)| format!("{n} {how}"))
            .collect();
            if how.is_empty() {
                msg.push('.');
            } else {
                msg.push_str(&format!(" ({}).", how.join(", ")));
            }
        }
        let read_msg = (read > 0).then(|| {
            let pages = if read == 1 { "page" } else { "pages" };
            format!(
                "Read {read} scanned {pages} with OCR, so their text can be searched and copied."
            )
        });
        if run.quiet {
            let mut parts = Vec::new();
            if !turned.is_empty() {
                parts.push(format!(
                    "{msg} They stay that way in the file only if you save it."
                ));
            }
            parts.extend(read_msg);
            if read == 0 && scans > 0 && self.settings.ocr && !self.ocr_ready() {
                parts.push(
                    "This file has scanned pages. To search their text, get the OCR models \
                     in Settings (☰)."
                        .into(),
                );
            }
            self.notice = (!parts.is_empty()).then(|| parts.join(" "));
        } else {
            if unknown > 0 {
                let pages = if unknown == 1 { "page" } else { "pages" };
                msg.push_str(&format!(
                    " {unknown} {pages} had no text to go by; turn those by hand."
                ));
            }
            if let Some(r) = read_msg {
                msg.push(' ');
                msg.push_str(&r);
            }
            self.notice = Some(msg);
        }
        // A search made while OCR was reading runs again to take it in.
        if read > 0 && !self.search.query.is_empty() {
            self.start_search(self.search.query.clone());
        }
    }
}

/// An auto-orient run: pages done so far, how many there are, whether it
/// runs quietly, whether it turns pages, pages read with OCR so far, and
/// the pages it has turned so far and what each turn went by.
pub(super) struct Orienting {
    pub done: Arc<AtomicUsize>,
    pub total: usize,
    pub quiet: bool,
    pub turn: bool,
    pub read: Option<Arc<AtomicUsize>>,
    turned: HashSet<usize>,
    from_image: usize,
    from_ocr: usize,
}

impl Orienting {
    fn read_count(&self) -> usize {
        self.read.as_ref().map_or(0, |r| r.load(Ordering::SeqCst))
    }
}

/// Page numbers for a file name or message: "3", "3-5" or "1, 4, 7-9".
pub(super) fn page_list_name(pages: &[usize]) -> String {
    let mut sorted: Vec<usize> = pages.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 1 {
            j += 1;
        }
        parts.push(if i == j {
            format!("{}", sorted[i] + 1)
        } else {
            format!("{}-{}", sorted[i] + 1, sorted[j] + 1)
        });
        i = j + 1;
    }
    parts.join(", ")
}

impl Viewer {
    /// Whether pages can be moved, removed or added now.
    fn can_arrange(&mut self) -> bool {
        self.finish_editing(true);
        self.commit_ink();
        self.open.is_some()
    }

    /// Moves `pages` before the page now at `to` (the page count for the
    /// end). Returns where they ended up.
    pub(super) fn move_pages(&mut self, pages: &[usize], to: usize) -> Vec<usize> {
        if !self.can_arrange() {
            return pages.to_vec();
        }
        let Some(o) = &mut self.open else {
            return Vec::new();
        };
        let old = o.session.move_pages(pages, to);
        let moved: Vec<usize> = old
            .iter()
            .enumerate()
            .filter(|(_, o)| pages.contains(o))
            .map(|(n, _)| n)
            .collect();
        if old.iter().enumerate().any(|(n, &o)| n != o) {
            let old: Vec<Option<usize>> = old.into_iter().map(Some).collect();
            self.rearranged(&old, moved.first().copied());
        }
        moved
    }

    /// Removes pages, keeping at least one.
    pub(super) fn delete_pages(&mut self, pages: &[usize]) {
        if pages.is_empty() || !self.can_arrange() {
            return;
        }
        let Some(o) = &mut self.open else { return };
        let Some(old) = o.session.delete_pages(pages) else {
            self.error = Some("A document needs at least one page, so the last ones stay.".into());
            return;
        };
        let keep = pages.iter().min().copied().unwrap_or(0);
        let old: Vec<Option<usize>> = old.into_iter().map(Some).collect();
        self.rearranged(&old, Some(keep.min(old.len() - 1)));
        let n = pages.len();
        self.notice = Some(format!(
            "Removed {n} {}. Save to keep the change; the removed pages are then gone from the \
             file too.",
            if n == 1 { "page" } else { "pages" }
        ));
    }

    /// Inserts the chosen PDF before `at` (the page count means the end).
    /// Errors are returned to the insertion dialog so the source can be corrected.
    pub(super) fn insert_pdf_path(
        &mut self,
        path: &std::path::Path,
        at: usize,
    ) -> Result<(), String> {
        if !self.can_arrange() {
            return Err("Open a PDF before inserting pages.".into());
        }
        let name = super::display_name(path);
        let doc = match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|data| {
                atelier_syntax::Document::open(data).map_err(|e| match e {
                    atelier_syntax::Error::Password => {
                        "it needs a password. Open it, save an unlocked copy, and add that".into()
                    }
                    e => e.to_string(),
                })
            }) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                return Err(format!("Could not insert {name}: {e}."));
            }
        };
        let Some(o) = &mut self.open else {
            return Err("The document was closed.".into());
        };
        match o.session.insert_file(doc, at) {
            Ok(old) => {
                let added = old.iter().filter(|o| o.is_none()).count();
                let first = old.iter().position(Option::is_none);
                let before = self.generation;
                self.rearranged(&old, first);
                if self.generation == before {
                    return Err(self
                        .error
                        .clone()
                        .unwrap_or_else(|| "Could not rebuild the inserted pages.".into()));
                }
                self.notice = Some(format!(
                    "Added {added} {} from {name}.",
                    if added == 1 { "page" } else { "pages" }
                ));
                Ok(())
            }
            Err(e) => Err(format!("Could not insert {name}: {e}.")),
        }
    }

    /// Saves `pages` as a new file, leaving the open document as it is.
    pub(super) fn extract_pages(&mut self, pages: &[usize]) {
        if pages.is_empty() {
            return;
        }
        self.finish_editing(true);
        let Some(o) = &self.open else { return };
        let mut pages = pages.to_vec();
        pages.sort_unstable();
        let bytes = match o.session.extract(&pages) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(format!("Could not take out the pages: {e}"));
                return;
            }
        };
        let stem = o.name.strip_suffix(".pdf").unwrap_or(&o.name);
        let label = if pages.len() == 1 { "page" } else { "pages" };
        let name = format!("{stem} ({label} {}).pdf", page_list_name(&pages));
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_file_name(name)
            .save_file()
        else {
            return;
        };
        match super::write_replacing(&path, &bytes) {
            Ok(()) => {
                self.notice = Some(format!(
                    "Saved {} {label} as {}.",
                    pages.len(),
                    super::display_name(&path)
                ));
            }
            Err(e) => self.error = Some(format!("Could not save {}: {e}", path.display())),
        }
    }

    /// Swaps in the document after pages moved, went or came. `old` gives
    /// each page's index before (`None` for a new one); what the viewer
    /// keeps per page follows its page. Shows `show` afterwards.
    pub(super) fn rearranged(&mut self, old: &[Option<usize>], show: Option<usize>) {
        // Suggested fields name pages by number, which no longer holds.
        self.close_detect();
        let Some(o) = &mut self.open else { return };
        o.session.set_revision_author(&self.settings.review_author);
        let built = match o.session.build_tracked(&o.doc) {
            Ok(b) => b,
            Err(e) => {
                self.error = Some(format!("Could not change the pages: {e}"));
                return;
            }
        };
        if let Some(w) = built.warnings.first() {
            self.error = Some(w.to_string());
        }
        let highlight = o.form.as_ref().is_none_or(|fv| fv.highlight);
        o.form = built.form.map(|form| {
            let mut view = super::form::FormView::new(form, &built.doc);
            view.highlight = highlight;
            view
        });
        o.doc = built.doc;
        o.safety.refresh(o.doc.clone());
        Self::refresh_script_state(o);
        if let Ok(p) = o.doc.pages() {
            let zoom = o.layout.zoom();
            let facing = o.layout.facing_pages();
            o.layout = Layout::new(p.iter().map(PageGeom::of).collect(), zoom);
            o.layout.set_facing_pages(facing);
        }
        fn remap<T>(map: &mut HashMap<usize, T>, old: &[Option<usize>]) {
            let mut was: HashMap<usize, T> = std::mem::take(map);
            for (n, o) in old.iter().enumerate() {
                if let Some(v) = o.and_then(|o| was.remove(&o)) {
                    map.insert(n, v);
                }
            }
        }
        remap(&mut o.thumbs, old);
        remap(&mut o.text, old);
        // Work still running for the old order (auto-orient, OCR, the
        // outline) reports pages by their old place: its results are
        // dropped, and auto-orient starts again on the new order.
        let mut texts = o
            .ocr_texts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        remap(&mut texts, old);
        o.ocr_texts = Arc::new(std::sync::Mutex::new(texts));
        let rerun = o.orienting.take().map(|run| (run.quiet, run.turn));
        let stale = std::mem::take(&mut o.thumb_stale);
        o.thumb_stale = old
            .iter()
            .enumerate()
            .filter(|(_, o)| o.is_some_and(|o| stale.contains(&o)))
            .map(|(n, _)| n)
            .collect();
        o.tiles.clear();
        o.failed.clear();
        o.stale.clear();
        o.outline = None;
        o.outline_asked = false;
        self.generation += 1;
        o.generation = self.generation;
        o.opened = self.generation;
        self.selection = None;
        self.colors.reset();
        self.sidebar.selected.clear();
        if !self.search.query.is_empty() {
            self.start_search(self.search.query.clone());
        }
        self.apply_fit();
        if let Some(p) = show {
            self.go_to_page(p);
        }
        self.update_title();
        if let Some((quiet, turn)) = rerun {
            self.start_run(quiet, turn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::page_list_name;

    #[test]
    fn page_lists_join_runs() {
        assert_eq!(page_list_name(&[0]), "1");
        assert_eq!(page_list_name(&[2, 0, 1, 5, 7, 8]), "1-3, 6, 8-9");
    }
}

#[cfg(test)]
mod orient_tests {
    use super::*;

    /// Three pages of different shapes.
    const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >> endobj\n4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n5 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj\n%%EOF";

    fn viewer() -> Viewer {
        let ctx = eframe::egui::Context::default();
        let mut viewer = Viewer::initialize(
            &eframe::CreationContext::_new_kittest(ctx),
            Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Default::default()
            },
        );
        viewer.open_with_password(PDF.to_vec(), "orient-test.pdf".into(), "", None);
        assert!(viewer.open.is_some());
        viewer
    }

    /// A run under way, as `start_run` leaves it, without its threads.
    fn begin(viewer: &mut Viewer, quiet: bool, turn: bool) {
        let o = viewer.open.as_mut().unwrap();
        o.orienting = Some(Orienting {
            done: Arc::new(AtomicUsize::new(0)),
            total: 3,
            quiet,
            turn,
            read: None,
            turned: HashSet::new(),
            from_image: 0,
            from_ocr: 0,
        });
    }

    fn rotations(viewer: &Viewer) -> Vec<u16> {
        let o = viewer.open.as_ref().unwrap();
        (0..3).map(|p| o.session.rotation(p)).collect()
    }

    #[test]
    fn pages_turn_as_they_are_judged_not_at_the_end() {
        let mut viewer = viewer();
        begin(&mut viewer, false, true);
        let width = |v: &Viewer, p| v.open.as_ref().unwrap().layout.geom(p).unwrap().size().0;
        let (first, last) = (width(&viewer, 0), width(&viewer, 2));

        // The first page is judged sideways: it turns at once, and is laid
        // out again, while the run is still going.
        viewer.turn_oriented(&[(0, Some((90, Basis::Image)))]);
        assert_eq!(rotations(&viewer), [90, 0, 0]);
        assert!(viewer.open.as_ref().unwrap().orienting.is_some());
        assert_ne!(width(&viewer, 0), first);
        assert_eq!(width(&viewer, 2), last);

        // An upright page and one with nothing to go by change nothing.
        let generation = viewer.open.as_ref().unwrap().generation;
        viewer.turn_oriented(&[(1, Some((0, Basis::Text))), (2, None)]);
        assert_eq!(rotations(&viewer), [90, 0, 0]);
        assert_eq!(viewer.open.as_ref().unwrap().generation, generation);

        // Two judged in the same frame turn in one rebuild.
        viewer.turn_oriented(&[(1, Some((180, Basis::Ocr))), (2, Some((270, Basis::Text)))]);
        assert_eq!(rotations(&viewer), [90, 180, 270]);
        assert_eq!(viewer.open.as_ref().unwrap().generation, generation + 1);

        // The end of the run turns nothing more and counts every turn once.
        let opened = viewer.open.as_ref().unwrap().opened;
        viewer.oriented(
            opened,
            &[
                Some((90, Basis::Image)),
                Some((180, Basis::Ocr)),
                Some((270, Basis::Text)),
            ],
        );
        assert_eq!(rotations(&viewer), [90, 180, 270]);
        assert_eq!(viewer.open.as_ref().unwrap().generation, generation + 1);
        assert!(viewer.open.as_ref().unwrap().orienting.is_none());
        assert_eq!(
            viewer.notice.as_deref(),
            Some(
                "Auto-orient turned 3 pages (1 by the words OCR read, 1 by the look of the scan)."
            )
        );
        assert!(viewer.is_dirty());
    }

    #[test]
    fn a_quiet_run_turns_pages_without_leaving_unsaved_changes() {
        let mut viewer = viewer();
        begin(&mut viewer, true, true);
        viewer.turn_oriented(&[(1, Some((90, Basis::Text)))]);
        assert_eq!(rotations(&viewer), [0, 90, 0]);
        assert!(!viewer.is_dirty());
        let opened = viewer.open.as_ref().unwrap().opened;
        viewer.oriented(opened, &[None, Some((90, Basis::Text)), None]);
        assert!(!viewer.is_dirty());
        assert_eq!(
            viewer.notice.as_deref(),
            Some("Auto-orient turned 1 page. They stay that way in the file only if you save it.")
        );
    }

    #[test]
    fn a_run_that_only_reads_turns_nothing_and_stale_results_are_ignored() {
        let mut viewer = viewer();
        // OCR without turning.
        begin(&mut viewer, true, false);
        viewer.turn_oriented(&[(0, Some((90, Basis::Ocr)))]);
        assert_eq!(rotations(&viewer), [0, 0, 0]);
        // Results for another opening of the file change nothing.
        let opened = viewer.open.as_ref().unwrap().opened;
        viewer.oriented(opened + 1, &[Some((90, Basis::Text)), None, None]);
        assert!(viewer.open.as_ref().unwrap().orienting.is_some());
        // With no run under way, nothing turns either.
        viewer.open.as_mut().unwrap().orienting = None;
        viewer.turn_oriented(&[(0, Some((90, Basis::Text)))]);
        assert_eq!(rotations(&viewer), [0, 0, 0]);
    }

    #[test]
    fn the_view_keeps_its_place_when_pages_above_it_turn() {
        let mut viewer = viewer();
        begin(&mut viewer, false, true);
        // Looking a little way down the last page.
        let before = viewer.open.as_ref().unwrap().layout.page_rect(2).unwrap();
        viewer.view_size = eframe::egui::Vec2::new(400.0, 50.0);
        viewer.offset = eframe::egui::Vec2::new(0.0, (before.y + before.h * 0.25) as f32);
        viewer.scroll_to = None;
        viewer.turn_oriented(&[(0, Some((90, Basis::Text)))]);
        let after = viewer.open.as_ref().unwrap().layout.page_rect(2).unwrap();
        assert_ne!(after.y, before.y);
        let to = f64::from(viewer.scroll_to.unwrap().y);
        assert!(
            (to - (after.y + after.h * 0.25)).abs() < 1.0,
            "{to} {after:?}"
        );
    }
}
