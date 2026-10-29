//! The Colours window: an eyedropper that says what colour a point of the
//! page is and what painted it (colour space, values, spot colour name),
//! and the list of spot colours the file uses.

use atelier_render::{Probed, SpotColor};
use eframe::egui::{self, Color32, CursorIcon, RichText, Sense, Stroke, StrokeKind, Vec2};

use super::Viewer;

#[derive(Default)]
pub(super) struct Colors {
    pub open: bool,
    /// Clicking the page picks a colour.
    pub picking: bool,
    /// A pick is being worked out.
    pub waiting: bool,
    /// The last pick: its page and what was found.
    pub last: Option<(usize, Result<Probed, String>)>,
    /// The file's spot colours, once listed.
    pub spots: Option<Vec<SpotColor>>,
    pub spots_asked: bool,
}

impl Colors {
    /// Forgets everything about the last file.
    pub fn reset(&mut self) {
        let open = self.open;
        *self = Self {
            open,
            picking: open,
            ..Self::default()
        };
    }
}

impl Viewer {
    /// Opens or closes the Colours window; opening it starts picking.
    pub(super) fn toggle_colors(&mut self) {
        let c = &mut self.colors;
        c.open = !c.open;
        c.picking = c.open;
    }

    /// While picking: a click on a page asks for the colour there.
    /// Returns whether picking took the pointer.
    pub(super) fn color_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        at: Option<(f64, f64)>,
    ) -> bool {
        if !self.colors.open || !self.colors.picking {
            return false;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.colors.picking = false;
            return true;
        }
        let Some(o) = &self.open else { return false };
        let Some((page, px, py)) = at.and_then(|(lx, ly)| o.layout.page_at(lx, ly)) else {
            return true;
        };
        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        if response.clicked() {
            // Pick at the screen's own resolution, so the colour is the one
            // under the pointer.
            let ppp = f64::from(ui.ctx().pixels_per_point());
            let scale = o.layout.zoom() * ppp;
            let xy = ((px * ppp).max(0.0) as usize, (py * ppp).max(0.0) as usize);
            let wake = self.ctx.clone();
            self.pool
                .probe(o.doc.clone(), o.generation, page, scale, xy, move || {
                    wake.request_repaint()
                });
            self.colors.waiting = true;
        }
        true
    }

    pub(super) fn colors_window(&mut self, ctx: &egui::Context) {
        if !self.colors.open {
            return;
        }
        if !self.colors.spots_asked
            && let Some(o) = &self.open
        {
            self.colors.spots_asked = true;
            let wake = self.ctx.clone();
            self.pool
                .spots(o.doc.clone(), o.opened, move || wake.request_repaint());
        }
        let mut open = true;
        egui::Window::new("Colours")
            .open(&mut open)
            .collapsible(true)
            .resizable(false)
            .default_width(300.0)
            .default_pos(ctx.content_rect().right_top() + Vec2::new(-330.0, 60.0))
            .show(ctx, |ui| self.colors_ui(ui));
        if !open {
            self.colors.open = false;
            self.colors.picking = false;
        }
    }

    fn colors_ui(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.colors;
        if ui
            .add(egui::Button::selectable(c.picking, "⌖ Pick a colour"))
            .on_hover_text("Click anywhere on a page (Esc stops)")
            .clicked()
        {
            c.picking = !c.picking;
        }
        ui.add_space(4.0);
        if c.waiting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Looking…");
            });
        } else {
            match &c.last {
                None => {
                    ui.label(RichText::new("Click a page to see its colour there.").weak());
                }
                Some((_, Err(e))) => {
                    ui.colored_label(Color32::from_rgb(220, 90, 70), e);
                }
                Some((page, Ok(p))) => picked_ui(ui, *page, p),
            }
        }
        ui.separator();
        ui.label(RichText::new("Spot colours in this file").strong());
        match &c.spots {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Listing…");
                });
            }
            Some(spots) if spots.is_empty() => {
                ui.label(RichText::new("None: everything is in process colours.").weak());
            }
            Some(spots) => {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for s in spots {
                            ui.horizontal(|ui| {
                                let rgb = s.rgb.map(|c| to_rgb8(c.r, c.g, c.b));
                                swatch(ui, rgb, 16.0);
                                ui.label(&s.name).on_hover_text(&s.space);
                            });
                        }
                    });
            }
        }
    }
}

