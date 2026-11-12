//! Platform window chrome: on macOS the toolbar fills the title bar, with
//! the native traffic lights at its start; Linux retains the compositor's
//! decoration behavior. Windows has a client-drawn titlebar with native
//! viewport commands. ATELIER_NATIVE_TITLEBAR opts out everywhere.
use eframe::egui::{self, CursorIcon, ResizeDirection, Sense, Vec2, ViewportCommand};

fn native_titlebar() -> bool {
    std::env::var_os("ATELIER_NATIVE_TITLEBAR").is_some()
}

pub fn custom_windows() -> bool {
    cfg!(target_os = "windows") && !native_titlebar()
}

/// The window content runs under a transparent macOS title bar, so the
/// toolbar is the title bar.
pub fn integrated_mac() -> bool {
    cfg!(target_os = "macos") && !native_titlebar()
}

pub fn viewport(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if custom_windows() {
        builder.with_decorations(false)
    } else if integrated_mac() {
        builder
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
    } else {
        builder
    }
}

/// Where the toolbar starts, past the traffic lights. They are hidden in
/// full screen, so nothing is kept for them there.
pub fn traffic_light_space(ui: &mut egui::Ui, width: f32) {
    if !integrated_mac() || ui.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let height = ui.spacing().interact_size.y;
    drag_area(ui, Vec2::new(width - ui.spacing().item_spacing.x, height));
}

pub fn window_buttons(ui: &mut egui::Ui) {
    if !custom_windows() {
        return;
    }
    if crate::icons::button(ui, crate::icons::Icon::Close, "Close window")
        .on_hover_text("Close window")
        .clicked()
    {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
    if ui
        .add(
            if maximized {
                crate::icons::Icon::Restore
            } else {
                crate::icons::Icon::Max
            }
            .button(),
        )
        .on_hover_text("Maximize / restore")
        .clicked()
    {
        ui.ctx()
            .send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if crate::icons::button(ui, crate::icons::Icon::Minimize, "Minimize window")
        .on_hover_text("Minimize window")
        .clicked()
    {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

pub fn drag_space(ui: &mut egui::Ui) {
    if !custom_windows() && !integrated_mac() {
        return;
    }
    drag_area(ui, Vec2::new(ui.available_width().max(0.0), 28.0));
}

/// Space that moves the window when dragged and zooms it when double
/// clicked, as a title bar does.
fn drag_area(ui: &mut egui::Ui, size: Vec2) {
    let (_, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
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
