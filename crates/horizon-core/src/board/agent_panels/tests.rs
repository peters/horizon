use super::*;
use crate::panel::{PanelKind, PanelOptions};

const IDLE: Observation = Observation {
    exited: false,
    interface_up: true,
    active: false,
    needs_input: false,
};

#[test]
fn state_is_decided_in_order_of_what_blocks_a_message() {
    let everything = Observation {
        exited: true,
        interface_up: true,
        active: true,
        needs_input: true,
    };
    assert_eq!(classify(everything), AgentState::Exited);
    assert_eq!(
        classify(Observation {
            exited: false,
            interface_up: false,
            ..everything
        }),
        AgentState::Starting
    );
    assert_eq!(
        classify(Observation {
            exited: false,
            ..everything
        }),
        AgentState::NeedsInput
    );
    assert_eq!(classify(Observation { active: true, ..IDLE }), AgentState::Working);
    assert_eq!(classify(IDLE), AgentState::Idle);
}

#[test]
fn refusals_have_distinct_codes_and_messages() {
    let all = [
        SendRefusal::UnknownPanel,
        SendRefusal::NotAnAgent,
        SendRefusal::Caller,
        SendRefusal::Exited,
        SendRefusal::Starting,
        SendRefusal::Busy,
        SendRefusal::NeedsInput,
    ];
    let codes: std::collections::HashSet<_> = all.iter().map(|refusal| refusal.code()).collect();
    assert_eq!(codes.len(), all.len());
    assert!(all.iter().all(|refusal| !refusal.message().is_empty()));
}

fn board_with(kinds: &[(&str, PanelKind)]) -> (Board, WorkspaceId, Vec<PanelId>) {
    let mut board = Board::new();
    let workspace = board.create_workspace("agents");
    let ids = kinds
        .iter()
        .map(|(local_id, kind)| {
            board
                .create_panel(
                    PanelOptions {
                        kind: *kind,
                        local_id: Some((*local_id).to_string()),
                        ..PanelOptions::default()
                    },
                    workspace,
                )
                .expect("panel should start")
        })
        .collect();
    (board, workspace, ids)
}

#[test]
fn lists_only_the_agents_of_the_workspace_and_marks_the_caller() {
    let (mut board, workspace, ids) = board_with(&[
        ("claude-one", PanelKind::Claude),
        ("codex-one", PanelKind::Codex),
        ("plain-shell", PanelKind::Shell),
    ]);
    let other = board.create_workspace("elsewhere");
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Claude,
                local_id: Some("claude-away".to_string()),
                ..PanelOptions::default()
            },
            other,
        )
        .expect("panel should start");

    let panels = board.agent_panels_in_workspace(workspace, ids[0]);

    let listed: Vec<_> = panels.iter().map(|panel| panel.panel_id.as_str()).collect();
    assert_eq!(listed, ["claude-one", "codex-one"]);
    assert_eq!(panels.iter().filter(|panel| panel.is_caller).count(), 1);
    assert!(panels[0].is_caller);
    assert_eq!(panels[0].kind, "claude");
    assert_eq!(panels[1].kind, "codex");
}

#[test]
fn sending_is_refused_to_self_to_strangers_and_to_agents_without_an_interface() {
    let (board, _workspace, ids) = board_with(&[("claude-one", PanelKind::Claude), ("plain-shell", PanelKind::Shell)]);
    let (agent, shell) = (ids[0], ids[1]);
    let caller = PanelId(9999);

    assert_eq!(board.check_agent_can_receive(agent, agent), Err(SendRefusal::Caller));
    assert_eq!(
        board.check_agent_can_receive(caller, PanelId(12345)),
        Err(SendRefusal::UnknownPanel)
    );
    assert_eq!(
        board.check_agent_can_receive(caller, shell),
        Err(SendRefusal::NotAnAgent)
    );
    // No agent interface is running in a test, so the agent never reaches its prompt:
    // it is still starting, or its process has already ended.
    assert!(matches!(
        board.check_agent_can_receive(caller, agent),
        Err(SendRefusal::Starting | SendRefusal::Exited)
    ));
    assert_ne!(board.agent_state(agent), Some(AgentState::Idle));
    assert_eq!(board.agent_state(shell), None);
}

#[test]
fn output_is_limited_to_agents() {
    let (board, _workspace, ids) = board_with(&[("claude-one", PanelKind::Claude), ("plain-shell", PanelKind::Shell)]);
    assert!(board.agent_output(ids[0], 10).is_some());
    assert!(board.agent_output(ids[1], 10).is_none());
    assert!(board.agent_output(PanelId(777), 10).is_none());
}

#[test]
fn other_agents_never_see_the_assistant_but_it_sees_itself_and_them() {
    let (board, workspace, ids) = board_with(&[
        (crate::assistant::ASSISTANT_PANEL_LOCAL_ID, PanelKind::Claude),
        ("codex-one", PanelKind::Codex),
    ]);
    let (assistant, codex) = (ids[0], ids[1]);

    let seen_by_codex = board.agent_panels_in_workspace(workspace, codex);
    let ids_seen_by_codex: Vec<_> = seen_by_codex.iter().map(|panel| panel.panel_id.as_str()).collect();
    assert_eq!(ids_seen_by_codex, ["codex-one"]);

    let seen_by_assistant = board.agent_panels_in_workspace(workspace, assistant);
    assert_eq!(seen_by_assistant.len(), 2);
    assert!(seen_by_assistant.iter().any(|panel| panel.is_caller));
}
