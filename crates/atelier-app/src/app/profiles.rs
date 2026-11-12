//! The Form profiles window: keeping named sets of details (name,
//! addresses, phone and whatever else the person adds) and filling the
//! fields of a page or of the whole document from one of them.
//!
//! What a profile would fill is listed before anything is filled, so a
//! match that is wrong can be unticked.
//!
//! An encrypted profile is decrypted only into memory, and only while
//! this window is open: closing the window locks every profile, as does
//! leaving one unused for a few minutes. Locking overwrites the details,
//! the keys and anything typed here. The text boxes are egui's, which
//! keeps its own copies of what is being edited (for undo and for
//! drawing); those are dropped when editing ends, not overwritten.

use std::collections::HashSet;
use std::time::Duration;

use atelier_form::{
    Entry, EntryKind, Form, Planned, Profile, Session, Skipped, Standard, Value, wipe_string,
};
use eframe::egui::{self, Vec2};

use super::Viewer;
use crate::profiles::{Item, Store};
use crate::sealing;

/// An unlocked profile nobody has used for this long is locked.
const IDLE: Duration = Duration::from_secs(5 * 60);
/// Room a text box starts with, so that typing does not move the text
/// and leave a copy behind where it was.
const ROOM: usize = 256;

/// Which fields to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Scope {
    #[default]
    Page,
    Document,
}

/// What a worked-out plan was worked out for.
#[derive(PartialEq, Eq)]
struct PlanKey {
    profile: String,
    revision: u64,
    generation: u64,
    /// The page, when only one is being filled.
    page: Option<usize>,
    replace: bool,
}

struct Plan {
    key: PlanKey,
    items: Vec<Planned>,
    /// Fields the person unticked.
    off: HashSet<usize>,
}

/// A profile being written or changed.
struct Draft {
    /// The profile it replaces, when it is not new.
    id: Option<String>,
    name: String,
    standard: Vec<(Standard, String)>,
    /// Label and value of each added detail.
    custom: Vec<(String, String)>,
    encrypt: bool,
    was_encrypted: bool,
    passphrase: String,
    again: String,
    problem: Option<String>,
    focus: bool,
    /// The text boxes shown, whose editing state goes when the draft does.
    boxes: HashSet<egui::Id>,
}

/// A copy of some text with room to grow.
fn roomy(text: &str) -> String {
    let mut s = String::with_capacity(ROOM.max(text.len() * 2));
    s.push_str(text);
    s
}

impl Draft {
    fn new(item: Option<&Item>) -> Self {
        let profile = item.and_then(Item::profile);
        Self {
            id: item.map(|i| i.id.clone()),
            name: item.map_or_else(String::new, |i| i.name.clone()),
            standard: Standard::ALL
                .into_iter()
                .map(|s| (s, roomy(profile.map_or("", |p| p.standard(s)))))
                .collect(),
            custom: profile
                .into_iter()
                .flat_map(|p| &p.entries)
                .filter_map(|e| match &e.kind {
                    EntryKind::Custom(label) => Some((label.clone(), roomy(&e.value))),
                    EntryKind::Standard(_) => None,
                })
                .collect(),
            encrypt: item.is_some_and(Item::encrypted),
            was_encrypted: item.is_some_and(Item::encrypted),
            passphrase: String::with_capacity(ROOM),
            again: String::with_capacity(ROOM),
            problem: None,
            focus: true,
            boxes: HashSet::new(),
        }
    }

    /// The profile as typed, without the details left empty.
    fn profile(&self) -> Profile {
        let standard = self
            .standard
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(s, v)| Entry {
                kind: EntryKind::Standard(*s),
                value: v.trim().to_string(),
            });
        let custom = self
            .custom
            .iter()
            .filter(|(l, v)| !l.trim().is_empty() && !v.trim().is_empty())
            .map(|(l, v)| Entry {
                kind: EntryKind::Custom(l.trim().to_string()),
                value: v.trim().to_string(),
            });
        Profile {
            name: self.name.trim().to_string(),
            entries: standard.chain(custom).collect(),
        }
    }

    /// Drops what egui remembers of the text boxes.
    fn forget(&self, ctx: &egui::Context) {
        for id in &self.boxes {
            egui::TextEdit::store_state(ctx, *id, egui::text_edit::TextEditState::default());
        }
    }
}

