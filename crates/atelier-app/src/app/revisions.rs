//! Reviewing tracked document transactions independently of comment decisions.
use super::Viewer;
use atelier_form::RevisionState;
use eframe::egui;

#[derive(Default)]
pub(super) struct ReviewUi {
    pub changes: bool,
    selected: Option<u64>,
}
impl Viewer {
    pub(super) fn revision_content(&mut self, ui: &mut egui::Ui) {
        let Some(o) = &self.open else {
            ui.weak("Open a document to track changes.");
            return;
        };
        let mut enabled = o.session.tracking_enabled();
        if ui.checkbox(&mut enabled, "Track changes").changed() {
            self.finish_editing(true);
            if let Some(o) = &mut self.open {
                match o
                    .session
                    .set_tracking(enabled, &self.settings.review_author)
                {
                    Ok(()) => self.rebuild(&Default::default(), false),
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        }
        let active = self
            .open
            .as_ref()
            .is_some_and(|o| o.session.revision_session_active());
        if ui
            .add_enabled(
                enabled,
                egui::Button::new(if active {
                    "Finish editing session"
                } else {
                    "Start editing session"
                }),
            )
            .on_hover_text("Group edits into one revision for review.")
            .clicked()
        {
            self.finish_editing(true);
            if let Some(o) = &mut self.open {
                if active {
                    o.session.end_revision_session();
                } else {
                    o.session.begin_revision_session();
                }
            }
        }
        let Some(o) = &self.open else { return };
        if let Some(problem) = o.session.tracking_problem() {
            ui.colored_label(ui.visuals().warn_fg_color, problem);
        }
        ui.weak("Tracked edits appear immediately. Reject restores the original content; resolve dependent edits from newest to oldest.");
        let revisions = o.session.revisions().to_vec();
        let undo = o.session.revision_undo_id();
        let redo = o.session.revision_redo();
        let mut action = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(undo.is_some(), egui::Button::new("Undo"))
                .on_hover_text("Undo last tracked edit (Ctrl+Z)")
                .clicked()
            {
                action = undo.map(|id| (id, RevisionState::Undone));
            }
            if ui
                .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text("Redo tracked edit (Ctrl+Shift+Z)")
                .clicked()
            {
                action = redo;
            }
        });
        if revisions.is_empty() {
            ui.weak("Enable tracking, then edit a form, mark a page, or arrange pages.");
            return;
        }
        let selected = self
            .review_ui
            .selected
            .and_then(|id| revisions.iter().position(|r| r.id == id))
            .unwrap_or(revisions.len() - 1);
        self.review_ui.selected = Some(revisions[selected].id);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(selected > 0, crate::icons::Icon::Left.button())
                .on_hover_text("Previous revision")
                .clicked()
            {
                self.review_ui.selected = Some(revisions[selected - 1].id);
                if let Some(page) = revisions[selected - 1].changes.iter().find_map(|c| c.page) {
                    self.go_to_page(page);
                }
            }
            ui.label(format!("{} / {}", selected + 1, revisions.len()));
            if ui
                .add_enabled(
                    selected + 1 < revisions.len(),
                    crate::icons::Icon::Right.button(),
                )
                .on_hover_text("Next revision")
                .clicked()
            {
                self.review_ui.selected = Some(revisions[selected + 1].id);
                if let Some(page) = revisions[selected + 1].changes.iter().find_map(|c| c.page) {
                    self.go_to_page(page);
                }
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("revision-list")
            .show(ui, |ui| {
                for revision in revisions.iter().rev() {
                    let selected = self.review_ui.selected == Some(revision.id);
                    let title = format!(
                        "#{} · {} · {}",
                        revision.id,
                        revision.state.label(),
                        revision.author
                    );
                    if ui.selectable_label(selected, title).clicked() {
                        self.review_ui.selected = Some(revision.id);
                        if let Some(page) = revision.changes.iter().find_map(|c| c.page) {
                            self.go_to_page(page);
                        }
                    }
                    if !selected {
                        continue;
                    }
                    ui.weak(super::annotate::display_date(&revision.created));
                    for change in &revision.changes {
                        let title = if let Some((name, identity)) = change.target.split_once(" (") {
                            if let Some((_, property)) = identity.split_once(") / ") {
                                format!("{name} / {property}")
                            } else {
                                name.to_owned()
                            }
                        } else {
                            change.target.clone()
                        };
                        ui.strong(title);
                        ui.label(format!(
                            "Before: {}",
                            if change.before.is_empty() {
                                "—"
                            } else {
                                &change.before
                            }
                        ));
                        ui.label(format!(
                            "After: {}",
                            if change.after.is_empty() {
                                "—"
                            } else {
                                &change.after
                            }
                        ));
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !matches!(
                                    revision.state,
                                    RevisionState::Accepted | RevisionState::Superseded
                                ),
                                egui::Button::new("Accept"),
                            )
                            .clicked()
                        {
                            action = Some((revision.id, RevisionState::Accepted));
                        }
                        if ui
                            .add_enabled(
                                matches!(
                                    revision.state,
                                    RevisionState::Pending | RevisionState::Accepted
                                ),
                                egui::Button::new("Reject"),
                            )
                            .clicked()
                        {
                            action = Some((revision.id, RevisionState::Rejected));
                        }
                    });
                    ui.collapsing("Decision history", |ui| {
                        for event in &revision.events {
                            ui.label(format!(
                                "{} · {} · {}",
                                event.state.label(),
                                event.author,
                                super::annotate::display_date(&event.date)
                            ));
                        }
                    });
                    ui.separator();
                }
            });
        if let Some((id, state)) = action {
            self.apply_revision_decision(id, state);
        }
    }
    pub(super) fn apply_revision_decision(&mut self, id: u64, state: RevisionState) {
        self.finish_editing(true);
        let Some(o) = &mut self.open else { return };
        let author = if self.settings.review_author.trim().is_empty() {
            "Reviewer"
        } else {
            &self.settings.review_author
        };
        match o.session.decide_revision(&o.doc, id, state, author) {
            Ok(()) => {
                let count = o.session.page_count();
                self.rearranged(&vec![None; count], None);
                self.review_ui.selected = Some(id);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    pub(super) fn undo_revision(&mut self) -> bool {
        let id = self
            .open
            .as_ref()
            .and_then(|o| o.session.revision_undo_id());
        if let Some(id) = id {
            self.apply_revision_decision(id, RevisionState::Undone);
            true
        } else {
            false
        }
    }
    pub(super) fn redo_revision(&mut self) {
        if let Some((id, state)) = self.open.as_ref().and_then(|o| o.session.revision_redo()) {
            self.apply_revision_decision(id, state);
        }
    }
}
