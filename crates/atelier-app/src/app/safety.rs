//! What a file might do or hide, shown before it is trusted: its XFA form
//! (and whether Atelier can show it), the scripts tied to elements on the
//! page, and the files embedded in it. Scripts are never run here; one a
//! person approves is only marked approved, for the script engine to come.
//! The file as opened is kept so it can be saved out untouched.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use atelier_form::{Action, Hotspot, ScriptTrait, Xfa, document_actions, hotspots, script_traits};
use atelier_syntax::Document;
use atelier_view::{Attachment, Checked, attachment_data, attachments, check_attachment};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2};

use super::Viewer;

const RISKY: Color32 = Color32::from_rgb(230, 140, 60);
const NOTE: Color32 = Color32::from_rgb(140, 180, 255);
const ACCENT: Color32 = Color32::from_rgb(40, 110, 230);
const APPROVED: Color32 = Color32::from_rgb(90, 180, 110);
/// Embedded files bigger than this (as stored) are checked only on request.
const AUTO_CHECK: usize = 16 << 20;

/// Everything known about what the open file carries.
pub(super) struct Safety {
    /// The file exactly as opened, for saving out untouched.
    pub original: Arc<Document>,
    pub actions: Vec<Action>,
    /// What each action's script uses, by action index.
    pub traits: Vec<Vec<(ScriptTrait, &'static str)>>,
    pub hotspots: Vec<Hotspot>,
    pub xfa: Option<Xfa>,
    pub files: Vec<Attachment>,
    /// Embedded files looked at so far, by index.
    checked: HashMap<usize, Result<Checked, String>>,
    /// The notice under the toolbar was closed.
    pub dismissed: bool,
    /// A dynamic XFA form: the pages are held back until the person
    /// chooses to see the stand-in pages.
    pub held: bool,
    /// Actions the person approved, by index. Nothing runs them yet.
    pub approved: HashSet<usize>,
    /// The element last clicked whose scripts are being reviewed.
    pub review: Option<usize>,
    /// The hotspot under the pointer.
    hovered: Option<usize>,
}

impl Safety {
    pub fn new(doc: Arc<Document>) -> Self {
        let actions = document_actions(&doc);
        let traits = actions
            .iter()
            .map(|a| {
                a.javascript
                    .as_deref()
                    .map(script_traits)
                    .unwrap_or_default()
            })
            .collect();
        let hotspots = hotspots(&doc, &actions);
        let xfa = atelier_form::xfa(&doc);
        let files = attachments(&doc);
        Self {
            held: xfa.as_ref().is_some_and(|x| x.dynamic),
            original: doc,
            actions,
            traits,
            hotspots,
            xfa,
            files,
            checked: HashMap::new(),
            dismissed: false,
            approved: HashSet::new(),
            review: None,
            hovered: None,
        }
    }

    fn scripts(&self) -> usize {
        self.actions.iter().filter(|a| a.is_javascript()).count()
    }

    /// Scripts that behave differently, or only work, in some viewers.
    fn viewer_dependent(&self) -> usize {
        let xfa = self.xfa.as_ref().map_or(0, |x| x.scripts.len());
        let acro = self
            .traits
            .iter()
            .filter(|t| t.iter().any(|(t, _)| t.is_viewer_dependent()))
            .count();
        xfa + acro
    }

    /// Whether the file is worth a word before it is used: an XFA form,
    /// scripts that reach outside the document, or embedded files.
    /// Ordinary form scripts alone are not.
    fn worth_noting(&self) -> bool {
        self.xfa.is_some()
            || !self.files.is_empty()
            || self.traits.iter().flatten().any(|(t, _)| t.needs_care())
    }

    /// The hotspot at a point in a page's user space, topmost first.
    fn hotspot_at(&self, page: usize, x: f64, y: f64) -> Option<usize> {
        self.hotspots
            .iter()
            .enumerate()
            .rev()
            .find(|(_, h)| h.page == page && h.contains(x, y))
            .map(|(i, _)| i)
    }

