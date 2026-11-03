use eframe::egui;
use eframe::egui::{
    Align2, CornerRadius, FontId, Response, Sense, StrokeKind, Ui, Vec2,
};

pub enum SplitButtonAction<T> {
    Primary,
    Menu(T),
}
pub fn split_button<T>(
    ui: &mut Ui,
    label: &str,
    menu: impl FnOnce(&mut Ui) -> Option<T>,
) -> Option<SplitButtonAction<T>> {
    let height = ui.spacing().interact_size.y;
    let padding = ui.spacing().button_padding;

    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        font.clone(),
        ui.visuals().text_color(),
    );

    let dropdown_width = 27.0;
    let primary_width = galley.size().x + padding.x * 2.0;

    let total_size = Vec2::new(
        primary_width + dropdown_width,
        height,
    );

    let (rect, _) = ui.allocate_exact_size(total_size, Sense::hover());

    let primary_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.min.x + primary_width, rect.max.y),
    );

    let dropdown_rect = egui::Rect::from_min_max(
        egui::pos2(primary_rect.max.x, rect.min.y),
        rect.max,
    );

    // Independent interaction regions.
    let primary_response = ui.interact(
        primary_rect,
        ui.id().with("split_primary"),
        Sense::click(),
    );

    let dropdown_response = ui.interact(
        dropdown_rect,
        ui.id().with("split_dropdown"),
        Sense::click(),
    );

    let radius = 6;

    let left_radius = CornerRadius {
        nw: radius,
        sw: radius,
        ne: 0,
        se: 0,
    };

    let right_radius = CornerRadius {
        nw: 0,
        sw: 0,
        ne: radius,
        se: radius,
    };

    // Base background.
    let inactive = ui.visuals().widgets.inactive;

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(radius),
        inactive.bg_fill,
    );

    // Primary hover / pressed state.
    let primary_visuals = ui.style().interact(&primary_response);

    if primary_response.hovered() || primary_response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            primary_rect,
            left_radius,
            primary_visuals.bg_fill,
        );
    }

    // Dropdown hover / pressed state.
    let dropdown_visuals = ui.style().interact(&dropdown_response);

    if dropdown_response.hovered() || dropdown_response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            dropdown_rect,
            right_radius,
            dropdown_visuals.bg_fill,
        );
    }

    // Shared outline.
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius),
        inactive.bg_stroke,
        StrokeKind::Inside,
    );

    // Vertical separator.
    let separator_color = inactive.bg_stroke.color;

    ui.painter().line_segment(
        [
            egui::pos2(primary_rect.max.x, rect.min.y + 5.0),
            egui::pos2(primary_rect.max.x, rect.max.y - 5.0),
        ],
        egui::Stroke::new(1.0, separator_color),
    );

    // Primary label.
    ui.painter().text(
        primary_rect.center(),
        Align2::CENTER_CENTER,
        label,
        font,
        primary_visuals.fg_stroke.color,
    );

    // Dropdown indicator.
    ui.painter().text(
        dropdown_rect.center() + egui::vec2(0.0, 2.5),
        Align2::CENTER_CENTER,
        "▾",
        FontId::proportional(12.0),
        dropdown_visuals.fg_stroke.color,
    );

    // Primary action.
    if primary_response.clicked() {
        return Some(SplitButtonAction::Primary);
    }

    // Dropdown menu.
    let mut selected = None;

    egui::Popup::menu(&dropdown_response).show(|ui| {
        selected = menu(ui);

        if selected.is_some() {
            ui.close();
        }
    });

    selected.map(SplitButtonAction::Menu)
    
}
