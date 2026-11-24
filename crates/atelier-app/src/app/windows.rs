//! More than one window, all in the one Atelier, so they share its icon
//! on the taskbar or in the Dock. The first window's viewer owns the
//! others; each has a viewer of its own, with its own tabs.
//!
//! A tab dragged out of a window comes to a new one as it is, with its
//! unsaved changes and undo. Closing the first window while others are
//! open closes only it: the last of the others takes its place, so the
//! rest stay. Quitting closes them all, asking about unsaved changes in
//! each.

use eframe::egui::{self, Pos2, ViewportCommand, ViewportId};

use super::tabs::Parked;
use super::{Pending, Viewer};

/// A window besides the first.
pub(super) struct Child {
    pub(super) id: ViewportId,
    builder: egui::ViewportBuilder,
    pub(super) viewer: Viewer,
    /// The tab it opens with, put in on its first frame so the title goes
    /// to its own window.
    arriving: Option<Parked>,
    had_tabs: bool,
    /// It was closed, and is dropped after this frame.
    gone: bool,
}

/// Tells windows apart.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Viewer {
    /// Closes every window, asking about unsaved changes in each.
    pub(super) fn quit(&mut self) {
        self.quit = true;
        self.ctx
            .send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    /// The first window was asked to close. Unsaved changes are asked
    /// about first; then, with other windows open, one of them moves into
    /// this one unless the whole of Atelier is quitting.
    pub(super) fn root_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if !self.closing && self.show_unsaved_tab() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.confirm = Some(Pending::Close);
            return;
        }
        if self.windows.is_empty() {
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        if self.quit {
            // Once the others are all saved or given up, this goes too.
            self.closing = true;
            return;
        }
        self.closing = false;
        self.drop_all();
        let Some(mut last) = self.windows.pop() else {
            return;
        };
        if let Some(p) = last.arriving.take() {
            last.viewer.tabs.push(Some(p));
        }
        let (tabs, front) = last.viewer.take_all();
        self.adopt(tabs, front);
        // It takes the other window's place on screen, so to the eye that
        // one stays and this one goes.
        let (outer, inner) = ctx.input_for(last.id, |i| {
            (i.viewport().outer_rect, i.viewport().inner_rect)
        });
        if let Some(outer) = outer {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(outer.min));
        }
        if let Some(inner) = inner {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(inner.size()));
        }
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    /// Shows the other windows, opens the ones tabs were dragged out to,
    /// and keeps their settings in step.
    pub(super) fn show_windows(&mut self, ctx: &egui::Context) {
        for w in &mut self.windows {
            let (id, builder) = (w.id, w.builder.clone());
            ctx.show_viewport_immediate(id, builder, |ui, _| w.frame(ui));
            if w.viewer.quit {
                w.viewer.quit = false;
                self.quit = true;
            }
        }
        self.windows.retain(|w| !w.gone);
        self.finish_tab_drops(ctx);
        let mut leaving = std::mem::take(&mut self.leaving);
        for w in &mut self.windows {
            leaving.append(&mut w.viewer.leaving);
        }
        for (tab, at) in leaving {
            let child = self.child(Some(tab), at);
            self.windows.push(child);
            ctx.request_repaint_of(ViewportId::ROOT);
        }
        self.sync_settings();
        if self.quit {
            self.quit_tick(ctx);
        }
    }

    fn finish_tab_drops(&mut self, ctx: &egui::Context) {
        let mut drops = Vec::new();
        if let Some(drop) = self.tab_drop.take() {
            drops.push((ViewportId::ROOT, drop));
        }
        for child in &mut self.windows {
            if let Some(drop) = child.viewer.tab_drop.take() {
                drops.push((child.id, drop));
            }
        }
        for (source, (tab, point)) in drops {
            let target = self
                .windows
                .iter()
                .rev()
                .filter(|child| child.id != source && !child.gone && child.viewer.confirm.is_none())
                .find_map(|child| {
                    child
                        .viewer
                        .tab_strip
                        .as_ref()?
                        .slot(point)
                        .map(|slot| (child.id, slot))
                })
                .or_else(|| {
                    (source != ViewportId::ROOT && self.confirm.is_none())
                        .then(|| {
                            self.tab_strip
                                .as_ref()?
                                .slot(point)
                                .map(|slot| (ViewportId::ROOT, slot))
                        })
                        .flatten()
                });
            let origin = if source == ViewportId::ROOT {
                &mut *self
            } else if let Some(child) = self.windows.iter_mut().find(|child| child.id == source) {
                &mut child.viewer
            } else {
                continue;
            };
            if origin.saving > 0 || origin.confirm.is_some() {
                continue;
            }
            let Some((destination, slot)) = target else {
                let scale = ctx.input_for(source, |input| input.pixels_per_point);
                let logical = (point.to_vec2() / scale).to_pos2();
                origin.request_pop_out(tab, Some(logical - egui::vec2(80.0, 60.0)));
                continue;
            };
            let Some(tab) = origin.take_tab(tab) else {
                continue;
            };
            let emptied = origin.tabs.is_empty();
            if destination == ViewportId::ROOT {
                self.insert_tab(tab, slot);
            } else if let Some(child) = self
                .windows
                .iter_mut()
                .find(|child| child.id == destination)
            {
                child.viewer.insert_tab(tab, slot);
            }
            if emptied && source != ViewportId::ROOT {
                if let Some(child) = self.windows.iter_mut().find(|child| child.id == source) {
                    child.gone = true;
                }
                ctx.send_viewport_cmd_to(source, ViewportCommand::Close);
            }
            ctx.request_repaint_of(source);
            ctx.request_repaint_of(destination);
            ctx.send_viewport_cmd_to(destination, ViewportCommand::Focus);
        }
    }

    pub(super) fn open_empty_window(&mut self) {
        let child = self.child(None, None);
        self.windows.push(child);
    }

    /// A new window, optionally with a transferred tab and screen position.
    fn child(&self, tab: Option<Parked>, at: Option<Pos2>) -> Child {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let size = self
            .ctx
            .input(|i| i.viewport().inner_rect)
            .map(|r| r.size());
        let mut builder = crate::chrome::window(self.icon.clone());
        if let Some(size) = size {
            builder = builder.with_inner_size(size);
        }
        if let Some(at) = at {
            builder = builder.with_position(at);
        }
        let id = ViewportId::from_hash_of(("atelier-window", n));
        let mut viewer = Viewer::blank_for(self.ctx.clone(), self.settings.clone(), id);
        viewer.icon = self.icon.clone();
        Child {
            id,
            builder,
            viewer,
            arriving: tab,
            had_tabs: false,
            gone: false,
        }
    }

    /// Quitting: asks each window with unsaved changes in turn, and
    /// closes the first window, and with it Atelier, once none has any.
    /// Cancelling any of the prompts stops it.
    fn quit_tick(&mut self, ctx: &egui::Context) {
        if self.confirm.is_some() || self.windows.iter().any(|w| w.viewer.confirm.is_some()) {
            return;
        }
        // The first window has not got as far as closing yet.
        if !self.closing {
            return;
        }
        if let Some(w) = self.windows.iter_mut().find(|w| w.viewer.has_unsaved()) {
            if w.viewer.closing {
                // Asked already and still unsaved: the prompt was cancelled.
                w.viewer.closing = false;
                self.closing = false;
                self.quit = false;
                return;
            }
            w.viewer.show_unsaved_tab();
            w.viewer.confirm = Some(Pending::Close);
            w.viewer.closing = true;
            ctx.send_viewport_cmd_to(w.id, ViewportCommand::Focus);
            return;
        }
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        self.windows.clear();
    }

    /// When Settings changed in one window, every window takes it up.
    fn sync_settings(&mut self) {
        let seen = self
            .settings_seen
            .get_or_insert_with(|| self.settings.clone());
        let changed = std::iter::once(&self.settings)
            .chain(self.windows.iter().map(|w| &w.viewer.settings))
            .find(|s| *s != seen)
            .cloned();
        let Some(settings) = changed else { return };
        *seen = settings.clone();
        if self.settings != settings {
            self.settings = settings.clone();
            self.apply_budgets();
        }
        for w in &mut self.windows {
            if w.viewer.settings != settings {
                w.viewer.settings = settings.clone();
                w.viewer.apply_budgets();
            }
        }
    }
}