    /// The actions a click on a hotspot would set off.
    fn on_click(&self, hotspot: usize) -> Vec<usize> {
        self.hotspots.get(hotspot).map_or_else(Vec::new, |h| {
            h.actions
                .iter()
                .copied()
                .filter(|&i| self.actions[i].on_click)
                .collect()
        })
    }
}

impl Viewer {
    /// The notice under the toolbar: one quiet line saying what the file
    /// carries, until it is closed.
    pub(super) fn safety_notice(&mut self, ui: &mut egui::Ui) {
        let Some(s) = self.open.as_ref().map(|o| &o.safety) else {
            return;
        };
        if s.dismissed || !s.worth_noting() {
            return;
        }
        let mut parts = Vec::new();
        if let Some(x) = &s.xfa {
            parts.push(if x.dynamic {
                "Dynamic XFA form: Atelier shows the file's stand-in pages, not the form"
                    .to_string()
            } else {
                "Static XFA form: Atelier shows and fills its AcroForm copy".to_string()
            });
        }
        let scripts = s.scripts() + s.xfa.as_ref().map_or(0, |x| x.scripts.len());
        if scripts > 0 {
            let dependent = s.viewer_dependent();
            parts.push(if dependent > 0 {
                format!("{scripts} scripts, {dependent} viewer-dependent (none run)")
            } else {
                format!("{scripts} scripts (none run)")
            });
        }
        if !s.files.is_empty() {
            parts.push(match s.files.len() {
                1 => "1 embedded file".to_string(),
                n => format!("{n} embedded files"),
            });
        }
        let mut details = false;
        let mut save = false;
        let mut close = false;
        egui::Panel::top("safety").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(NOTE, "ⓘ");
                ui.label(parts.join(" · "));
                if ui.small_button("Details").clicked() {
                    details = true;
                }
                if s.xfa.is_some()
                    && ui
                        .small_button("Save original…")
                        .on_hover_text("Save the file exactly as it was opened, for other viewers")
                        .clicked()
                {
                    save = true;
                }
                if ui
                    .small_button("✕")
                    .on_hover_text("Hide this notice")
                    .clicked()
                {
                    close = true;
                }
            });
        });
        if details {
            self.inspector.open = true;
            self.inspector.show_safety();
        }
        if save {
            self.save_original();
        }
        if close && let Some(o) = &mut self.open {
            o.safety.dismissed = true;
        }
    }

    /// In place of the pages of a dynamic XFA form, until the person asks
    /// for them: what the file is and what Atelier can show of it.
    /// Returns whether the pages are held back.
    pub(super) fn held_view(&mut self, ui: &mut egui::Ui) -> bool {
        let Some(o) = &self.open else { return false };
        if !o.safety.held {
            return false;
        }
        let rect = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, super::BACKDROP);
        let reasons = o
            .safety
            .xfa
            .as_ref()
            .map(|x| x.reasons.clone())
            .unwrap_or_default();
        let mut show = false;
        let mut save = false;
        let mut inspect = false;
        let card = Rect::from_center_size(
            rect.center(),
            Vec2::new(520.0_f32.min(rect.width() - 32.0), 300.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                ui.set_width(card.width() - 24.0);
                ui.heading("This is a dynamic XFA form");
                ui.add_space(6.0);
                ui.label(
                    "Atelier does not draw XFA forms. The PDF pages are a stand-in that \
                     viewers without XFA show, often just a \"please wait\" page.",
                );
                ui.add_space(4.0);
                for r in &reasons {
                    ui.label(RichText::new(format!("• {r}")).weak());
                }
                ui.add_space(4.0);
                ui.label(
                    "The file is left as it is. Open it in Adobe Acrobat or Reader to use \
                     the form; you can still inspect it here.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Show the PDF pages").clicked() {
                        show = true;
                    }
                    if ui.button("Save original…").clicked() {
                        save = true;
                    }
                    if ui.button("Inspect").clicked() {
                        inspect = true;
                    }
                });
            });
        });
        if show && let Some(o) = &mut self.open {
            o.safety.held = false;
        }
        if save {
            self.save_original();
        }
        if inspect {
            self.inspector.open = true;
            self.inspector.show_safety();
        }
        true
    }

    /// Writes the file exactly as it was opened, whatever was changed
    /// since, so other viewers get the untouched original.
    pub(super) fn save_original(&mut self) {
        let Some(o) = &self.open else { return };
        let stem = o.name.strip_suffix(".pdf").unwrap_or(&o.name);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .set_file_name(format!("{stem} (original).pdf"))
            .save_file()
        else {
            return;
        };
        match super::write_replacing(&path, o.safety.original.data()) {
            Ok(()) => {
                self.notice = Some(format!(
                    "Saved the original, untouched, as {}.",
                    super::display_name(&path)
                ));
            }
            Err(e) => self.error = Some(format!("Could not save {}: {e}", path.display())),
        }
    }

    /// The pointer over the page: names the action under it, and outlines
    /// the element it belongs to.
    pub(super) fn safety_hover(&mut self, response: &egui::Response, at: Option<(f64, f64)>) {
        let Some(o) = &mut self.open else { return };
        let found = at.and_then(|(lx, ly)| {
            let (page, _, _) = o.layout.page_at(lx, ly)?;
            let user = o.layout.to_user(page, lx, ly)?;
            o.safety.hotspot_at(page, user.x, user.y)
        });
        o.safety.hovered = found;
        let Some(h) = found.and_then(|i| o.safety.hotspots.get(i)) else {
            return;
        };
        let text = if h.has_javascript(&o.safety.actions) {
            "JavaScript action".to_string()
        } else {
            let kind = h
                .actions
                .first()
                .map_or("", |&i| o.safety.actions[i].kind.as_str());
            format!("{kind} action")
        };
        response.clone().on_hover_text_at_pointer(text);
    }

    /// A click on the page: when it lands on an element whose click would
    /// run scripts not yet approved, the inspector opens on them.
    pub(super) fn safety_click(&mut self, lx: f64, ly: f64) {
        let Some(o) = &mut self.open else { return };
        let Some((page, _, _)) = o.layout.page_at(lx, ly) else {
            return;
        };
        let Some(user) = o.layout.to_user(page, lx, ly) else {
            return;
        };
        let Some(h) = o.safety.hotspot_at(page, user.x, user.y) else {
            return;
        };
        let s = &o.safety;
        let unapproved = s
            .on_click(h)
            .into_iter()
            .any(|i| s.actions[i].is_javascript() && !s.approved.contains(&i));
        if unapproved {
            o.safety.review = Some(h);
            self.inspector.open = true;
            self.inspector.show_actions();
        }
    }

    /// The outline of the element under the pointer or being reviewed.
    pub(super) fn draw_hotspots(&self, ui: &egui::Ui, origin: Pos2) {
        let Some(o) = &self.open else { return };
        let s = &o.safety;
        let painter = ui.painter();
        for (i, colour) in [(s.hovered, NOTE), (s.review, ACCENT)] {
            let Some(h) = i.and_then(|i| s.hotspots.get(i)) else {
                continue;
            };
            if let Some(r) = self.user_rect(h.page, &h.rect, origin) {
                painter.rect_stroke(
                    r.expand(1.0),
                    2.0,
                    Stroke::new(1.5, colour),
                    StrokeKind::Outside,
                );
            }
        }
    }

    // ----- Inspector tabs ---------------------------------------------

    /// The Actions tab: the element being reviewed first, with what a
    /// click would run, then every action in the file.
    pub(super) fn actions_tab(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &mut self.open else { return };
        let s = &mut o.safety;
        ui.colored_label(
            NOTE,
            "Atelier never runs scripts on its own. They are listed here so you can see \
             what this file would try to do.",
        );
        ui.separator();
        if s.actions.is_empty() && s.xfa.as_ref().is_none_or(|x| x.scripts.is_empty()) {
            ui.label("This file has no scripts or actions.");
            return;
        }
        let mut notice = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                if let Some(h) = s.review.and_then(|h| s.hotspots.get(h)).cloned() {
                    let click = s.on_click(s.review.unwrap_or(0));
                    let others: Vec<usize> = h
                        .actions
                        .iter()
                        .copied()
                        .filter(|i| !click.contains(i))
                        .collect();
                    let title = click
                        .first()
                        .or(others.first())
                        .map_or("", |&i| s.actions[i].location.as_str());
                    let element = title
                        .rsplit_once(", ")
                        .map_or(title, |(e, _)| e)
                        .to_string();
                    egui::Frame::group(ui.style())
                        .stroke(Stroke::new(1.5, ACCENT))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(element).strong());
                                if ui
                                    .small_button("✕")
                                    .on_hover_text("Stop reviewing")
                                    .clicked()
                                {
                                    s.review = None;
                                }
                            });
                            if !click.is_empty() {
                                ui.label("A click on it would run:");
                                for &i in &click {
                                    action_row(ui, s, i, true);
                                }
                                let pending: Vec<usize> = click
                                    .iter()
                                    .copied()
                                    .filter(|i| {
                                        s.actions[*i].is_javascript() && !s.approved.contains(i)
                                    })
                                    .collect();
                                ui.horizontal(|ui| {
                                    if !pending.is_empty()
                                        && ui
                                            .button("Execute")
                                            .on_hover_text("Approve these scripts for this file")
                                            .clicked()
                                    {
                                        s.approved.extend(pending.iter().copied());
                                        notice = Some(
                                        "Approved. Nothing ran: Atelier has no script engine yet."
                                            .to_string(),
                                    );
                                    }
                                    if ui.button("Deny").clicked() {
                                        for i in &click {
                                            s.approved.remove(i);
                                        }
                                        s.review = None;
                                    }
                                });
                            }
                            if !others.is_empty() {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Also tied to it:").weak());
                                for &i in &others {
                                    action_row(ui, s, i, false);
                                }
                            }
                        });
                    ui.add_space(8.0);
                }
                if let Some(x) = s.xfa.as_ref().filter(|x| !x.scripts.is_empty()) {
                    {
                        ui.label(
                            RichText::new(format!("XFA scripts ({})", x.scripts.len())).strong(),
                        );
                        for (i, script) in x.scripts.iter().enumerate() {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(RISKY, RichText::new(&script.language).strong());
                                ui.label(&script.location);
                            });
                            traits_line(ui, &script_traits(&script.source));
                            code(ui, ("xfa", i), &script.source);
                            ui.separator();
                        }
                        ui.label(RichText::new("Actions").strong());
                    }
                }
                for i in 0..s.actions.len() {
                    action_row(ui, s, i, false);
                    ui.separator();
                }
            });
        if let Some(n) = notice {
            self.notice = Some(n);
        }
    }

    /// The Files tab: XFA, then every embedded file with what it looks
    /// like, its hash and a way to save it out.
    pub(super) fn files_tab(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &mut self.open else { return };
        let s = &mut o.safety;
        let doc = s.original.clone();
        let mut save = None;
        let mut open = None;
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                match &s.xfa {
                    None => {
                        ui.label("No XFA form.");
                    }
                    Some(x) => {
                        let kind = if x.dynamic { "Dynamic" } else { "Static" };
                        ui.label(RichText::new(format!("{kind} XFA form")).strong());
                        for r in &x.reasons {
                            ui.label(format!("• {r}"));
                        }
                        let names: Vec<&str> = x.packets.iter().map(|(n, _)| n.as_str()).collect();
                        ui.label(RichText::new(format!("Packets: {}", names.join(", "))).weak());
                        ui.label(
                            RichText::new(
                                "The XFA is kept exactly as it is; saving only appends changes.",
                            )
                            .weak(),
                        );
                    }
                }
                ui.separator();
                if s.files.is_empty() {
                    ui.label("No embedded files.");
                    return;
                }
                ui.label(RichText::new(format!("Embedded files ({})", s.files.len())).strong());
                ui.label(
                    RichText::new("Files are never opened from here; save one out to look at it.")
                        .weak(),
                );
                ui.separator();
                for i in 0..s.files.len() {
                    let a = &s.files[i];
                    if !s.checked.contains_key(&i) && stored_len(&doc, a) <= AUTO_CHECK {
                        let c = attachment_data(&doc, a)
                            .map(|d| check_attachment(a, &d))
                            .map_err(|e| e.to_string());
                        s.checked.insert(i, c);
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&a.name).strong());
                        ui.label(RichText::new(a.source.label()).weak());
                    });
                    if let Some(d) = &a.description {
                        ui.label(d);
                    }
                    let mut facts = Vec::new();
                    if let Some(m) = &a.mime {
                        facts.push(format!("says {m}"));
                    }
                    if let Some(r) = &a.relationship {
                        facts.push(format!("relationship {r}"));
                    }
                    if let Some(d) = &a.modified.clone().or(a.created.clone()) {
                        facts.push(d.clone());
                    }
                    if !facts.is_empty() {
                        ui.label(RichText::new(facts.join(" · ")).weak());
                    }
                    match s.checked.get(&i) {
                        Some(Ok(c)) => {
                            ui.label(format!("{} · {}", size_text(c.size), c.kind));
                            for w in &c.cautions {
                                ui.colored_label(RISKY, format!("⚠ {w}"));
                            }
                            if c.checksum_ok == Some(true) {
                                ui.label(RichText::new("Checksum matches.").weak());
                            }
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("SHA-256").weak());
                                ui.add(
                                    egui::TextEdit::singleline(&mut c.sha256.as_str())
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY),
                                );
                            });
                        }
                        Some(Err(e)) => {
                            ui.colored_label(RISKY, format!("Cannot read it: {e}"));
                        }
                        None => {
                            if ui.small_button("Check").clicked() {
                                let c = attachment_data(&doc, a)
                                    .map(|d| check_attachment(a, &d))
                                    .map_err(|e| e.to_string());
                                s.checked.insert(i, c);
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        if a.stream.is_some() && ui.button("Save…").clicked() {
                            save = Some(i);
                        }
                        let pdf =
                            matches!(s.checked.get(&i), Some(Ok(c)) if c.kind == "PDF document");
                        if pdf
                            && ui
                                .button("Open in Atelier")
                                .on_hover_text(
                                    "Open the embedded PDF here (its scripts are not run)",
                                )
                                .clicked()
                        {
                            open = Some(i);
                        }
                    });
                    ui.separator();
                }
            });
        if let Some(i) = save {
            self.save_attachment(i);
        }
        if let Some(i) = open {
            self.open_attachment(i);
        }
    }

    fn save_attachment(&mut self, i: usize) {
        let Some(s) = self.open.as_ref().map(|o| &o.safety) else {
            return;
        };
        let Some(a) = s.files.get(i) else { return };
        let data = match attachment_data(&s.original, a) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(format!("Could not read {}: {e}", a.name));
                return;
            }
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(safe_file_name(&a.name))
            .save_file()
        else {
            return;
        };
        match super::write_replacing(&path, &data) {
            Ok(()) => {
                self.notice = Some(format!("Saved {}.", super::display_name(&path)));
            }
            Err(e) => self.error = Some(format!("Could not save {}: {e}", path.display())),
        }
    }

    fn open_attachment(&mut self, i: usize) {
        let Some(s) = self.open.as_ref().map(|o| &o.safety) else {
            return;
        };
        let Some(a) = s.files.get(i) else { return };
        match attachment_data(&s.original, a) {
            Ok(data) => {
                let name = safe_file_name(&a.name);
                self.open_bytes(data, name, None);
            }
            Err(e) => self.error = Some(format!("Could not read {}: {e}", a.name)),
        }
    }
}