impl Drop for Draft {
    fn drop(&mut self) {
        for (_, v) in &mut self.standard {
            wipe_string(v);
        }
        for (_, v) in &mut self.custom {
            wipe_string(v);
        }
        wipe_string(&mut self.passphrase);
        wipe_string(&mut self.again);
    }
}

#[derive(Default)]
pub(super) struct Profiles {
    pub open: bool,
    /// The saved profiles, read when the window first opens.
    store: Option<Store>,
    /// The id of the profile picked in the list.
    selected: Option<String>,
    search: String,
    scope: Scope,
    /// Fill fields that already hold something.
    replace: bool,
    /// Show an encrypted profile's values in the list of matches.
    reveal: bool,
    /// What is typed in the unlock box.
    passphrase: String,
    problem: Option<String>,
    draft: Option<Draft>,
    plan: Option<Plan>,
    confirm_delete: bool,
}

impl Drop for Profiles {
    fn drop(&mut self) {
        wipe_string(&mut self.passphrase);
    }
}

/// The open document, as far as filling it goes.
struct DocView<'a> {
    form: Option<&'a Form>,
    session: &'a Session,
    generation: u64,
    page: usize,
}

/// Something the window asks the viewer to do.
enum Ask {
    Fill(Vec<(usize, Value)>, String),
    Detect,
}

impl Profiles {
    /// Locks everything and forgets what was typed.
    fn lock_up(&mut self, ctx: &egui::Context) {
        if let Some(draft) = self.draft.take() {
            draft.forget(ctx);
        }
        self.plan = None;
        self.confirm_delete = false;
        self.reveal = false;
        wipe_string(&mut self.passphrase);
        if let Some(store) = &mut self.store {
            store.lock_all();
        }
    }

    /// Locks profiles left unused, and asks to be woken for the next.
    fn lock_idle(&mut self, ctx: &egui::Context) {
        let Some(store) = &mut self.store else { return };
        let (locked, next) = store.lock_idle(IDLE);
        if locked {
            // What was shown or being edited may be from a locked profile.
            self.plan = None;
            if let Some(draft) = self.draft.take_if(|d| d.was_encrypted) {
                draft.forget(ctx);
            }
            self.problem = Some("Locked after five minutes without use.".into());
        }
        if let Some(next) = next {
            ctx.request_repaint_after(next + Duration::from_millis(50));
        }
    }

