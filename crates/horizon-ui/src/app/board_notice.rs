//! What a saved session of an older Horizon says about the panels that this
//! build removed.
//!
//! A board can name a panel of a kind that this release no longer draws, such as
//! the Usage panel, the Git Changes panel or the setup board. The board still
//! opens with every other panel, and the person reads one line that names how
//! many panels are left out and of which kind, so the gap is not a surprise.

use horizon_core::RetiredPanels;

/// The line that a session with left-out panels says, or `None` for a session
/// that lost nothing.
pub(in crate::app) fn left_out_panels_notice(left_out: &RetiredPanels) -> Option<String> {
    if left_out.is_empty() {
        return None;
    }
    let total = left_out.total();
    let kinds: Vec<String> = left_out
        .kinds()
        .into_iter()
        .map(|(kind, count)| format!("{} {count}", kind_name(kind)))
        .collect();
    let panel = if total == 1 { "panel" } else { "panels" };
    let pronoun = if total == 1 { "it is" } else { "they are" };
    Some(format!(
        "This session had {total} {panel} of kinds that this Horizon removed, so {pronoun} left out: {}.",
        kinds.join(", ")
    ))
}

/// The line that a start says when it ended the programs of holders that no panel
/// showed, or `None` when no holder ran without a panel.
pub(in crate::app) fn unshown_holders_notice(count: usize) -> Option<String> {
    (count > 0).then(|| format!("Horizon ended {count} agent program(s) that no panel showed."))
}

/// The name of a removed panel kind as the person read it in the old window.
fn kind_name(kind: &str) -> &str {
    match kind {
        "git_changes" => "Git Changes",
        "usage" => "Usage",
        "setup_board" => "Setup board",
        "upkeep" => "Upkeep",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::left_out_panels_notice;

    fn counts(entries: &[(&str, usize)]) -> horizon_core::RetiredPanels {
        // The board reader builds this value, and its counter is crate private,
        // so the test reads a board that names the kinds.
        let mut panels = String::from("  panels:");
        if entries.is_empty() {
            panels.push_str(" []\n");
        } else {
            panels.push('\n');
            for (kind, count) in entries {
                for index in 0..*count {
                    let _ = writeln!(panels, "  - name: Panel {index}\n    kind: {kind}");
                }
            }
        }
        let board = format!("workspaces:\n- name: Work\n{panels}");
        let (_state, left_out) =
            horizon_core::RuntimeState::from_yaml_reporting(&board).expect("a board with the removed kinds");
        left_out
    }

    #[test]
    fn a_notice_names_how_many_agent_programs_no_panel_showed() {
        assert_eq!(super::unshown_holders_notice(0), None);
        assert_eq!(
            super::unshown_holders_notice(2).as_deref(),
            Some("Horizon ended 2 agent program(s) that no panel showed.")
        );
    }

    #[test]
    fn a_session_that_lost_nothing_says_nothing() {
        assert_eq!(left_out_panels_notice(&counts(&[])), None);
    }

    #[test]
    fn one_left_out_panel_names_its_count_and_its_kind() {
        let notice = left_out_panels_notice(&counts(&[("usage", 1)])).expect("a notice");

        assert_eq!(
            notice,
            "This session had 1 panel of kinds that this Horizon removed, so it is left out: Usage 1."
        );
    }

    #[test]
    fn several_left_out_panels_of_two_kinds_are_one_notice() {
        let notice = left_out_panels_notice(&counts(&[("usage", 2), ("git_changes", 1)])).expect("a notice");

        assert_eq!(
            notice,
            "This session had 3 panels of kinds that this Horizon removed, so they are left out: \
             Git Changes 1, Usage 2."
        );
    }
}
