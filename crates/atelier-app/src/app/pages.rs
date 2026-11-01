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
        let read = run.read_count();
        let scans = rotations
            .iter()
            .filter(|r| !matches!(r, Some((_, Basis::Text | Basis::Ocr))))
            .count();
        let mut turned = HashSet::new();
        let (mut from_image, mut from_ocr) = (0, 0);
        for (p, r) in rotations.iter().enumerate() {
            if let Some((r, basis)) = *r
                && run.turn
                && r != o.session.rotation(p)
            {
                o.session.rotate(p, i32::from(r));
                turned.insert(p);
                match basis {
                    Basis::Image => from_image += 1,
                    Basis::Ocr => from_ocr += 1,
                    Basis::Text => {}
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
        if !turned.is_empty() {
            self.rebuild(&turned, true);
        }
    }
}

/// An auto-orient run: pages done so far, how many there are, whether it
/// runs quietly, whether it turns pages, and pages read with OCR so far.
pub(super) struct Orienting {
    pub done: Arc<AtomicUsize>,
    pub total: usize,
    pub quiet: bool,
    pub turn: bool,
    pub read: Option<Arc<AtomicUsize>>,
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
        let Some(o) = &mut self.open else { return };
        let built = match o.session.build() {
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
