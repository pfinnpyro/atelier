//! Right-click menus on the page: copying text, and cutting, copying and
//! pasting field values.

use atelier_form::{FieldKind, Value, flags};
use atelier_view::{Caret, Selection, word_at};
use eframe::egui;

use super::Viewer;

/// What the open menu is for.
#[derive(Clone, Copy)]
pub(super) enum MenuTarget {
    Text {
        page: usize,
        at: atelier_raster::Point,
    },
    Field((usize, usize)),
    /// A mark added since the file was opened or saved.
    Mark(usize),
    SavedMark {
        page: usize,
        ordinal: usize,
    },
}

/// The clipboard's text, read when Paste is picked from a menu (egui only
/// sees the clipboard when Ctrl+V is pressed).
pub(super) fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// What a field's text becomes when pasted into: one line unless the field
/// takes several, and no longer than it allows.
fn fit_text(text: &str, multiline: bool, max_len: Option<usize>) -> String {
    let text = if multiline {
        text.replace("\r\n", "\n")
    } else {
        text.lines().collect::<Vec<_>>().join(" ")
    };
    match max_len {
        Some(m) => text.chars().take(m).collect(),
        None => text,
    }
}

impl Viewer {
    /// A right click on the page: picks what the menu is for. Text under
    /// the pointer is selected first unless it is already in the selection.
    pub(super) fn menu_target_at(&mut self, lx: f64, ly: f64) {
        self.commit_ink();
        if let Some(i) = self.mark_at(lx, ly) {
            self.menu = Some(MenuTarget::Mark(i));
            return;
        }
        if let Some((page, ordinal)) = self.saved_mark_at(lx, ly) {
            self.menu = Some(MenuTarget::SavedMark { page, ordinal });
            return;
        }
        if let Some(at) = self.widget_at(lx, ly) {
            self.finish_editing(true);
            if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
                fv.focus = Some(at);
            }
            self.menu = Some(MenuTarget::Field(at));
            return;
        }
        let Some(o) = &self.open else { return };
        let Some((page, _, _)) = o.layout.page_at(lx, ly) else {
            return;
        };
        if self.over_text(lx, ly) {
            let in_selection = self.caret_at(lx, ly).is_some_and(|c| {
                self.selection
                    .is_some_and(|s| !s.is_empty() && s.start() <= c && c < s.end())
            });
            if !in_selection
                && let (Some(text), Some(user)) =
                    (o.text.get(&page), o.layout.to_user(page, lx, ly))
            {
                self.selection = word_at(text, page, user);
            }
        }
        if let Some(at) = o.layout.to_user(page, lx, ly) {
            self.menu = Some(MenuTarget::Text { page, at });
        }
    }

    /// The menu's items.
    pub(super) fn context_menu(&mut self, ui: &mut egui::Ui) {
        match self.menu {
            Some(MenuTarget::Text { page, at }) => self.text_menu(ui, page, at),
            Some(MenuTarget::Field(at)) => self.field_menu(ui, at),
            Some(MenuTarget::SavedMark { page, ordinal }) => {
                if ui.button("Edit comment…").clicked() {
                    self.edit_saved_comment(page, ordinal);
                    ui.close();
                }
                if self.saved_ink(page, ordinal).is_some() {
                    if ui.button("Edit drawing…").clicked() {
                        self.edit_saved_ink(page, ordinal, false);
                        ui.close();
                    }
                    if ui.button("Redraw…").clicked() {
                        self.edit_saved_ink(page, ordinal, true);
                        ui.close();
                    }
                }
                if ui.button("Remove this annotation").clicked() {
                    self.remove_saved_mark(page, ordinal);
                    ui.close();
                }
            }
            Some(MenuTarget::Mark(i)) => {
                let mark = self.open.as_ref().and_then(|o| {
                    o.session
                        .added_annots()
                        .get(i)
                        .map(|(page, annot)| (*page, (*annot).clone()))
                });
                if let Some((page, annot)) = mark {
                    if matches!(annot.markup, atelier_form::Markup::Ink { .. }) {
                        if ui.button("Edit drawing…").clicked() {
                            self.edit_ink(i, false);
                            ui.close();
                        }
                        if ui.button("Redraw…").clicked() {
                            self.edit_ink(i, true);
                            ui.close();
                        }
                        ui.separator();
                    }
                    if let Some(review) = annot.review() {
                        if ui.button("Edit comment…").clicked() {
                            self.edit_added_comment(i);
                            ui.close();
                        }
                        ui.strong(&review.author);
                        ui.weak(super::annotate::display_date(&review.created));
                        if ui.button("Copy comment text").clicked() {
                            ui.ctx().copy_text(review.text.clone());
                            ui.close();
                        }
                    }
                    if ui.button("Add comment in this area…").clicked() {
                        let rect = annot.rect();
                        self.comment_here(page, atelier_raster::Point::new(rect[0], rect[3]));
                        ui.close();
                    }
                    ui.separator();
                }
                if ui.button("Remove this mark").clicked() {
                    self.remove_mark(i);
                    ui.close();
                }
            }
            None => {
                ui.close();
            }
        }
    }

    fn text_menu(&mut self, ui: &mut egui::Ui, page: usize, at: atelier_raster::Point) {
        let has_selection = self.selection.is_some_and(|s| !s.is_empty());
        if ui
            .add_enabled(
                has_selection,
                egui::Button::new("Copy").shortcut_text("Ctrl+C"),
            )
            .clicked()
        {
            self.copy_selection();
            ui.close();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("Highlight"))
            .clicked()
        {
            self.highlight_selection();
            ui.close();
        }
        for (label, strike) in [("Underline", false), ("Strike out", true)] {
            if ui
                .add_enabled(has_selection, egui::Button::new(label))
                .clicked()
            {
                self.review_selection(Some(strike));
                ui.close();
            }
        }
        if ui
            .button(if has_selection {
                "Comment on selected text…"
            } else {
                "Add comment here…"
            })
            .clicked()
        {
            self.comment_here(page, at);
            ui.close();
        }
        ui.menu_button("Review tools", |ui| {
            self.mark_menu(ui);
        });
        ui.separator();
        let glyphs = self
            .open
            .as_ref()
            .and_then(|o| o.text.get(&page))
            .map_or(0, |t| t.glyphs.len());
        if ui
            .add_enabled(glyphs > 0, egui::Button::new("Select all on this page"))
            .clicked()
        {
            self.selection = Some(Selection {
                anchor: Caret { page, index: 0 },
                focus: Caret {
                    page,
                    index: glyphs,
                },
            });
            ui.close();
        }
        ui.separator();
        if ui.button("Rotate this page clockwise").clicked() {
            self.rotate_page(page, 1);
            ui.close();
        }
        if ui.button("Rotate this page counterclockwise").clicked() {
            self.rotate_page(page, -1);
            ui.close();
        }
    }

    fn field_menu(&mut self, ui: &mut egui::Ui, at: (usize, usize)) {
        let Some(o) = &self.open else { return };
        let Some(f) = o.form.as_ref().and_then(|fv| fv.form.fields.get(at.0)) else {
            return;
        };
        let typed =
            f.kind == FieldKind::Text || f.kind == FieldKind::ComboBox && f.has_flag(flags::EDIT);
        let value = f.display_value();
        let has_value = !f.value.same_as(&Value::None);
        let multiline = f.has_flag(flags::MULTILINE);
        let max_len = f.max_len.filter(|&m| m > 0);
        let original = o
            .session
            .original()
            .and_then(|form| form.fields.get(at.0))
            .map(|f| f.value.clone())
            .unwrap_or_default();
        let changed = !f.value.same_as(&original);
        let empty = if typed {
            Value::Text(String::new())
        } else {
            Value::None
        };
        let sized = matches!(
            f.kind,
            FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox
        );
        let size = o
            .session
            .field_style(at.0)
            .map_or(0.0, |s| s.appearance.size);
        let mark = (f.kind == FieldKind::Checkbox).then(|| o.session.check_mark(at.0));

        if typed
            && ui
                .add_enabled(has_value, egui::Button::new("Cut"))
                .clicked()
        {
            ui.ctx().copy_text(value.clone());
            self.commit(at.0, empty.clone());
            ui.close();
        }
        if ui
            .add_enabled(has_value, egui::Button::new("Copy"))
            .clicked()
        {
            ui.ctx().copy_text(value);
            ui.close();
        }
        if typed && ui.button("Paste").clicked() {
            match clipboard_text() {
                Some(t) => self.commit(at.0, Value::Text(fit_text(&t, multiline, max_len))),
                None => self.notice = Some("The clipboard has no text to paste.".into()),
            }
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(has_value, egui::Button::new("Clear"))
            .clicked()
        {
            self.commit(at.0, empty);
            ui.close();
        }
        if ui
            .add_enabled(changed, egui::Button::new("Undo changes to this field"))
            .clicked()
        {
            self.commit(at.0, original);
            ui.close();
        }
        if sized {
            ui.separator();
            let mut picked = None;
            ui.menu_button("Text size", |ui| {
                for option in std::iter::once(0.0).chain(super::detect::SIZES) {
                    if ui
                        .radio(size == option, super::detect::size_label(option))
                        .clicked()
                    {
                        picked = Some(option);
                        ui.close();
                    }
                }
            });
            if let Some(size) = picked {
                self.set_text_size(at.0, size);
            }
        }
        if let Some(mark) = mark {
            ui.separator();
            let mut picked = None;
            ui.menu_button("Mark when ticked", |ui| {
                for option in atelier_form::CheckMark::ALL {
                    if ui.radio(mark == Some(option), option.label()).clicked() {
                        picked = Some(option);
                        ui.close();
                    }
                }
            });
            if let Some(mark) = picked {
                self.set_mark(at.0, mark);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fit_text;

    #[test]
    fn pasted_text_fits_the_field() {
        assert_eq!(fit_text("a\nb", false, None), "a b");
        assert_eq!(fit_text("a\r\nb", true, None), "a\nb");
        assert_eq!(fit_text("abcdef", false, Some(3)), "abc");
    }
}
