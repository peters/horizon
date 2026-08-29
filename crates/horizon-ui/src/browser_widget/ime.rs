use egui::{Id, Rect, Ui, pos2, vec2};

#[derive(Clone, Copy, Default)]
struct BrowserImeState;

pub(super) fn publish_page_ime_output(ui: &Ui, body_id: Id, body_rect: Rect) {
    ui.data_mut(|data| {
        data.insert_temp(body_id, BrowserImeState);
    });
    let to_global = ui.ctx().layer_transform_to_global(ui.layer_id()).unwrap_or_default();
    let cursor_rect = Rect::from_min_size(pos2(body_rect.min.x, body_rect.min.y), vec2(1.0, 1.0));
    ui.ctx().output_mut(|output| {
        output.ime = Some(egui::output::IMEOutput {
            purpose: egui::IMEPurpose::Normal,
            rect: to_global * body_rect,
            cursor_rect: to_global * cursor_rect,
            should_interrupt_composition: false,
        });
    });
}

pub(super) fn clear_page_ime_state(ui: &Ui, body_id: Id) {
    ui.data_mut(|data| {
        data.remove_temp::<BrowserImeState>(body_id);
    });
}

#[cfg(test)]
pub(super) fn page_ime_enabled(ui: &Ui, body_id: Id) -> bool {
    ui.data(|data| data.get_temp::<BrowserImeState>(body_id)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_ime_state_tracks_publish_and_clear() {
        let context = egui::Context::default();
        let body_id = Id::new("browser-body");
        let body_rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 80.0));

        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            publish_page_ime_output(ui, body_id, body_rect);
            assert!(page_ime_enabled(ui, body_id));
            clear_page_ime_state(ui, body_id);
            assert!(!page_ime_enabled(ui, body_id));
        });
        assert_eq!(
            output.platform_output.ime.map(|ime| ime.purpose),
            Some(egui::IMEPurpose::Normal)
        );
        output.textures_delta.clear();
    }
}