    fn select(&mut self, id: Option<String>) {
        if self.selected != id {
            self.selected = id;
            self.problem = None;
            self.confirm_delete = false;
            self.plan = None;
            wipe_string(&mut self.passphrase);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, doc: Option<&DocView<'_>>) -> Option<Ask> {
        if self.draft.is_some() {
            self.draft_ui(ui);
            return None;
        }
        self.list_ui(ui);
        ui.separator();
        let Some(id) = self.selected.clone() else {
            ui.weak("Pick a profile, or make a new one.");
            return None;
        };
        let (name, locked, encrypted) = {
            let item = self.store.as_ref()?.find(&id)?;
            (item.name.clone(), item.locked(), item.encrypted())
        };
        if locked {
            self.unlock_ui(ui, &id, &name);
            return None;
        }
        ui.horizontal(|ui| {
            ui.strong(&name);
            if encrypted {
                ui.weak("encrypted, unlocked");
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Edit…").clicked() {
                let item = self.store.as_ref().and_then(|s| s.find(&id));
                self.draft = Some(Draft::new(item));
                self.problem = None;
            }
            if encrypted && ui.button("Lock").clicked() {
                self.lock_up(ui.ctx());
            }
            if ui.button("Delete…").clicked() {
                self.confirm_delete = true;
            }
        });
        if self.confirm_delete {
            ui.horizontal(|ui| {
                ui.label(format!("Delete “{name}” for good?"));
                if ui.button("Delete").clicked() {
                    self.plan = None;
                    if let Some(store) = &mut self.store {
                        match store.delete(&id) {
                            Ok(()) => self.select(None),
                            Err(e) => self.problem = Some(e.to_string()),
                        }
                    }
                    self.confirm_delete = false;
                }
                if ui.button("Keep").clicked() {
                    self.confirm_delete = false;
                }
            });
        }
        if encrypted {
            ui.weak(
                "Decrypted in memory only. It locks when this window closes, \
                 or after five minutes unused.",
            );
        }
        if let Some(problem) = &self.problem {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
        if self.draft.is_some() || self.selected.is_none() {
            return None;
        }
        ui.separator();
        self.fill_ui(ui, doc, &id, &name, encrypted)
    }

    /// The search box and the profiles it finds, best first.
    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search profiles")
                    .desired_width(210.0),
            )
            .on_hover_text("Finds profiles by their name or by anything in them");
            if ui.button("New profile…").clicked() {
                self.draft = Some(Draft::new(None));
                self.problem = None;
            }
        });
        let Some(store) = &self.store else { return };
        if store.items.is_empty() {
            ui.weak(
                "No profiles yet. A profile holds a name, addresses, a phone number \
                 and anything else you fill in often.",
            );
            return;
        }
        let mut found: Vec<(f32, &Item)> = store
            .items
            .iter()
            .filter_map(|item| {
                // A locked profile can only be found by its name.
                let mut text = item.name.clone();
                for e in item.profile().into_iter().flat_map(|p| &p.entries) {
                    text.push(' ');
                    text.push_str(e.label());
                    text.push(' ');
                    text.push_str(&e.value);
                }
                let score = atelier_form::search(&self.search, &text);
                wipe_string(&mut text);
                score.map(|s| (s, item))
            })
            .collect();
        // The list is already in name order; this keeps it among equals.
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        if found.is_empty() {
            ui.weak("No profile matches.");
            return;
        }
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("profile-list")
            .max_height(132.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (_, item) in &found {
                    let text = if item.locked() {
                        format!("{}  (locked)", item.name)
                    } else if item.encrypted() {
                        format!("{}  (unlocked)", item.name)
                    } else {
                        item.name.clone()
                    };
                    let on = self.selected.as_deref() == Some(item.id.as_str());
                    if ui.selectable_label(on, text).clicked() {
                        pick = Some(item.id.clone());
                    }
                }
            });
        if pick.is_some() {
            self.select(pick);
        }
    }

    fn unlock_ui(&mut self, ui: &mut egui::Ui, id: &str, name: &str) {
        ui.label(format!(
            "“{name}” is encrypted. Enter its passphrase to use it."
        ));
        if self.passphrase.capacity() < ROOM {
            self.passphrase = String::with_capacity(ROOM);
        }
        let mut unlock = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.passphrase)
                    .password(true)
                    .hint_text("Passphrase")
                    .desired_width(210.0),
            );
            unlock |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            unlock |= ui.button("Unlock").clicked();
            if unlock {
                egui::TextEdit::store_state(
                    ui.ctx(),
                    field.id,
                    egui::text_edit::TextEditState::default(),
                );
            }
        });
        if unlock && let Some(store) = &mut self.store {
            self.problem = store
                .unlock(id, &self.passphrase)
                .err()
                .map(|e| e.to_string());
            wipe_string(&mut self.passphrase);
            self.plan = None;
        }
        if let Some(problem) = &self.problem {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
    }

    /// What the profile would fill, and the button that fills it.
    fn fill_ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: Option<&DocView<'_>>,
        id: &str,
        name: &str,
        encrypted: bool,
    ) -> Option<Ask> {
        let Some(doc) = doc else {
            ui.weak("Open a form to fill it from this profile.");
            return None;
        };
        let Some(form) = doc.form else {
            ui.label("This document has no form fields to fill.");
            let detect = ui
                .button("Find tables and form fields…")
                .on_hover_text("Suggest fields for the boxes and lines of a printed form")
                .clicked();
            return detect.then_some(Ask::Detect);
        };
        ui.horizontal(|ui| {
            ui.label("Fill");
            ui.selectable_value(&mut self.scope, Scope::Page, "This page");
            ui.selectable_value(&mut self.scope, Scope::Document, "Whole document");
        });
        ui.checkbox(&mut self.replace, "Replace what is already filled in");

        let item = self.store.as_ref()?.find(id)?;
        let key = PlanKey {
            profile: id.to_string(),
            revision: item.revision,
            generation: doc.generation,
            page: (self.scope == Scope::Page).then_some(doc.page),
            replace: self.replace,
        };
        if self.plan.as_ref().is_none_or(|p| p.key != key) {
            let profile = item.profile()?;
            let items = atelier_form::plan(form, profile, self.replace, |field| match key.page {
                Some(page) => doc.session.pages_of(field).contains(&page),
                None => true,
            });
            self.plan = Some(Plan {
                key,
                items,
                off: HashSet::new(),
            });
        }
        let plan = self.plan.as_mut()?;
        if plan.items.is_empty() {
            ui.weak(match self.scope {
                Scope::Page => "Nothing on this page matches this profile.",
                Scope::Document => "Nothing in this document matches this profile.",
            });
            return None;
        }
        if encrypted {
            ui.checkbox(&mut self.reveal, "Show values");
        }
        let hide = encrypted && !self.reveal;
        egui::ScrollArea::vertical()
            .id_salt("profile-matches")
            .max_height(240.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("profile-matches-grid")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for p in &plan.items {
                            let mut on = p.skipped.is_none() && !plan.off.contains(&p.field);
                            ui.add_enabled_ui(p.skipped.is_none(), |ui| {
                                if ui.checkbox(&mut on, short(&p.label, 34)).changed() {
                                    if on {
                                        plan.off.remove(&p.field);
                                    } else {
                                        plan.off.insert(p.field);
                                    }
                                }
                            })
                            .response
                            .on_hover_text(&p.label);
                            ui.weak(&p.from);
                            match p.skipped {
                                Some(Skipped::TooLong) => {
                                    ui.weak("too long for the field");
                                }
                                Some(Skipped::NoSuchChoice) => {
                                    ui.weak("not one of the field's choices");
                                }
                                None if hide => {
                                    ui.label("••••••");
                                }
                                None => {
                                    let mut shown = p.value.display();
                                    ui.label(short(&shown.replace('\n', " / "), 40));
                                    wipe_string(&mut shown);
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        let chosen = || {
            plan.items
                .iter()
                .filter(|p| p.skipped.is_none() && !plan.off.contains(&p.field))
        };
        let n = chosen().count();
        let label = match n {
            1 => "Fill 1 field".to_string(),
            n => format!("Fill {n} fields"),
        };
        if ui.add_enabled(n > 0, egui::Button::new(label)).clicked() {
            let fills = chosen().map(|p| (p.field, p.value.clone())).collect();
            if let Some(store) = &mut self.store {
                store.touch(id);
            }
            return Some(Ask::Fill(fills, name.to_string()));
        }
        None
    }

    /// The form a profile is written in.
    fn draft_ui(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = &mut self.draft else { return };
        let mut save = false;
        let mut cancel = false;
        ui.strong(if draft.id.is_some() {
            "Edit profile"
        } else {
            "New profile"
        });
        egui::ScrollArea::vertical()
            .id_salt("profile-draft")
            .max_height(430.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("profile-draft-grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Profile name");
                        let name = ui.add(
                            egui::TextEdit::singleline(&mut draft.name)
                                .hint_text("Home, Work…")
                                .desired_width(250.0),
                        );
                        if draft.focus {
                            name.request_focus();
                            draft.focus = false;
                        }
                        ui.end_row();
                        for (which, value) in &mut draft.standard {
                            ui.label(which.label());
                            let edit = if which.multiline() {
                                egui::TextEdit::multiline(value).desired_rows(3)
                            } else {
                                egui::TextEdit::singleline(value)
                            };
                            draft.boxes.insert(ui.add(edit.desired_width(250.0)).id);
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.label("Other details");
                ui.weak(
                    "Label each as forms do. Commas separate other names for the \
                     same thing: “SSN, Social security number”.",
                );
                let mut remove = None;
                for (i, (label, value)) in draft.custom.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(label)
                                .hint_text("Label")
                                .desired_width(130.0),
                        );
                        let edit = egui::TextEdit::singleline(value)
                            .hint_text("Value")
                            .desired_width(170.0);
                        draft.boxes.insert(ui.add(edit).id);
                        if ui.small_button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    let (_, mut value) = draft.custom.remove(i);
                    wipe_string(&mut value);
                }
                if ui.button("Add a detail").clicked() {
                    draft.custom.push((String::new(), roomy("")));
                }
                ui.add_space(6.0);
                ui.checkbox(&mut draft.encrypt, "Encrypt with a passphrase");
                if draft.encrypt {
                    egui::Grid::new("profile-passphrase-grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            for (label, text) in [
                                ("Passphrase", &mut draft.passphrase),
                                ("Again", &mut draft.again),
                            ] {
                                ui.label(label);
                                let edit = egui::TextEdit::singleline(text)
                                    .password(true)
                                    .desired_width(250.0);
                                draft.boxes.insert(ui.add(edit).id);
                                ui.end_row();
                            }
                        });
                    if draft.was_encrypted {
                        ui.weak("Leave both empty to keep the passphrase it has.");
                    }
                    ui.weak(
                        "An encrypted profile is decrypted only in memory, while this \
                         window is open. Without its passphrase it cannot be recovered.",
                    );
                    ui.weak(match sealing::system_policy() {
                        Some(true) => {
                            format!(
                                "Encrypted by {}. Windows FIPS policy is on.",
                                sealing::MODULE
                            )
                        }
                        Some(false) => {
                            format!(
                                "Encrypted by {}. Windows FIPS policy is off.",
                                sealing::MODULE
                            )
                        }
                        None => format!("Encrypted by {}.", sealing::MODULE),
                    });
                } else if draft.was_encrypted {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Saving will write this profile to disk unencrypted.",
                    );
                }
            });
        if let Some(problem) = &draft.problem {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
        ui.horizontal(|ui| {
            save = ui.button("Save").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            if let Some(draft) = self.draft.take() {
                draft.forget(ui.ctx());
            }
        } else if save {
            self.save_draft(ui.ctx());
        }
    }

    fn save_draft(&mut self, ctx: &egui::Context) {
        let (Some(draft), Some(store)) = (&mut self.draft, &mut self.store) else {
            return;
        };
        let profile = draft.profile();
        if profile.name.is_empty() {
            draft.problem = Some("Give the profile a name.".into());
            return;
        }
        let id = draft.id.as_deref();
        let saved = if !draft.encrypt {
            store.save_plain(id, profile)
        } else if draft.passphrase.is_empty() && draft.again.is_empty() && draft.was_encrypted {
            store.save_sealed(id, profile, None)
        } else if draft.passphrase.is_empty() {
            draft.problem = Some("Enter a passphrase to encrypt with.".into());
            return;
        } else if draft.passphrase != draft.again {
            draft.problem = Some("The two passphrases differ.".into());
            return;
        } else if draft.passphrase.chars().count() < sealing::MIN_PASSPHRASE {
            draft.problem = Some(format!(
                "Use a passphrase of at least {} characters.",
                sealing::MIN_PASSPHRASE
            ));
            return;
        } else {
            store.save_sealed(id, profile, Some(&draft.passphrase))
        };
        match saved {
            Ok(id) => {
                if let Some(draft) = self.draft.take() {
                    draft.forget(ctx);
                }
                self.plan = None;
                self.selected = Some(id);
                self.problem = None;
            }
            Err(e) => draft.problem = Some(e.to_string()),
        }
    }
}

