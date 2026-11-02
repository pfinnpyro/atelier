//! Embedded SVG controls follow the button text color.
use eframe::egui::{self, Vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Settings,
    Comments,
    Find,
    Menu,
    Left,
    Right,
    Minus,
    Plus,
    Minimize,
    Max,
    Restore,
    Close,
    Sidebar,
    SidebarRight,
}

impl Icon {
    pub fn image(self) -> egui::Image<'static> {
        let source = match self {
            Self::Find => egui::include_image!("../assets/icons/find.svg"),
            Self::Menu => egui::include_image!("../assets/icons/menu.svg"),
            Self::Comments => egui::include_image!("../assets/icons/comments.svg"),
            Self::Settings => egui::include_image!("../assets/icons/settings.svg"),
            Self::Left => egui::include_image!("../assets/icons/left.svg"),
            Self::Right => egui::include_image!("../assets/icons/right.svg"),
            Self::Minus => egui::include_image!("../assets/icons/minus.svg"),
            Self::Plus => egui::include_image!("../assets/icons/plus.svg"),
            Self::Minimize => egui::include_image!("../assets/icons/minimize.svg"),
            Self::Max => egui::include_image!("../assets/icons/max.svg"),
            Self::Restore => egui::include_image!("../assets/icons/restore.svg"),
            Self::Close => egui::include_image!("../assets/icons/close.svg"),
            Self::Sidebar => egui::include_image!("../assets/icons/sidebar.svg"),
            Self::SidebarRight => egui::include_image!("../assets/icons/sidebar-right.svg"),
        };
        let label = match self {
            Self::Settings => "Settings",
            Self::Comments => "Toggle comments pane",
            Self::Find => "Find",
            Self::Menu => "Menu",
            Self::Left => "Previous page",
            Self::Right => "Next page",
            Self::Minus => "Zoom out",
            Self::Plus => "Zoom in",
            Self::Minimize => "Minimize window",
            Self::Max => "Maximize window",
            Self::Restore => "Restore window",
            Self::Close => "Close",
            Self::Sidebar => "Page pictures and outline",
            Self::SidebarRight => "Inspector sidebar",
        };
        egui::Image::new(source)
            .alt_text(label)
            .fit_to_exact_size(Vec2::splat(16.0))
    }
    pub fn button(self) -> egui::Button<'static> {
        egui::Button::new(self.image())
            .image_tint_follows_text_color(true)
            .min_size(Vec2::new(30.0, 30.0))
    }
}
pub fn button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    let response = ui.add(icon.button());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    response.on_hover_text(label)
}