/// What a pick found: a swatch and its values, then what painted it.
fn picked_ui(ui: &mut egui::Ui, page: usize, p: &Probed) {
    let [r, g, b] = p.rgb;
    let hex = format!("#{r:02X}{g:02X}{b:02X}");
    ui.horizontal(|ui| {
        swatch(ui, Some([r, g, b]), 40.0);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&hex).monospace().size(15.0));
                if ui
                    .small_button("Copy")
                    .on_hover_text("Copy the hex code")
                    .clicked()
                {
                    ui.ctx().copy_text(hex.clone());
                }
            });
            ui.label(format!("RGB {r}, {g}, {b} as shown on page {}", page + 1));
        });
    });
    ui.add_space(4.0);
    let Some(paint) = &p.paint else {
        ui.label(RichText::new("Nothing is drawn here: this is the paper.").weak());
        return;
    };
    egui::Grid::new("picked")
        .num_columns(2)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            ui.label("Drawn by");
            ui.label(paint.kind.label());
            ui.end_row();
            ui.label("Colour space");
            ui.label(&paint.space);
            ui.end_row();
            if !paint.spots.is_empty() {
                ui.label("Spot colour");
                ui.label(RichText::new(paint.spots.join(", ")).strong());
                ui.end_row();
            }
            if !paint.components.is_empty() {
                ui.label("Values");
                ui.label(values_text(
                    &paint.space,
                    &paint.components,
                    &paint.colorants,
                ));
                ui.end_row();
            }
            if let Some((alt, values)) = &paint.alternate {
                ui.label("Printed as");
                ui.label(format!("{alt}: {}", values_text(alt, values, &[])));
                ui.end_row();
            }
        });
}

/// A colour's components in the usual units for its space.
fn values_text(space: &str, c: &[f32], colorants: &[String]) -> String {
    let pct = |v: f32| format!("{:.0}%", v * 100.0);
    if space.starts_with("Separation") || space.starts_with("DeviceN") {
        let names: Vec<&str> = colorants.iter().map(String::as_str).collect();
        return c
            .iter()
            .enumerate()
            .map(|(i, v)| match names.get(i) {
                Some(n) if c.len() > 1 => format!("{n} {}", pct(*v)),
                _ => format!("tint {}", pct(*v)),
            })
            .collect::<Vec<_>>()
            .join(", ");
    }
    match c.len() {
        4 => format!(
            "C {} M {} Y {} K {}",
            pct(c[0]),
            pct(c[1]),
            pct(c[2]),
            pct(c[3])
        ),
        3 if space == "Lab" => format!("L {:.0} a {:.0} b {:.0}", c[0], c[1], c[2]),
        3 => {
            let [r, g, b] = to_rgb8(c[0], c[1], c[2]);
            format!("R {r} G {g} B {b}")
        }
        1 if space.starts_with("Indexed") => format!("entry {}", c[0] as i64),
        1 => format!("grey {}", pct(c[0])),
        _ => c
            .iter()
            .map(|v| format!("{v:.3}"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn to_rgb8(r: f32, g: f32, b: f32) -> [u8; 3] {
    [r, g, b].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn swatch(ui: &mut egui::Ui, rgb: Option<[u8; 3]>, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    match rgb {
        Some([r, g, b]) => {
            p.rect_filled(rect, 3.0, Color32::from_rgb(r, g, b));
        }
        None => {
            p.line_segment(
                [rect.left_bottom(), rect.right_top()],
                Stroke::new(1.0, Color32::GRAY),
            );
        }
    }
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_in_their_units() {
        assert_eq!(
            values_text("DeviceCMYK", &[0.0, 0.91, 0.76, 0.0], &[]),
            "C 0% M 91% Y 76% K 0%"
        );
        assert_eq!(
            values_text("DeviceRGB", &[1.0, 0.0, 0.5], &[]),
            "R 255 G 0 B 128"
        );
        assert_eq!(
            values_text(
                "Separation PANTONE 185 C (alternate DeviceCMYK)",
                &[0.5],
                &["PANTONE 185 C".into()]
            ),
            "tint 50%"
        );
        assert_eq!(
            values_text(
                "DeviceN Cyan, Gold (alternate DeviceCMYK)",
                &[0.2, 1.0],
                &["Cyan".into(), "Gold".into()]
            ),
            "Cyan 20%, Gold 100%"
        );
    }
}
