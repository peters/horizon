use super::super::{HorizonApp, TOOLBAR_HEIGHT, util};

impl HorizonApp {
    pub(super) fn render_cast_notification(&mut self, ctx: &egui::Context) {
        let Some(message) = self.casting.notification.as_deref() else {
            return;
        };
        let viewport = util::viewport_local_rect(ctx);
        let width = (viewport.width() - 24.0).max(0.0);
        let mut dismissed = false;
        // The toolbar is outside every valid capture source; notices never cover a cast image.
        egui::Area::new(egui::Id::new("cast_notification"))
            .fixed_pos(viewport.min + egui::vec2(12.0, 8.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                ui.set_width(width);
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(egui::Stroke::new(1.0, ui.visuals().error_fg_color))
                    .corner_radius(6)
                    .inner_margin(4)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2((width - 50.0).max(0.0), TOOLBAR_HEIGHT - 24.0),
                                egui::Label::new(egui::RichText::new(message).color(ui.visuals().error_fg_color))
                                    .truncate(),
                            );
                            dismissed = ui.small_button("×").clicked();
                        });
                    });
            });
        if dismissed {
            self.casting.notification = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    #[test]
    fn notice_remains_visible_with_picker_closed_and_outside_capture_canvas() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.casting.notify("Synthetic encoder failure".into());
        assert!(!app.casting.picker_open());
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
            ..egui::RawInput::default()
        };
        let _output = ctx
            .run_ui(input, |ui| app.render_cast_notification(ui.ctx()))
            .discard_textures();
        let area = ctx
            .memory(|m| m.area_rect(egui::Id::new("cast_notification")))
            .expect("notice shown");
        assert!(
            area.bottom() <= TOOLBAR_HEIGHT,
            "notice must remain outside capture canvas: {area:?}"
        );
        assert!(app.casting.notification.is_some());
    }
}
