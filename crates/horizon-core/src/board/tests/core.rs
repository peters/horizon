use std::time::{Duration, SystemTime};

use crate::attention::{AttentionSeverity, AttentionState, RESOLVED_ATTENTION_RETENTION};
use crate::panel::{AgentAttentionSignal, PanelKind, PanelOptions, current_unix_millis};

use super::super::*;
use super::editor_panel_options;

#[test]
fn rename_workspace_updates_matching_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");

    assert!(board.rename_workspace(workspace_id, "backend"));
    assert_eq!(board.workspaces[0].name, "backend");
}

#[test]
fn rename_workspace_rejects_blank_names() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");

    assert!(!board.rename_workspace(workspace_id, "   "));
    assert_eq!(board.workspaces[0].name, "frontend");
}

#[test]
fn rename_panel_updates_matching_panel() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    assert!(board.rename_panel(panel_id, "backend shell"));
    assert_eq!(
        board.panel(panel_id).expect("panel should exist").title,
        "backend shell"
    );
}

#[test]
fn rename_panel_rejects_blank_names() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    let original_title = board.panel(panel_id).expect("panel should exist").title.clone();

    assert!(!board.rename_panel(panel_id, "   "));
    assert_eq!(board.panel(panel_id).expect("panel should exist").title, original_title);
}

#[test]
fn close_panel_removes_panel_attention() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = PanelId(7);

    board.create_attention(
        workspace_id,
        Some(panel_id),
        "codex-ui",
        "Needs user feedback",
        AttentionSeverity::High,
    );

    board.close_panel(panel_id);

    assert!(board.unresolved_attention().next().is_none());
}

#[test]
fn resolve_attention_marks_item_resolved() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let attention_id = board.create_attention(
        workspace_id,
        None,
        "system",
        "Review build result",
        AttentionSeverity::Medium,
    );

    assert!(board.resolve_attention(attention_id));
    assert!(board.unresolved_attention().next().is_none());
}

#[test]
fn dismissing_attention_keeps_same_signal_suppressed_until_it_clears() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(99);

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    let attention_id = board.unresolved_attention().next().expect("open attention").id;
    assert!(board.dismiss_attention(attention_id));

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    assert!(board.unresolved_attention().next().is_none());

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "");
    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    assert!(board.unresolved_attention().next().is_some());
}

#[test]
fn stale_ready_for_input_attention_auto_dismisses() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let attention_id = board.create_attention(
        workspace_id,
        Some(PanelId(7)),
        "agent",
        "Ready for input",
        AttentionSeverity::High,
    );

    let item = board
        .attention
        .iter_mut()
        .find(|item| item.id == attention_id)
        .expect("attention item");
    item.created_at = std::time::SystemTime::now() - READY_FOR_INPUT_AUTO_DISMISS_AFTER - Duration::from_secs(1);

    board.dismiss_expired_ready_attention(READY_FOR_INPUT_AUTO_DISMISS_AFTER);

    let item = board
        .attention
        .iter()
        .find(|item| item.id == attention_id)
        .expect("attention item");
    assert_eq!(item.state, AttentionState::Dismissed);
}

#[test]
fn idle_processing_expires_and_prunes_ready_attention() {
    let mut board = Board::new();
    board.attention_enabled = true;
    let workspace_id = board.create_workspace("agents");
    let attention_id = board.create_attention(
        workspace_id,
        Some(PanelId(7)),
        "agent",
        "Ready for input",
        AttentionSeverity::High,
    );
    board
        .attention
        .iter_mut()
        .find(|item| item.id == attention_id)
        .expect("attention item")
        .created_at = SystemTime::now() - READY_FOR_INPUT_AUTO_DISMISS_AFTER - Duration::from_secs(1);

    let output = board.process_output();

    assert!(!output.had_terminal_output);
    assert!(board.attention.is_empty());
}

#[test]
fn heuristic_reconciliation_does_not_resolve_explicit_notifications() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(7);
    let notification_id = board.create_attention(
        workspace_id,
        Some(panel_id),
        "agent-notify",
        "Need review",
        AttentionSeverity::High,
    );

    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    board.reconcile_agent_attention_signal(panel_id, workspace_id, "");

    let notification = board
        .attention
        .iter()
        .find(|item| item.id == notification_id)
        .expect("notification");
    assert!(notification.is_open());
    assert_eq!(board.unresolved_attention().count(), 1);
}