impl Child {
    /// One frame of this window.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let v = &mut self.viewer;
        if let Some(tab) = self.arriving.take() {
            v.new_tab();
            v.unpark(tab);
        }
        if ui.input(|i| i.viewport().close_requested()) {
            if v.closing || !v.show_unsaved_tab() {
                self.gone = true;
            } else {
                ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                v.confirm = Some(Pending::Close);
            }
        }
        #[cfg(target_os = "macos")]
        v.mac_commands();
        v.window_ui(ui, None);
        // A window whose last tab closed goes with it.
        self.had_tabs |= !v.tabs.is_empty();
        if self.had_tabs && v.tabs.is_empty() && v.loading == 0 && v.locked.is_none() {
            self.gone = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
%%EOF";

    fn viewer() -> Viewer {
        Viewer::initialize(
            &eframe::CreationContext::_new_kittest(egui::Context::default()),
            crate::settings::Settings {
                check_updates: false,
                ocr: false,
                orient_on_open: false,
                ..Default::default()
            },
        )
    }

    fn names(v: &Viewer) -> Vec<String> {
        v.tabs
            .iter()
            .map(|t| match t {
                Some(p) => p.open.name.clone(),
                None => format!("[{}]", v.open.as_ref().unwrap().name),
            })
            .collect()
    }

    #[test]
    fn a_tab_moves_to_another_window_and_back_as_it_was() {
        let mut first = viewer();
        first.open_with_password(PDF.to_vec(), "a.pdf".into(), "", None);
        first.open_with_password(PDF.to_vec(), "b.pdf".into(), "", None);
        first.open.as_mut().unwrap().layers_pending = true;
        assert_eq!(names(&first), ["a.pdf", "[b.pdf]"]);

        first.request_pop_out(1, None);
        assert_eq!(names(&first), ["[a.pdf]"]);
        let (tab, _) = first.leaving.pop().unwrap();
        // A window with a single tab keeps it.
        first.request_pop_out(0, None);
        assert!(first.leaving.is_empty());

        let mut other = Viewer::blank(first.ctx.clone(), first.settings.clone());
        other.new_tab();
        other.unpark(tab);
        assert_eq!(names(&other), ["[b.pdf]"]);
        // What was not yet saved came along.
        assert!(other.open.as_ref().unwrap().layers_pending);

        let (tabs, front) = other.take_all();
        first.adopt(tabs, front);
        assert_eq!(names(&first), ["a.pdf", "[b.pdf]"]);
        assert!(first.open.as_ref().unwrap().layers_pending);
    }

    #[test]
    fn a_drop_in_another_window_inserts_at_the_slot_and_moves_the_only_tab() {
        let mut root = viewer();
        root.open_bytes(PDF.to_vec(), "a.pdf".into(), None);
        root.open_bytes(PDF.to_vec(), "b.pdf".into(), None);
        root.open_bytes(PDF.to_vec(), "c.pdf".into(), None);
        root.rotate_pages(1);
        root.open.as_mut().unwrap().layers_pending = true;
        let tab = root.take_tab(2).unwrap();
        let mut child = root.child(Some(tab), None);
        child.viewer.insert_tab(child.arriving.take().unwrap(), 0);
        root.tab_strip = Some(super::super::tabs::TabStrip {
            rect: egui::Rect::from_min_max(egui::pos2(0.0, 30.0), egui::pos2(500.0, 62.0)),
            centers: vec![55.0, 165.0],
        });
        child.viewer.tab_drop = Some((0, egui::pos2(125.0, 45.0)));
        root.windows.push(child);
        root.finish_tab_drops(&root.ctx.clone());
        assert_eq!(names(&root), ["a.pdf", "[c.pdf]", "b.pdf"]);
        assert!(root.open.as_ref().unwrap().layers_pending);
        assert_eq!(root.open.as_ref().unwrap().session.rotation(0), 90);
        assert!(root.is_dirty());
        assert!(root.windows[0].viewer.tabs.is_empty());
        assert!(root.windows[0].gone);
    }

    #[test]
    fn empty_launch_handover_opens_another_window_in_the_primary() {
        let mut primary = viewer();
        let (sender, receiver) = std::sync::mpsc::channel();
        primary.incoming = Some(receiver);
        sender.send(Vec::new()).unwrap();
        primary.take_handovers();
        assert_eq!(primary.windows.len(), 1);
        let child = &mut primary.windows[0];
        let ctx = primary.ctx.clone();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| child.frame(ui));
        output.textures_delta.clear();
        assert!(child.viewer.tabs.is_empty());
        assert!(!child.gone);
    }

