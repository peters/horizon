//! The mirror in the dock: the header of a session of another computer, the rows
//! of its conversation, and the line in place of the field.

use egui::FullOutput;
use horizon_sync::{MirrorComputer, MirrorEntry, MirrorProgram, MirrorSession};

use crate::app::orchestrator_dock::mirror::{header_line, line, render};
use crate::app::session_mirror::{Mirror, MirrorKey, MirrorView};
use crate::test_egui::DiscardTextures;

fn session() -> MirrorSession {
    MirrorSession {
        program: MirrorProgram::Claude,
        session: "00000000-0000-0000-0000-000000000001".to_owned(),
        title: "made-up title".to_owned(),
        folder: "repo".to_owned(),
        parent: "~/github".to_owned(),
        updated_at: 1_700_000_000_000,
        live: true,
    }
}

fn computer() -> MirrorComputer {
    MirrorComputer {
        device: "laptop-a".to_owned(),
        label: "Laptop A".to_owned(),
        sessions: vec![session()],
        unreadable: vec![MirrorProgram::Antigravity],
        own: false,
    }
}

/// A view of one session with three rows, as the pump gives it to a frame.
fn view() -> std::sync::Arc<MirrorView> {
    let mut mirror = Mirror::default();
    let computers = vec![computer()];
    let _ = mirror.apply(&horizon_sync::LiveEvent::MirrorComputers { computers });
    let _ = mirror.apply(&horizon_sync::LiveEvent::MirrorEntries {
        owner: "laptop-a".to_owned(),
        program: "claude".to_owned(),
        session: session().session,
        entries: vec![
            MirrorEntry::Person {
                text: "made-up request".to_owned(),
            },
            MirrorEntry::Agent {
                text: "made-up answer".to_owned(),
            },
            MirrorEntry::Tool {
                name: "Bash".to_owned(),
                request: "made-up command".to_owned(),
            },
        ],
        tail: true,
    });
    mirror.snapshot().expect("the mirror changed")
}

fn key() -> MirrorKey {
    MirrorKey {
        device: "laptop-a".to_owned(),
        program: MirrorProgram::Claude,
        session: session().session,
    }
}

/// The texts that one frame of the dock of `view` painted.
fn drawn(view: &MirrorView, key: &MirrorKey) -> Vec<String> {
    let ctx = egui::Context::default();
    let output: FullOutput = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            let content = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(420.0, 900.0));
            ui.set_min_size(content.size());
            render(ui, content, view, key);
            let _ = &view;
        })
        .discard_textures();
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_header_reads_the_title_the_program_and_the_computer() {
    let texts = drawn(&view(), &key());
    assert!(texts.contains(&"made-up title".to_owned()), "{texts:?}");
    assert!(
        texts.contains(&"Claude Code on Laptop A · read only".to_owned()),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"live".to_owned()),
        "the chip of a running session: {texts:?}"
    );
}

#[test]
fn the_rows_of_the_conversation_are_drawn_and_the_field_is_one_line() {
    let texts = drawn(&view(), &key());
    assert!(texts.contains(&"made-up request".to_owned()), "{texts:?}");
    assert!(texts.contains(&"made-up answer".to_owned()), "{texts:?}");
    assert!(texts.contains(&"Bash".to_owned()), "{texts:?}");
    assert!(
        texts.contains(&"This session runs on Laptop A. You read it here.".to_owned()),
        "{texts:?}"
    );
}

#[test]
fn a_session_that_no_process_runs_reads_idle() {
    let mut view = Mirror::default();
    let mut computer = computer();
    computer.sessions[0].live = false;
    let _ = view.apply(&horizon_sync::LiveEvent::MirrorComputers {
        computers: vec![computer],
    });
    let view = view.snapshot().expect("the mirror changed");
    let texts = drawn(&view, &key());
    assert!(texts.contains(&"idle".to_owned()), "{texts:?}");
    assert!(!texts.contains(&"live".to_owned()), "{texts:?}");
}

#[test]
fn the_line_and_the_header_name_the_program_and_the_computer() {
    assert_eq!(header_line("Codex", "Laptop A"), "Codex on Laptop A · read only");
    assert_eq!(line("Laptop A"), "This session runs on Laptop A. You read it here.");
}

#[test]
fn the_choice_of_a_mirrored_session_begins_and_ends_with_the_dock() {
    use horizon_core::{Board, RuntimeState, WorkspaceState, new_local_id};

    use crate::app::orchestrator_dock::OrchestratorDock;

    let workspace = WorkspaceState {
        local_id: new_local_id(),
        name: "work".to_owned(),
        ..WorkspaceState::default()
    };
    let mut board = Board::from_runtime_state(&RuntimeState {
        workspaces: vec![workspace],
        ..RuntimeState::default()
    })
    .expect("the board of the test");
    let mut dock = OrchestratorDock::new(true);

    dock.front_desk_mut().show_mirror(key());
    assert_eq!(dock.front_desk().mirror(), Some(&key()), "the dock shows the session");
    assert!(!dock.front_desk().shows(&board), "the dock shows no front desk");

    dock.front_desk_mut().show();
    assert_eq!(dock.front_desk().mirror(), None, "the front desk ends the follow");
    assert!(dock.front_desk().shows(&board));

    dock.front_desk_mut().show_mirror(key());
    dock.front_desk_mut().show_workspace();
    assert_eq!(dock.front_desk().mirror(), None, "a workspace ends the follow");

    dock.front_desk_mut().show_mirror(key());
    dock.front_desk_mut().follow_active_workspace(&board);
    board.active_workspace = None;
    dock.front_desk_mut().follow_active_workspace(&board);
    assert_eq!(
        dock.front_desk().mirror(),
        None,
        "a change of the active workspace ends the follow"
    );
}