/// Text cut to fit a column.
fn short(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(most - 1).collect();
    cut.push('…');
    cut
}

impl Viewer {
    /// Opens or closes the Form profiles window. Closing it locks every
    /// encrypted profile.
    pub(super) fn toggle_profiles(&mut self) {
        let p = &mut self.profiles;
        if p.open {
            p.lock_up(&self.ctx);
            p.open = false;
        } else {
            p.store.get_or_insert_with(Store::load);
            p.open = true;
        }
    }

    pub(super) fn profiles_window(&mut self, ctx: &egui::Context) {
        if !self.profiles.open {
            return;
        }
        self.profiles.lock_idle(ctx);
        let page = self.current_page();
        let doc = self.open.as_ref().map(|o| DocView {
            form: o.form.as_ref().map(|fv| &fv.form),
            session: &o.session,
            generation: o.generation,
            page,
        });
        let mut open = true;
        let mut ask = None;
        let profiles = &mut self.profiles;
        let shown = egui::Window::new("Form profiles")
            .open(&mut open)
            .collapsible(true)
            .resizable(false)
            .default_width(400.0)
            .default_pos(ctx.content_rect().right_top() + Vec2::new(-440.0, 60.0))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ask = profiles.ui(ui, doc.as_ref());
            });
        // Working in the window keeps the profile in use.
        let busy = ctx.input(|i| {
            i.pointer.any_pressed()
                || i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)))
        });
        if busy
            && shown.is_some_and(|s| s.response.contains_pointer())
            && let (Some(store), Some(id)) = (&mut profiles.store, &profiles.selected)
        {
            store.touch(id);
        }
        if !open {
            profiles.lock_up(ctx);
            profiles.open = false;
        }
        match ask {
            Some(Ask::Fill(fills, name)) => self.fill_from_profile(fills, &name),
            Some(Ask::Detect) => self.start_detect(),
            None => {}
        }
    }

    /// Puts a profile's values into the fields they were matched to.
    fn fill_from_profile(&mut self, fills: Vec<(usize, Value)>, name: &str) {
        // Text still being typed into a field goes in first.
        self.finish_editing(true);
        let n = fills.len();
        let scripted = self.open.as_ref().is_some_and(|o| o.scripts.is_some());
        if scripted {
            // Allowed scripts check and reformat each value as if typed.
            for (field, value) in fills {
                self.commit(field, value);
            }
        } else if let Some(o) = &mut self.open {
            let mut pages = HashSet::new();
            for (field, value) in fills {
                o.session.set(field, value);
                pages.extend(o.session.pages_of(field));
            }
            self.rebuild(&pages, false);
        }
        self.notice = Some(match n {
            1 => format!("Filled 1 field from “{name}”."),
            n => format!("Filled {n} fields from “{name}”."),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_keeps_only_what_was_filled_in() {
        let mut d = Draft::new(None);
        assert!(!d.encrypt);
        d.name = "  Home ".into();
        d.standard[0].1.push_str(" Ada Lovelace ");
        d.custom.push(("Member number".into(), "M-4471".into()));
        d.custom.push(("No value".into(), "  ".into()));
        d.custom.push((" ".into(), "no label".into()));
        let p = d.profile();
        assert_eq!(p.name, "Home");
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.standard(Standard::Name), "Ada Lovelace");
        assert_eq!(p.entries[1].label(), "Member number");
        // Text boxes start with room, so typing does not move the text.
        assert!(d.standard.iter().all(|(_, v)| v.capacity() >= ROOM));
        assert!(d.passphrase.capacity() >= ROOM);
    }

    /// A one-page form with a name, a city that is filled in and a
    /// field no profile fills.
    fn form_pdf() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [4 0 R 5 0 R 6 0 R] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [10 150 190 170] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (City) /V (Paris) /Rect [10 120 190 140] >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Date) /Rect [10 90 190 110] >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// Draws the window's contents twice, as egui does to settle sizes.
    /// Nothing is clicked, so nothing is asked of the viewer.
    fn draw(p: &mut Profiles, doc: Option<&DocView<'_>>) {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut asked = false;
            let mut drawn = ctx.run_ui(egui::RawInput::default(), |ui| {
                asked = p.ui(ui, doc).is_some();
            });
            // Nothing is put on a screen, so the font texture goes unused.
            drawn.textures_delta.clear();
            assert!(!asked);
        }
    }

    fn home() -> Profile {
        let mut d = Draft::new(None);
        d.name.push_str("Home");
        for (which, value) in &mut d.standard {
            value.push_str(match which {
                Standard::Name => "Ada Lovelace",
                Standard::City => "London",
                Standard::MailingAddress => "PO Box 12\nLondon",
                _ => "",
            });
        }
        d.profile()
    }

    #[test]
    fn the_window_draws_in_every_state_and_plans_what_it_would_fill() {
        let doc = std::sync::Arc::new(atelier_syntax::Document::open(form_pdf()).unwrap());
        let session = Session::new(doc.clone());
        let form = Form::load(&doc).unwrap().unwrap();
        let view = |form, page| DocView {
            form,
            session: &session,
            generation: 1,
            page,
        };

        // Nothing saved, then a new profile being written.
        let mut p = Profiles::default();
        p.store = Some(Store::for_test("window"));
        draw(&mut p, None);
        p.draft = Some(Draft::new(None));
        draw(&mut p, None);
        let draft = p.draft.as_mut().unwrap();
        assert!(draft.boxes.len() >= Standard::ALL.len());
        // Saving needs a name, and a passphrase typed twice to encrypt.
        let ctx = egui::Context::default();
        p.save_draft(&ctx);
        assert!(p.draft.as_ref().unwrap().problem.is_some());
        let draft = p.draft.as_mut().unwrap();
        draft.name.push_str("Home");
        draft.standard[0].1.push_str("Ada Lovelace");
        draft.custom.push(("Member number".into(), "M-4471".into()));
        draft.encrypt = true;
        draw(&mut p, None);
        p.save_draft(&ctx);
        assert!(p.draft.is_some() && p.selected.is_none());
        let draft = p.draft.as_mut().unwrap();
        draft.passphrase.push_str("correct horse");
        draft.again.push_str("correct horsf");
        p.save_draft(&ctx);
        assert!(p.draft.is_some() && p.selected.is_none());
        let draft = p.draft.as_mut().unwrap();
        draft.again.replace_range(.., "correct horse");
        p.save_draft(&ctx);
        assert!(p.draft.is_none());
        let id = p.selected.clone().unwrap();

        // Unlocked: with no document, a document without a form, and a
        // form, for which the matches are worked out.
        draw(&mut p, None);
        draw(&mut p, Some(&view(None, 0)));
        assert!(p.plan.is_none());
        draw(&mut p, Some(&view(Some(&form), 0)));
        let plan = p.plan.as_ref().unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].label, "Name");
        // Another page has none of the fields; the whole document does.
        draw(&mut p, Some(&view(Some(&form), 1)));
        assert!(p.plan.as_ref().unwrap().items.is_empty());
        p.scope = Scope::Document;
        p.reveal = true;
        draw(&mut p, Some(&view(Some(&form), 1)));
        assert_eq!(p.plan.as_ref().unwrap().items.len(), 1);
        // Editing it starts from what it holds.
        let draft = Draft::new(p.store.as_ref().unwrap().find(&id));
        assert!(draft.encrypt && draft.was_encrypted);
        assert_eq!(draft.standard[0].1, "Ada Lovelace");
        assert_eq!(draft.custom.len(), 1);
        drop(draft);

        // Locking takes the matches and the details away; the window
        // then asks for the passphrase.
        p.lock_up(&ctx);
        assert!(p.plan.is_none());
        assert!(p.store.as_ref().unwrap().find(&id).unwrap().locked());
        draw(&mut p, Some(&view(Some(&form), 0)));
        assert!(p.plan.is_none());
        p.search.push_str("hmoe");
        draw(&mut p, None);
        p.search.replace_range(.., "nothing like it");
        draw(&mut p, None);

        // A plain profile fills more with replacing on.
        p.search.clear();
        let plain = p.store.as_mut().unwrap().save_plain(None, home()).unwrap();
        p.select(Some(plain));
        p.replace = true;
        draw(&mut p, Some(&view(Some(&form), 0)));
        assert_eq!(p.plan.as_ref().unwrap().items.len(), 2);
        p.confirm_delete = true;
        draw(&mut p, Some(&view(Some(&form), 0)));
        p.store.take().unwrap().clean_up();
    }

    #[test]
    fn long_text_is_cut_to_fit() {
        assert_eq!(short("short", 10), "short");
        assert_eq!(short("a longer label", 8), "a longe…");
    }
}
