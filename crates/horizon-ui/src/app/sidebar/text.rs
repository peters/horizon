//! Text of the sidebar rows: single-line galleys with an Inter weight, the
//! tile initials and the label of a panel row.
use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, TextFormat};
use horizon_core::{PanelKind, PresetConfig, agent_definition};

use crate::theme;

/// Inter weights of the sidebar.
pub(super) const REGULAR: f32 = 400.0;
pub(super) const MEDIUM: f32 = 500.0;
pub(super) const SEMIBOLD: f32 = 620.0;
pub(super) const BOLD: f32 = 700.0;

/// One line of `text` in Inter at `weight`, cut with an ellipsis at `max_width`.
pub(super) fn line(ui: &egui::Ui, text: &str, size: f32, weight: f32, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut format = TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..Default::default()
    };
    if (weight - REGULAR).abs() > f32::EPSILON {
        format.coords = VariationCoords::new([(b"wght", weight)]);
    }
    layout(ui, text, format, max_width)
}

/// One line of monospace text, for counts and kind labels.
pub(super) fn mono(ui: &egui::Ui, text: &str, size: f32, color: Color32) -> Arc<Galley> {
    let format = TextFormat {
        font_id: FontId::monospace(size),
        color,
        ..Default::default()
    };
    layout(ui, text, format, f32::INFINITY)
}

fn layout(ui: &egui::Ui, text: &str, format: TextFormat, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap = TextWrapping {
        max_width: max_width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    ui.painter().layout_job(job)
}

/// Secondary text that stays readable: a step under `FG_SOFT`, well above `FG_DIM`.
pub(super) fn secondary() -> Color32 {
    theme::blend(theme::FG_SOFT(), theme::BG_ELEVATED(), 0.14)
}

/// `text` with runs of whitespace, such as terminal column padding, folded to one space.
pub(super) fn fold_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One or two letters for a workspace tile: the first letters of its first two words.
pub(super) fn initials(name: &str) -> String {
    name.split(|character: char| !character.is_alphanumeric())
        .filter_map(|word| word.chars().next())
        .take(2)
        // One character per word: some letters upper-case to more than one (ß to SS).
        .filter_map(|letter| letter.to_uppercase().next())
        .collect()
}

/// Symbols that agents put in front of their terminal title as a spinner or a state.
fn is_title_symbol(character: char) -> bool {
    character.is_whitespace() || ('\u{2190}'..='\u{2BFF}').contains(&character)
}

/// The text of a panel row. An agent row shows the agent's terminal title, which
/// names its task, without the spinner in front. `own_name` is a name that the
/// person gave the panel; it stays in front of the task. A name that only labels
/// the agent (`None`) is left out, because the kind label already names the agent.
/// Other panels, and agents without a title, show `display_title`.
pub(super) fn panel_label(
    kind: PanelKind,
    own_name: Option<&str>,
    display_title: &str,
    terminal_title: &str,
) -> String {
    if kind.is_agent() {
        let task = fold_whitespace(terminal_title.trim_start_matches(is_title_symbol));
        if !task.is_empty() {
            return match own_name {
                Some(name) if name != terminal_title => format!("{} \u{00B7} {task}", fold_whitespace(name)),
                _ => task,
            };
        }
    }
    fold_whitespace(display_title)
}

/// Whether the name of an agent panel only labels the agent: it is not custom,
/// or it is the agent's own name or the name of a preset of that agent kind.
pub(super) fn name_labels_agent(kind: PanelKind, name: &str, name_is_custom: bool, presets: &[PresetConfig]) -> bool {
    !name_is_custom
        || agent_definition(kind).is_some_and(|agent| name.eq_ignore_ascii_case(agent.display_name))
        || presets
            .iter()
            .any(|preset| preset.kind == kind && preset.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use horizon_core::PanelKind;

    use horizon_core::PresetConfig;

    use super::{fold_whitespace, initials, name_labels_agent, panel_label};

    #[test]
    fn initials_take_the_first_letters_of_the_first_two_words() {
        assert_eq!(initials("Checkout redesign"), "CR");
        assert_eq!(initials("billing-service"), "BS");
        assert_eq!(initials("Atlas app (acme-mobile)"), "AA");
        assert_eq!(initials("Horizon"), "H");
        assert_eq!(initials("é-mail"), "ÉM");
        assert_eq!(initials("  "), "");
        assert_eq!(initials("ßeta ßig"), "SS");
        assert_eq!(initials("ßeta").chars().count(), 1);
    }

    #[test]
    fn a_status_line_loses_its_terminal_padding() {
        assert_eq!(fold_whitespace("lane-876          Fix sync"), "lane-876 Fix sync");
    }

    #[test]
    fn an_agent_row_shows_the_task_without_its_spinner() {
        let label = |terminal_title| panel_label(PanelKind::Claude, None, "Claude Code — title", terminal_title);
        assert_eq!(
            label("\u{25D0} Fix recurring sync timeout"),
            "Fix recurring sync timeout"
        );
        assert_eq!(label("\u{2733} Claude Code"), "Claude Code");
        assert_eq!(label("\u{280B}  Review   tests"), "Review tests");
        // Without a title the row keeps the panel's own title.
        assert_eq!(label(""), "Claude Code — title");
        assert_eq!(label("\u{25D0} "), "Claude Code — title");
    }

    #[test]
    fn an_agent_row_keeps_a_name_that_the_person_gave() {
        assert_eq!(
            panel_label(
                PanelKind::Claude,
                Some("Claude Code backend"),
                "Claude Code backend — \u{25D0} Fix sync",
                "\u{25D0} Fix sync"
            ),
            "Claude Code backend \u{00B7} Fix sync"
        );
    }

    #[test]
    fn only_a_preset_or_agent_name_labels_the_agent() {
        let presets = [PresetConfig {
            name: "Claude Code".to_owned(),
            alias: None,
            kind: PanelKind::Claude,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: None,
        }];
        assert!(name_labels_agent(PanelKind::Claude, "claude code", true, &presets));
        assert!(name_labels_agent(PanelKind::Claude, "Claude", true, &[]));
        assert!(name_labels_agent(PanelKind::Codex, "Backend", false, &[]));
        assert!(!name_labels_agent(
            PanelKind::Claude,
            "Claude Code backend",
            true,
            &presets
        ));
        assert!(!name_labels_agent(PanelKind::Codex, "Claude Code", true, &presets));
    }

    #[test]
    fn other_panels_keep_their_title() {
        assert_eq!(
            panel_label(PanelKind::Browser, Some("Docs"), "Docs — Example page", "Example page"),
            "Docs — Example page"
        );
        assert_eq!(
            panel_label(PanelKind::Shell, None, "\u{25D0} build", "\u{25D0} build"),
            "\u{25D0} build"
        );
    }
}