#[test]
fn startup_ready_signal_is_baselined_but_actionable_prompt_opens_attention() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(7);
    let launched_at = current_unix_millis();
    let first = AgentAttentionSignal {
        summary: "Ready for input",
        fingerprint: "Ready for input".to_string(),
    };
    let second = AgentAttentionSignal {
        summary: "Waiting for input",
        fingerprint: "Waiting for input\0Second question?".to_string(),
    };

    board.observe_agent_attention_signal(panel_id, workspace_id, Some(&first), launched_at);
    assert!(board.unresolved_attention().next().is_none());

    board.observe_agent_attention_signal(panel_id, workspace_id, Some(&second), launched_at);
    let item = board.unresolved_attention().next().expect("distinct prompt attention");
    assert_eq!(item.summary, "Waiting for input");
}

#[test]
fn first_actionable_prompt_during_startup_grace_opens_attention() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(7);
    let approval = AgentAttentionSignal {
        summary: "Waiting for approval",
        fingerprint: "Waiting for approval\0Allow deployment? [y/N]".to_string(),
    };

    board.observe_agent_attention_signal(panel_id, workspace_id, Some(&approval), current_unix_millis());

    let item = board.unresolved_attention().next().expect("startup approval attention");
    assert_eq!(item.summary, "Waiting for approval");
}

#[test]
fn first_ready_after_startup_approval_resolves_without_opening_ready_attention() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(7);
    let launched_at = current_unix_millis();
    let approval = AgentAttentionSignal {
        summary: "Waiting for approval",
        fingerprint: "Waiting for approval\0Allow deployment? [y/N]".to_string(),
    };
    let ready = AgentAttentionSignal {
        summary: "Ready for input",
        fingerprint: "Ready for input".to_string(),
    };

    board.observe_agent_attention_signal(panel_id, workspace_id, Some(&approval), launched_at);
    board.observe_agent_attention_signal(panel_id, workspace_id, Some(&ready), launched_at);

    assert!(board.unresolved_attention_for_panel(panel_id).is_none());
}

#[test]
fn pruning_keeps_open_and_recently_resolved_attention_only() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let open_id = board.create_attention(workspace_id, None, "system", "Open", AttentionSeverity::High);
    let stale_id = board.create_attention(workspace_id, None, "system", "Stale", AttentionSeverity::Low);
    let dismissed_id = board.create_attention(workspace_id, None, "system", "Dismissed", AttentionSeverity::Low);
    assert!(board.resolve_attention(stale_id));
    assert!(board.dismiss_attention(dismissed_id));
    let now = SystemTime::now();
    board
        .attention
        .iter_mut()
        .find(|item| item.id == stale_id)
        .expect("stale item")
        .resolved_at = Some(now - RESOLVED_ATTENTION_RETENTION - Duration::from_secs(1));

    board.prune_closed_attention(now, RESOLVED_ATTENTION_RETENTION);

    assert_eq!(board.attention.len(), 1);
    assert_eq!(board.attention[0].id, open_id);
}

#[test]
fn restarting_panel_resets_heuristic_attention_tracking() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");

    board.restart_panel(panel_id).expect("panel restart");

    assert!(board.unresolved_attention_for_panel(panel_id).is_none());
    assert!(!board.panel_attention_signals.contains_key(&panel_id));
    assert!(!board.panel_attention_startup_baselined.contains(&panel_id));
}

#[test]
fn repeated_resolve_or_dismiss_reports_no_state_change() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let resolved_id = board.create_attention(workspace_id, None, "system", "Resolved", AttentionSeverity::Low);
    let dismissed_id = board.create_attention(workspace_id, None, "system", "Dismissed", AttentionSeverity::Low);

    assert!(board.resolve_attention(resolved_id));
    assert!(!board.resolve_attention(resolved_id));
    assert!(board.dismiss_attention(dismissed_id));
    assert!(!board.dismiss_attention(dismissed_id));
}

#[test]
fn disabling_attention_clears_existing_feed_state() {
    let mut board = Board::new();
    board.set_attention_enabled(true);
    let workspace_id = board.create_workspace("agents");
    let panel_id = PanelId(7);
    board.reconcile_agent_attention_signal(panel_id, workspace_id, "Ready for input");
    assert!(board.unresolved_attention().next().is_some());

    board.set_attention_enabled(false);

    assert!(board.attention.is_empty());
    assert!(board.panel_attention_signals.is_empty());
    assert!(board.panel_attention_startup_baselined.is_empty());
}