/// One action: its kind, where it is, what it uses and its script.
fn action_row(ui: &mut egui::Ui, s: &Safety, i: usize, highlight: bool) {
    let a = &s.actions[i];
    ui.horizontal_wrapped(|ui| {
        let risky = matches!(
            a.kind.as_str(),
            "JavaScript" | "Launch" | "URI" | "SubmitForm" | "ImportData" | "GoToR" | "GoToE"
        );
        let kind = RichText::new(&a.kind).strong();
        if risky {
            ui.colored_label(RISKY, kind);
        } else {
            ui.label(kind);
        }
        ui.label(&a.location);
        if let Some(r) = a.owner {
            ui.label(RichText::new(r.to_string()).weak());
        }
        if s.approved.contains(&i) {
            ui.colored_label(APPROVED, "approved");
        }
    });
    if let Some(t) = &a.target {
        ui.label(format!("→ {t}"));
    }
    traits_line(ui, &s.traits[i]);
    if let Some(js) = &a.javascript {
        if highlight {
            egui::Frame::new()
                .fill(ui.visuals().selection.bg_fill.gamma_multiply(0.35))
                .inner_margin(4.0)
                .corner_radius(3.0)
                .show(ui, |ui| code(ui, ("review", i), js));
        } else {
            code(ui, ("script", i), js);
        }
    }
}

