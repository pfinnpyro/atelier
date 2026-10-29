//! Right-click menus on the page: copying text, and cutting, copying and
//! pasting field values.

use atelier_form::{FieldKind, Value, flags};
use atelier_view::{Caret, Selection, word_at};
use eframe::egui;

use super::Viewer;

/// What the open menu is for.
#[derive(Clone, Copy)]
pub(super) enum MenuTarget {
    Text { page: usize },
    Field((usize, usize)),
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
        if let Some(at) = self.widget_at(lx, ly) {
            self.finish_editing(true);
            if let Some(fv) = self.open.as_mut().and_then(|o| o.form.as_mut()) {
                fv.focus = Some(at);
            }
            self.menu = Some(MenuTarget::Field(at));
            return;
        }
        let Some(o) = &self.open else { return };
        let page = o.layout.page_at_y(ly);
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
        self.menu = Some(MenuTarget::Text { page });
    }

    /// The menu's items.
    pub(super) fn context_menu(&mut self, ui: &mut egui::Ui) {
        match self.menu {
            Some(MenuTarget::Text { page }) => self.text_menu(ui, page),
            Some(MenuTarget::Field(at)) => self.field_menu(ui, at),
            None => {
                ui.close();
            }
        }
    }

    fn text_menu(&mut self, ui: &mut egui::Ui, page: usize) {
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
