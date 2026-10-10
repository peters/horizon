//! A failure's cause, cut after a few rows so a long error line cannot take over the
//! card. The whole cause is in its tooltip, behind Show more, and in Copy error.
use crate::app::cloud_panel::runtime::action_button;
use crate::theme;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{FontId, Galley, RichText};
use std::sync::Arc;

/// Rows a cause takes until it is expanded.
pub(super) const ROWS: usize = 3;
/// The size of the output lines the cause is read beside.
const SIZE: f32 = 13.0;

/// `text` wrapped to `width` and cut with an ellipsis after `rows` rows. A token
/// longer than a row, such as a hash or a path, breaks inside itself.
fn layout(ui: &egui::Ui, text: &str, width: f32, rows: usize) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TextFormat::simple(FontId::monospace(SIZE), theme::PALETTE_RED()),
    );
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// What a drawn cause offers: Show more when it was cut, Show less once expanded.
pub(super) struct Shown {
    id: egui::Id,
    /// Which cause was expanded, so a new cause starts cut again.
    key: egui::Id,
    expanded: bool,
    elided: bool,
}

/// Draws `text` in the width left. `salt` keeps one place's expansion apart from another's.
pub(super) fn show(ui: &mut egui::Ui, salt: impl std::hash::Hash + std::fmt::Debug, text: &str) -> Shown {
    let id = egui::Id::new(("cloud-failure-cause", salt));
    let key = egui::Id::new(text);
    let expanded = ui.data(|data| data.get_temp::<egui::Id>(id)) == Some(key);
    let galley = layout(ui, text, ui.available_width(), if expanded { usize::MAX } else { ROWS });
    let elided = galley.elided;
    // The label's own elided tooltip would repeat the one below in the cause's red.
    let response = ui.add(egui::Label::new(galley).show_tooltip_when_elided(false));
    // A screen reader hears the whole cause, however it is cut on screen.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    if elided {
        response.on_hover_text(RichText::new(text).monospace().size(SIZE));
    }
    Shown {
        id,
        key,
        expanded,
        elided,
    }
}

impl Shown {
    /// Adds Show more or Show less to the row of actions under the cause.
    pub(super) fn toggle(&self, ui: &mut egui::Ui) {
        if !(self.elided || self.expanded) {
            return;
        }
        let label = if self.expanded { "Show less" } else { "Show more" };
        if ui.add(action_button(label)).clicked() {
            let Self { id, key, expanded, .. } = *self;
            ui.data_mut(|data| {
                if expanded {
                    data.remove::<egui::Id>(id);
                } else {
                    data.insert_temp(id, key);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;

    const CONFLICT: &str = "Error response from daemon: Conflict. The container name \"/horizon-contract-00000000-1111-2222-3333-444444444444\" is already in use by container \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\". You have to remove (or rename) that container to be able to reuse that name.";

    fn laid_out(text: &str, width: f32, rows: usize) -> Arc<Galley> {
        let mut galley = None;
        let _ = egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                galley = Some(layout(ui, text, width, rows));
            })
            .discard_textures();
        galley.unwrap()
    }

    #[test]
    fn a_long_cause_is_cut_after_three_rows_inside_its_width() {
        let galley = laid_out(CONFLICT, 360.0, ROWS);
        assert!(galley.elided, "the cause is longer than three rows at 360 px");
        assert_eq!(galley.rows.len(), ROWS);
        let last = galley.rows.last().map(|row| row.text()).unwrap_or_default();
        assert!(last.ends_with('…'), "{last}");
        assert!(galley.size().x <= 360.0, "{}", galley.size().x);
    }

    #[test]
    fn an_expanded_cause_shows_every_character_and_breaks_a_hash_inside_its_width() {
        let galley = laid_out(CONFLICT, 360.0, usize::MAX);
        assert!(!galley.elided);
        assert_eq!(galley.text(), CONFLICT);
        assert!(galley.rows.len() > ROWS);
        assert!(
            galley.rows.iter().all(|row| row.size.x <= 360.0),
            "the 64-character container id wraps instead of widening the card"
        );
    }

    #[test]
    fn a_short_cause_is_not_cut() {
        let galley = laid_out("error from registry: denied", 360.0, ROWS);
        assert!(!galley.elided);
        assert_eq!(galley.rows.len(), 1);
    }

    /// The characters a galley draws; an elided galley keeps its whole text but not its glyphs.
    fn drawn(galley: &Galley) -> String {
        galley.rows.iter().map(|row| row.text()).collect()
    }

    /// Texts and the Show more or Show less label one frame draws for `CONFLICT`.
    fn frame(ctx: &egui::Context, text: &str) -> (Vec<String>, bool, bool) {
        let mut flags = (false, false);
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(360.0);
                let shown = show(ui, "test", text);
                flags = (shown.elided, shown.expanded);
            })
            .discard_textures();
        let texts = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(drawn(&text.galley)),
                _ => None,
            })
            .collect();
        (texts, flags.0, flags.1)
    }

    #[test]
    fn show_more_expands_only_the_cause_it_was_chosen_for() {
        let ctx = egui::Context::default();
        let (texts, elided, expanded) = frame(&ctx, CONFLICT);
        assert!(elided && !expanded);
        assert!(!texts.iter().any(|text| text == CONFLICT), "{texts:?}");
        let id = egui::Id::new(("cloud-failure-cause", "test"));
        ctx.data_mut(|data| data.insert_temp(id, egui::Id::new(CONFLICT)));
        let (texts, elided, expanded) = frame(&ctx, CONFLICT);
        assert!(expanded && !elided);
        assert!(texts.iter().any(|text| text == CONFLICT), "{texts:?}");
        let other = CONFLICT.replace("0123", "4567");
        let (_, elided, expanded) = frame(&ctx, &other);
        assert!(elided && !expanded, "a new cause starts cut again");
    }
}