#[test]
fn disabling_attention_clears_feed_when_already_disabled() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    board.create_attention(
        workspace_id,
        None,
        "restore",
        "Failed to restore agent",
        AttentionSeverity::High,
    );

    board.set_attention_enabled(false);

    assert!(board.attention.is_empty());
}

#[test]
fn enabling_attention_preserves_existing_restore_failures() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let attention_id = board.create_attention(
        workspace_id,
        None,
        "restore",
        "Failed to restore agent",
        AttentionSeverity::High,
    );

    board.set_attention_enabled(true);

    assert!(board.attention_enabled);
    assert_eq!(
        board.unresolved_attention().map(|item| item.id).collect::<Vec<_>>(),
        [attention_id]
    );
}

#[test]
fn enabling_attention_rearms_agent_initial_scan() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");
    board.panel_mut(panel_id).expect("panel should exist").kind = PanelKind::Gemini;

    assert!(
        !board
            .panel_mut(panel_id)
            .is_some_and(crate::panel::Panel::take_initial_attention_scan_pending)
    );
    board.set_attention_enabled(true);

    assert!(
        board
            .panel_mut(panel_id)
            .is_some_and(crate::panel::Panel::take_initial_attention_scan_pending)
    );
}

#[test]
fn attention_enable_seed_only_baselines_ready_prompts() {
    let ready = AgentAttentionSignal {
        summary: "Ready for input",
        fingerprint: "Ready for input".to_string(),
    };
    let approval = AgentAttentionSignal {
        summary: "Waiting for approval",
        fingerprint: "Waiting for approval\0Allow deployment? [y/N]".to_string(),
    };

    assert!(super::super::attention::should_baseline_attention_signal(&ready));
    assert!(!super::super::attention::should_baseline_attention_signal(&approval));
}

#[cfg(unix)]
#[test]
fn enabling_attention_surfaces_an_existing_actionable_agent_prompt() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("agents");
    let panel_id = board
        .create_panel(
            PanelOptions {
                command: Some("/bin/sh".to_string()),
                args: vec![
                    "-c".to_string(),
                    "printf 'Waiting for approval\\nAllow deployment? [y/N]\\n'; sleep 2".to_string(),
                ],
                kind: PanelKind::Gemini,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("agent fixture should spawn");

    let started_at = std::time::Instant::now();
    while board
        .panel(panel_id)
        .and_then(crate::panel::Panel::detect_attention_signal)
        .is_none()
        && started_at.elapsed() < Duration::from_secs(2)
    {
        let _ = board.process_output();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        board
            .panel(panel_id)
            .and_then(crate::panel::Panel::detect_attention_signal)
            .is_some(),
        "agent fixture did not render its approval prompt"
    );

    assert!(
        board
            .panel_mut(panel_id)
            .is_some_and(crate::panel::Panel::take_initial_attention_scan_pending),
        "fixture should model a panel whose first attention scan already ran"
    );
    board.set_attention_enabled(true);
    let _ = board.process_output();

    let item = board
        .unresolved_attention_for_panel(panel_id)
        .expect("existing approval should become actionable on enable");
    assert_eq!(item.summary, "Waiting for approval");
    board.shutdown_terminal_panels();
}

#[test]
fn focusing_panel_tracks_active_workspace() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("frontend");
    let panel_id = board
        .create_panel(editor_panel_options(), workspace_id)
        .expect("panel should spawn");

    board.focus(panel_id);

    assert_eq!(board.active_workspace, Some(workspace_id));
}

#[test]
fn shutdown_terminal_panels_waits_for_shell_and_command_panels() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("shutdown");
    let shell_panel = board
        .create_panel(PanelOptions::default(), workspace_id)
        .expect("shell panel should spawn");
    let command_panel = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("command panel should spawn");

    board.shutdown_terminal_panels();

    assert!(
        board
            .panel_mut(shell_panel)
            .expect("shell panel should exist")
            .wait_for_shutdown(Duration::from_millis(10))
    );
    assert!(
        board
            .panel_mut(command_panel)
            .expect("command panel should exist")
            .wait_for_shutdown(Duration::from_millis(10))
    );
}

#[test]
fn begin_async_shutdown_completes_for_shell_and_command_panels() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace("shutdown");
    board
        .create_panel(PanelOptions::default(), workspace_id)
        .expect("shell panel should spawn");
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("command panel should spawn");

    let progress = board.begin_async_shutdown();
    let started_at = std::time::Instant::now();
    while !progress.is_complete() && started_at.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(progress.is_complete());
}
