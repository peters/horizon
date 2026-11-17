//! Text of the sidebar rows: single-line galleys with an Inter weight, the
//! tile initials and the label of a panel row.
use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::text::{LayoutJob, TextWrapping};
use egui::{Color32, FontId, Galley, TextFormat};
use horizon_core::{Attention, PanelKind, PresetConfig, agent_definition, agent_work::title_name};

use crate::theme;
use crate::typeface::Weight;

/// Inter weights of the sidebar, named by the one weight enum of the interface.
pub(super) const REGULAR: f32 = Weight::Regular.wght();
pub(super) const MEDIUM: f32 = Weight::Medium.wght();
pub(super) const SEMIBOLD: f32 = Weight::Semibold.wght();
pub(super) const BOLD: f32 = Weight::Black.wght();

/// One line of `text` in Inter at `weight`, cut with an ellipsis at `max_width`.
pub(super) fn line(ui: &egui::Ui, text: &str, size: f32, weight: f32, color: Color32, max_width: f32) -> Arc<Galley> {
    layout(ui, text, format(size, weight, color), max_width, ONE_ROW)
}

/// One paragraph of `text` in Inter at `weight`, wrapped at `max_width` so every
/// word that fits stays whole. A notice of the machine section names a machine,
/// and the sidebar is narrow by design: one cut line would hide the name.
pub(super) fn paragraph(
    ui: &egui::Ui,
    text: &str,
    size: f32,
    weight: f32,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    layout(ui, text, format(size, weight, color), max_width, ANY_ROW)
}

/// The format of one run of sidebar text at `size` and `weight`.
fn format(size: f32, weight: f32, color: Color32) -> TextFormat {
    let mut format = TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..Default::default()
    };
    if (weight - REGULAR).abs() > f32::EPSILON {
        format.coords = VariationCoords::new([(b"wght", weight)]);
    }
    format
}

/// A line shows one row and cuts the rest with an ellipsis.
const ONE_ROW: usize = 1;
/// A paragraph takes the rows that its words need.
const ANY_ROW: usize = usize::MAX;

/// One line of monospace text, for counts and kind labels.
pub(super) fn mono(ui: &egui::Ui, text: &str, size: f32, color: Color32) -> Arc<Galley> {
    let format = TextFormat {
        font_id: FontId::monospace(size),
        color,
        ..Default::default()
    };
    layout(ui, text, format, f32::INFINITY, ONE_ROW)
}

