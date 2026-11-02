//! Platform window chrome: macOS retains traffic lights; Linux retains the
//! compositor's decoration behavior. Windows has a client-drawn titlebar with
//! native viewport commands. ATELIER_NATIVE_TITLEBAR opts out everywhere.
use eframe::egui::{self, CursorIcon, ResizeDirection, Sense, Vec2, ViewportCommand};

pub fn custom_windows() -> bool {
    cfg!(target_os = "windows") && std::env::var_os("ATELIER_NATIVE_TITLEBAR").is_none()
}
pub fn viewport(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if custom_windows() {
        builder.with_decorations(false)
    } else {
        builder
    }
}

pub fn window_buttons(ui: &mut egui::Ui) {
    if !custom_windows() {
        return;
    }
    if ui.button("×").on_hover_text("Close window").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    if ui
        .button(if maximized { "❐" } else { "□" })
        .on_hover_text("Maximize / restore")
        .clicked()
    {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if ui.button("−").on_hover_text("Minimize window").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

pub fn drag_space(ui: &mut egui::Ui) {
    if !custom_windows() {
        return;
    }
    let (_, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(0.0), 28.0),
        Sense::click_and_drag(),
    );
    if response.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if response.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

pub fn resize_edges(ctx: &egui::Context) {
    if !custom_windows()
        || ctx.input(|i| {
            i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
        })
    {
        return;
    }
    let rect = ctx.viewport_rect();
    let t = 5.0;
    let corner = 10.0;
    let edges = [
        (
            ResizeDirection::NorthWest,
            rect.min,
            Vec2::splat(corner),
            CursorIcon::ResizeNwSe,
        ),
        (
            ResizeDirection::NorthEast,
            rect.right_top() - Vec2::new(corner, 0.0),
            Vec2::splat(corner),
            CursorIcon::ResizeNeSw,
        ),
        (
            ResizeDirection::SouthWest,
            rect.left_bottom() - Vec2::new(0.0, corner),
            Vec2::splat(corner),
            CursorIcon::ResizeNeSw,
        ),
        (
            ResizeDirection::SouthEast,
            rect.max - Vec2::splat(corner),
            Vec2::splat(corner),
            CursorIcon::ResizeNwSe,
        ),
        (
            ResizeDirection::North,
            rect.min + Vec2::new(corner, 0.0),
            Vec2::new((rect.width() - 2.0 * corner).max(0.0), t),
            CursorIcon::ResizeVertical,
        ),
        (
            ResizeDirection::South,
            rect.left_bottom() + Vec2::new(corner, -t),
            Vec2::new((rect.width() - 2.0 * corner).max(0.0), t),
            CursorIcon::ResizeVertical,
        ),
        (
            ResizeDirection::West,
            rect.min + Vec2::new(0.0, corner),
            Vec2::new(t, (rect.height() - 2.0 * corner).max(0.0)),
            CursorIcon::ResizeHorizontal,
        ),
        (
            ResizeDirection::East,
            rect.right_top() + Vec2::new(-t, corner),
            Vec2::new(t, (rect.height() - 2.0 * corner).max(0.0)),
            CursorIcon::ResizeHorizontal,
        ),
    ];
    for (index, (direction, position, size, cursor)) in edges.into_iter().enumerate() {
        egui::Area::new(egui::Id::unique(("window-edge", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(ctx, |ui| {
                let (_, response) = ui.allocate_exact_size(size, Sense::drag());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(cursor);
                }
                if response.drag_started() {
                    ui.ctx()
                        .send_viewport_cmd(ViewportCommand::BeginResize(direction));
                }
            });
    }
}