    #[test]
    fn pointer_drag_reorders_tabs_using_the_release_position() {
        let mut viewer = viewer();
        for name in ["a.pdf", "b.pdf", "c.pdf"] {
            viewer.open_bytes(PDF.to_vec(), name.into(), None);
        }
        let ctx = viewer.ctx.clone();
        let mut frame = |events| {
            let bounds = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 600.0));
            let mut input = egui::RawInput {
                screen_rect: Some(bounds),
                events,
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&ViewportId::ROOT)
                .unwrap()
                .inner_rect = Some(bounds);
            let mut output = ctx.run_ui(input, |ui| viewer.tab_bar(ui));
            output.textures_delta.clear();
            let strip = viewer.tab_strip.as_ref().unwrap();
            (
                egui::pos2(strip.centers[2] - 20.0, strip.rect.center().y),
                egui::pos2(strip.centers[0] - 30.0, strip.rect.center().y),
            )
        };
        let (start, end) = frame(Vec::new());
        let button = |position, pressed| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(start)]);
        frame(vec![button(start, true)]);
        frame(vec![egui::Event::PointerMoved(start.lerp(end, 0.5))]);
        frame(vec![egui::Event::PointerMoved(end), button(end, false)]);
        assert_eq!(names(&viewer), ["[c.pdf]", "a.pdf", "b.pdf"]);
    }
}