fn layout(ui: &egui::Ui, text: &str, format: TextFormat, max_width: f32, max_rows: usize) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), format);
    job.wrap = TextWrapping {
        max_width: max_width.max(1.0),
        max_rows,
        // A paragraph breaks between its words. A word that is longer than the
        // width still breaks inside, so no text leaves the sidebar.
        break_anywhere: max_rows == ONE_ROW,
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
///
/// A title that names the state of the agent is a statement about its turn and not
/// a name: the row shows the name of the panel of that title, which stands still
/// while the state of the program changes.
pub(super) fn panel_label(
    kind: PanelKind,
    own_name: Option<&str>,
    display_title: &str,
    terminal_title: &str,
) -> String {
    if kind.is_agent() {
        let task = match title_name(kind, terminal_title) {
            Some(name) => fold_whitespace(name),
            None => fold_whitespace(terminal_title.trim_start_matches(is_title_symbol)),
        };
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

/// What a screen reader says for a workspace row: its name, the panel count that
/// the row shows (`panels`, accordion only) and whether it is in a new window.
pub(super) fn workspace_spoken(name: &str, panels: Option<usize>, detached: bool) -> String {
    let mut spoken = match panels {
        Some(1) => format!("{name}, 1 panel"),
        Some(count) => format!("{name}, {count} panels"),
        None => name.to_owned(),
    };
    if detached {
        spoken.push_str(", in a new window");
    }
    spoken
}

/// What a screen reader says for a panel row: the kind, the label and the one
/// word of its attention state.
pub(super) fn panel_spoken(kind: PanelKind, label: &str, attention: Attention) -> String {
    match attention.spoken() {
        Some(word) => format!("{}: {label}, {}", kind.display_name(), word.to_lowercase()),
        None => format!("{}: {label}", kind.display_name()),
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::PanelKind;

    use horizon_core::Attention;
    use horizon_core::PresetConfig;

    use crate::test_egui::DiscardTextures;

    use super::{fold_whitespace, initials, name_labels_agent, panel_label, panel_spoken, workspace_spoken};

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
    fn a_title_loses_its_terminal_padding() {
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
    fn a_state_title_of_a_codex_row_shows_the_folder_alone() {
        let label = |terminal_title| panel_label(PanelKind::Codex, None, "Codex \u{2014} title", terminal_title);
        for title in [
            "\u{280b} work",
            "\u{2819} work",
            "\u{2839} work",
            "[ . ] Action Required | work",
            "[ ! ] Action Required | work",
            "[ . ] Action Required | Create a file | work",
            "[ ! ] Action Required | Create a file | work",
        ] {
            assert_eq!(label(title), "work", "{title:?}");
        }
        // A state title that leaves no name keeps the name of the panel.
        for title in ["\u{280b}", "[ ! ] Action Required"] {
            assert_eq!(label(title), "Codex \u{2014} title", "{title:?}");
        }
        // The name of a person stays in front of the folder of the program.
        assert_eq!(
            panel_label(
                PanelKind::Codex,
                Some("Backend"),
                "Backend \u{2014} work",
                "[ ! ] Action Required | Create a file | work"
            ),
            "Backend \u{00B7} work"
        );
    }

    #[test]
    fn a_thousand_frames_of_a_state_title_show_one_name_and_lay_out_no_text() {
        const FRAMES: [char; 8] = [
            '\u{280b}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283c}', '\u{2834}', '\u{2826}', '\u{2827}',
        ];
        // The titles of a thousand frames of one turn: the spinner of a working
        // program, and then the two frames of an approval that blink twice a
        // second.
        let titles: Vec<String> = (0..1_000)
            .map(|frame| {
                if frame < 500 {
                    format!("{} work", FRAMES[frame % FRAMES.len()])
                } else {
                    let frame_of_wait = if frame % 2 == 0 { "[ . ]" } else { "[ ! ]" };
                    format!("{frame_of_wait} Action Required | Create a file | work")
                }
            })
            .collect();
        let ctx = egui::Context::default();
        let mut names = std::collections::BTreeSet::new();
        let mut first = None;
        let _frame = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                for title in &titles {
                    let label = panel_label(PanelKind::Codex, None, "Codex \u{2014} title", title);
                    // The row lays its name out at the width of the sidebar. The
                    // layout is memoized, so a name that stands still is the galley
                    // of the frame before it, and a name that moves is a new one.
                    let galley = super::line(ui, &label, 12.0, super::REGULAR, egui::Color32::WHITE, 160.0);
                    assert_eq!(galley.text(), "work", "the name of the row is the folder");
                    match &first {
                        None => first = Some(galley),
                        Some(first) => assert!(
                            std::sync::Arc::ptr_eq(first, &galley),
                            "a frame of the state of a title lays out no text of the row"
                        ),
                    }
                    names.insert(label);
                }
            })
            .discard_textures();
        assert_eq!(names.len(), 1, "the name of the row stands still: {names:?}");
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

    #[test]
    fn a_screen_reader_hears_what_the_rows_show() {
        assert_eq!(workspace_spoken("Horizon", Some(4), false), "Horizon, 4 panels");
        assert_eq!(
            workspace_spoken("Docs", Some(1), true),
            "Docs, 1 panel, in a new window"
        );
        assert_eq!(workspace_spoken("Docs", None, false), "Docs");
        assert_eq!(
            panel_spoken(PanelKind::Claude, "Fix sync", Attention::Working),
            "Claude: Fix sync, working"
        );
        assert_eq!(
            panel_spoken(PanelKind::Claude, "Fix sync", Attention::NeedsYou),
            "Claude: Fix sync, needs you"
        );
        assert_eq!(
            panel_spoken(PanelKind::Claude, "Fix sync", Attention::Background),
            "Claude: Fix sync, works in the background"
        );
        assert_eq!(
            panel_spoken(PanelKind::Editor, "notes.md", Attention::Idle),
            "Editor: notes.md"
        );
    }
}
