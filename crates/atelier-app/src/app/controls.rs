//! Shared, theme-derived desktop controls. Native widgets keep keyboard and
//! accessibility semantics; the connected geometry is drawn as one surface.
use eframe::egui::{self, CornerRadius, Response, Stroke, Vec2};

pub(super) const HEIGHT: f32 = 30.0;
use crate::settings::ButtonDisplay;

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Open,
    Save,
    FitWidth,
    FitPage,
    SideBySide,
    Organize,
}

fn icon(ui: &egui::Ui, rect: egui::Rect, kind: Icon, color: egui::Color32) {
    let r = egui::Rect::from_center_size(rect.center(), Vec2::splat(16.0));
    let p = ui.painter();
    let stroke = Stroke::new(1.5, color);
    let line = |a: [f32; 2], b: [f32; 2]| {
        p.line_segment(
            [r.min + Vec2::new(a[0], a[1]), r.min + Vec2::new(b[0], b[1])],
            stroke,
        )
    };
    match kind {
        Icon::Open => {
            p.rect_stroke(
                r.shrink2(Vec2::new(0.0, 3.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line([1.0, 4.0], [1.0, 1.0]);
            line([1.0, 1.0], [7.0, 1.0]);
            line([7.0, 1.0], [9.0, 4.0]);
        }
        Icon::Save => {
            p.rect_stroke(r, 2.0, stroke, egui::StrokeKind::Inside);
            p.rect_stroke(
                r.shrink2(Vec2::new(4.0, 0.0)).with_max_y(r.top() + 6.0),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_stroke(
                egui::Rect::from_min_max(r.min + Vec2::new(4.0, 10.0), r.max - Vec2::new(4.0, 1.0)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::FitWidth => {
            line([0.0, 1.0], [0.0, 15.0]);
            line([16.0, 1.0], [16.0, 15.0]);
            line([3.0, 8.0], [13.0, 8.0]);
            line([3.0, 8.0], [6.0, 5.0]);
            line([3.0, 8.0], [6.0, 11.0]);
            line([13.0, 8.0], [10.0, 5.0]);
            line([13.0, 8.0], [10.0, 11.0]);
        }
        Icon::FitPage => {
            p.rect_stroke(
                r.shrink2(Vec2::new(3.0, 0.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line([6.0, 5.0], [10.0, 5.0]);
            line([6.0, 8.0], [10.0, 8.0]);
        }
        Icon::SideBySide => {
            p.rect_stroke(r, 1.0, stroke, egui::StrokeKind::Inside);
            line([8.0, 0.0], [8.0, 16.0]);
        }
        Icon::Organize => {
            for y in [0.0, 9.0] {
                for x in [0.0, 9.0] {
                    p.rect_stroke(
                        egui::Rect::from_min_size(r.min + Vec2::new(x, y), Vec2::splat(6.0)),
                        1.0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
    }
}

fn content_width(ui: &egui::Ui, label: &str, display: ButtonDisplay) -> f32 {
    let text = ui
        .painter()
        .layout_no_wrap(
            label.into(),
            egui::FontId::proportional(13.0),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    match display {
        ButtonDisplay::IconOnly => 32.0,
        ButtonDisplay::TextOnly => text + 20.0,
        ButtonDisplay::IconAndText => text + 44.0,
    }
}
fn content(
    ui: &egui::Ui,
    rect: egui::Rect,
    label: &str,
    kind: Icon,
    display: ButtonDisplay,
    color: egui::Color32,
) {
    if display != ButtonDisplay::TextOnly {
        icon(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(rect.min.x + 16.0, rect.center().y),
                Vec2::splat(16.0),
            ),
            kind,
            color,
        );
    }
    if display != ButtonDisplay::IconOnly {
        let x = rect.min.x
            + if display == ButtonDisplay::IconAndText {
                32.0
            } else {
                10.0
            };
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            color,
        );
    }
}
fn response(ui: &mut egui::Ui, rect: egui::Rect, id: egui::Id, label: &str) -> Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    response
}

pub(super) fn toolbar_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: Icon,
    display: ButtonDisplay,
    selected: bool,
) -> Response {
    let width = content_width(ui, label, display);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, HEIGHT), egui::Sense::hover());
    let response = response(ui, rect, ui.auto_id_with(label), label);
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label)
    });
    let visuals = ui.style().interact_selectable(&response, selected);
    let fill = if selected {
        ui.visuals().selection.bg_fill
    } else {
        visuals.weak_bg_fill
    };
    ui.painter().rect(
        rect,
        6.0,
        fill,
        Stroke::new(1.0, visuals.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    content(ui, rect, label, kind, display, visuals.text_color());
    response.on_hover_text(label)
}

/// Fixed geometry: hover changes only paint, never margins or border width.
pub(super) fn split_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: Icon,
    display: ButtonDisplay,
    menu_help: &str,
) -> (Response, Response) {
    let width = content_width(ui, label, display);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width + 25.0, HEIGHT), egui::Sense::hover());
    let main_rect = rect.with_max_x(rect.min.x + width);
    let arrow_rect = rect.with_min_x(main_rect.max.x);
    let main = response(ui, main_rect, ui.auto_id_with((label, "primary")), label);
    let arrow = response(ui, arrow_rect, ui.auto_id_with((label, "menu")), menu_help);
    let base = ui.visuals().widgets.inactive;
    let main_visual = ui.style().interact(&main);
    let arrow_visual = ui.style().interact(&arrow);
    ui.painter().rect_filled(
        main_rect,
        CornerRadius {
            nw: 6,
            sw: 6,
            ne: 0,
            se: 0,
        },
        main_visual.weak_bg_fill,
    );
    ui.painter().rect_filled(
        arrow_rect,
        CornerRadius {
            nw: 0,
            sw: 0,
            ne: 6,
            se: 6,
        },
        arrow_visual.weak_bg_fill,
    );
    ui.painter().rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, base.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    ui.painter().line_segment(
        [
            arrow_rect.left_top() + Vec2::new(0.0, 6.0),
            arrow_rect.left_bottom() - Vec2::new(0.0, 6.0),
        ],
        Stroke::new(1.0, base.bg_stroke.color),
    );
    content(
        ui,
        main_rect,
        label,
        kind,
        display,
        main_visual.text_color(),
    );
    let c = arrow_rect.center();
    ui.painter().add(egui::Shape::line(
        vec![
            c + Vec2::new(-3.0, -1.5),
            c + Vec2::new(0.0, 1.5),
            c + Vec2::new(3.0, -1.5),
        ],
        Stroke::new(1.5, arrow_visual.text_color()),
    ));
    (main, arrow.on_hover_text(menu_help))
}

/// A single tab surface; the close target is embedded rather than a sibling button.
pub(super) fn document_tab(
    ui: &mut egui::Ui,
    id: usize,
    label: &str,
    selected: bool,
) -> (Response, Response) {
    let font = egui::FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), font, ui.visuals().text_color());
    let width = (galley.size().x + 52.0).clamp(110.0, 280.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 32.0), egui::Sense::hover());
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 17.0, rect.center().y),
        Vec2::splat(24.0),
    );
    let tab = response(
        ui,
        rect.with_max_x(close_rect.left()),
        ui.auto_id_with(("document-tab", id)),
        label,
    );
    tab.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Tab, ui.is_enabled(), selected, label)
    });
    let close = response(
        ui,
        close_rect,
        ui.auto_id_with(("close-tab", id)),
        "Close tab",
    );
    let fill = if selected {
        ui.visuals().widgets.active.weak_bg_fill
    } else if tab.hovered() || close.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    if selected {
        ui.painter().line_segment(
            [
                rect.left_bottom() + Vec2::new(6.0, -1.0),
                rect.right_bottom() - Vec2::new(6.0, 1.0),
            ],
            Stroke::new(2.0, ui.visuals().selection.bg_fill),
        );
    }
    ui.painter_at(rect.with_max_x(close_rect.left() - 4.0))
        .galley(
            egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
            galley,
            ui.visuals().text_color(),
        );
    if close.hovered() || close.has_focus() {
        ui.painter()
            .rect_filled(close_rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    crate::icons::Icon::Close
        .image()
        .tint(ui.style().interact(&close).text_color())
        .paint_at(
            ui,
            egui::Rect::from_center_size(close_rect.center(), Vec2::splat(12.0)),
        );
    (tab, close.on_hover_text("Close tab (Ctrl+W)"))
}

pub(super) fn style(ctx: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = Vec2::new(8.0, 7.0);
            style.spacing.button_padding = Vec2::new(10.0, 5.0);
            style.spacing.interact_size.y = 28.0;
            style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.active.corner_radius = CornerRadius::same(6);
            style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);
        });
    }
}