fn traits_line(ui: &mut egui::Ui, traits: &[(ScriptTrait, &'static str)]) {
    if traits.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (t, name) in traits {
            let colour = if t.needs_care() { RISKY } else { NOTE };
            ui.colored_label(colour, format!("• {}", t.label()))
                .on_hover_text(format!("Found {name}"));
        }
    });
}

fn code(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, text: &str) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(220.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut &*text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(1),
            );
        });
}

/// The size of a file's stream as stored, before decoding.
fn stored_len(doc: &Document, a: &Attachment) -> usize {
    match a.stream.map(|r| doc.get(r)) {
        Some(Ok(atelier_syntax::Object::Stream(s))) => s.data.len(),
        _ => 0,
    }
}

fn size_text(n: usize) -> String {
    match n {
        n if n < 1 << 10 => format!("{n} bytes"),
        n if n < 1 << 20 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
    }
}

/// A file name from inside a PDF, safe to offer in a save dialog: no
/// folders, no characters Windows refuses.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_matches('.').to_string();
    if clean.is_empty() {
        "attachment".into()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_lose_folders_and_bad_characters() {
        assert_eq!(safe_file_name("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(safe_file_name("a/b/c?.txt"), "c_.txt");
        assert_eq!(safe_file_name(".."), "attachment");
        assert_eq!(safe_file_name("report.pdf"), "report.pdf");
    }
}