pub(super) fn parse_zoom(text: &str) -> Option<f64> {
    let value = text
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse::<f64>()
        .ok()?;
    (value.is_finite() && (10.0..=800.0).contains(&value)).then_some(value / 100.0)
}

#[cfg(test)]
mod tests {
    use super::parse_zoom;
    #[test]
    fn custom_zoom_rejects_invalid_values() {
        assert_eq!(parse_zoom(" 125.5% "), Some(1.255));
        assert_eq!(parse_zoom("10"), Some(0.1));
        assert_eq!(parse_zoom("800%"), Some(8.0));
        for value in ["NaN", "inf", "0", "-20", "801", "hello", ""] {
            assert_eq!(parse_zoom(value), None);
        }
    }
    #[test]
    fn split_hover_preserves_bounds_and_following_controls_in_every_mode() {
        use super::*;
        let ctx = egui::Context::default();
        for display in [
            ButtonDisplay::IconOnly,
            ButtonDisplay::IconAndText,
            ButtonDisplay::TextOnly,
        ] {
            let mut positions = Vec::new();
            for step in 0..4 {
                let pointer = match step {
                    1 => positions
                        .first()
                        .map(|(main, _, _): &(egui::Rect, egui::Rect, egui::Rect)| main.center()),
                    2 => positions.first().map(|(_, arrow, _)| arrow.center()),
                    _ => None,
                };
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::new(800.0, 200.0),
                        )),
                        events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
                        ..Default::default()
                    },
                    |ui| {
                        ui.horizontal(|ui| {
                            let (main, arrow) =
                                split_button(ui, "Open", Icon::Open, display, "Recent files");
                            let next =
                                toolbar_button(ui, "Fit page", Icon::FitPage, display, false);
                            assert_eq!(main.rect.height(), HEIGHT);
                            assert_eq!(arrow.rect.height(), HEIGHT);
                            assert_eq!(main.rect.right(), arrow.rect.left());
                            assert_eq!(main.rect.center().y, next.rect.center().y);
                            positions.push((main.rect, arrow.rect, next.rect));
                        });
                    },
                );
                output.textures_delta.clear();
            }
            assert!(
                positions.windows(2).all(|pair| pair[0] == pair[1]),
                "hover changed geometry in {display:?}"
            );
        }
    }
}
