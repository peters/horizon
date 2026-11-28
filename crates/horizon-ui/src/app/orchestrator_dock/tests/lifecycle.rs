//! The end of a program of the dock: the close of Horizon, the close of the last
//! panel of a workspace, the removal of a workspace, and the switch of the saved
//! session. Every program that these paths leave is a program that no person can
//! stop, so each test reads the process id of the program that its stand-in
//! started and asserts that the process ended.
//!
//! The stand-in of a test that must survive a close ignores the close of its
//! input and the termination signal, so only a stop of the dock ends it.

use horizon_core::agent_bridge::Entry;

use crate::app::orchestrator_dock::session::Phase;
use crate::app::orchestrator_dock::tests::chat::{
    WIDE, answering, child_of, entries_of, exit_status, send, stand_in, stand_in_named, state_of, stubborn,
    wait_until_no_process, window_of,
};
use crate::app::orchestrator_dock::tests::frame::Window;
use crate::app::test_support::{editor_workspace_state, program_runs};

#[test]
fn the_end_of_horizon_leaves_no_child_of_a_conversation() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    send(&mut window, "hello there");
    window.wait_for("the answer of the stand-in", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    let child = child_of(&pid);

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        window.app().begin_shutdown();
        window.app().run_exit_cleanup();
    }
    assert_eq!(
        window.app().orchestrator_dock.running_bridges(),
        0,
        "no bridge of the dock outlives the end of Horizon"
    );
    #[cfg(target_os = "linux")]
    wait_until_no_process("the child of the conversation", child);
    let _ = child;
}

#[test]
fn the_close_of_horizon_ends_a_program_that_ignores_its_input_and_the_termination_signal() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &stubborn(&pid));
    let mut window = window_of(&program);

    send(&mut window, "a turn that never ends");
    let child = child_of(&pid);
    window.wait_for("the running state of the conversation", |app| {
        state_of(app, "chat") == Some(Phase::Running)
    });

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        window.app().begin_shutdown();
    }
    let status = exit_status(&mut window);

    assert_eq!(status, 0, "the close of Horizon ends well");
    #[cfg(target_os = "linux")]
    assert!(
        !program_runs(child),
        "the program of the conversation ended before Horizon asked for the exit"
    );
    let _ = child;
}

#[test]
fn the_close_of_horizon_ends_two_programs_that_ignore_their_input_and_the_termination_signal() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let first_pid = folder.path().join("first-pid");
    let second_pid = folder.path().join("second-pid");
    let first = stand_in_named(folder.path(), "first.sh", &stubborn(&first_pid));
    let second = stand_in_named(folder.path(), "second.sh", &stubborn(&second_pid));
    let workspaces = vec![
        editor_workspace_state("chat-a", [40.0, 40.0]),
        editor_workspace_state("chat-b", [1000.0, 40.0]),
    ];
    // The dock runs one program, so each conversation starts its own stubborn
    // program under the name that the test gave it.
    let mut window = Window::with_program(WIDE, workspaces, &first);
    window.focus_workspace("chat-a");
    let _ = window.frame(Vec::new());

    send(&mut window, "a turn that never ends");
    let first_child = child_of(&first_pid);
    window.focus_workspace("chat-b");
    window.app().orchestrator_dock.set_program(second.clone());
    send(&mut window, "another turn that never ends");
    let second_child = child_of(&second_pid);
    window.wait_for("the running state of both conversations", |app| {
        state_of(app, "chat-a") == Some(Phase::Running) && state_of(app, "chat-b") == Some(Phase::Running)
    });

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        window.app().begin_shutdown();
    }
    let status = exit_status(&mut window);

    assert_eq!(status, 0, "the close of Horizon ends well");
    #[cfg(target_os = "linux")]
    {
        assert!(
            !program_runs(first_child),
            "the first program of the dock ended before Horizon asked for the exit"
        );
        assert!(
            !program_runs(second_child),
            "the second program of the dock ended before Horizon asked for the exit"
        );
    }
    let _ = (first_child, second_child);
}

#[test]
fn a_workspace_whose_last_panel_closes_stops_and_forgets_its_conversation() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    send(&mut window, "hello there");
    window.wait_for("the answer of the stand-in", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    let child = child_of(&pid);
    let panel = window.app().board.workspaces[0].panels[0];

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        // The close of the last panel of a workspace takes the workspace with it.
        window.app().close_panel(panel);
    }
    assert!(
        window.app().board.workspace_id_by_local_id("chat").is_none(),
        "the last panel of the workspace closed with it"
    );

    // The frame of the dock forgets a conversation whose workspace went.
    let _ = window.frame(Vec::new());
    assert!(
        window.app().orchestrator_dock.session("chat").is_none(),
        "the conversation of the closed workspace went"
    );
    assert_eq!(window.app().orchestrator_dock.running_bridges(), 0);
    #[cfg(target_os = "linux")]
    wait_until_no_process("the program of the closed workspace", child);
    let _ = child;
}

#[test]
fn a_switch_of_the_saved_session_stops_and_clears_the_dock() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    // The program of the conversation ignores the close of its input, so only a
    // stop of the dock ends it.
    let program = stand_in(folder.path(), &stubborn(&pid));
    let mut window = window_of(&program);

    send(&mut window, "a turn that never ends");
    let child = child_of(&pid);
    window.wait_for("the running state of the conversation", |app| {
        state_of(app, "chat") == Some(Phase::Running)
    });

    let target = window
        .app()
        .session_store
        .create_session_from_runtime(horizon_core::RuntimeState {
            workspaces: vec![editor_workspace_state("other", [40.0, 40.0])],
            ..horizon_core::RuntimeState::default()
        })
        .expect("the session of the switch");
    let ctx = window.ctx().clone();
    // The switch of a test refuses a viewport that still stabilizes.
    window.app().root_viewport_stabilizer = None;
    window.app().activate_runtime_session(&ctx, &target);

    assert!(
        window.app().orchestrator_dock.session("chat").is_none(),
        "the conversations of the board that the switch leaves go with it"
    );
    assert_eq!(
        window.app().orchestrator_dock.running_bridges(),
        0,
        "no program of the dock speaks for the board that the switch left"
    );
    #[cfg(target_os = "linux")]
    wait_until_no_process("the program of the session that the switch left", child);
    let _ = child;
}

#[test]
fn a_conversation_that_is_forgotten_stops_its_program_and_drops_its_entries() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    send(&mut window, "hello there");
    window.wait_for("the answer of the stand-in", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    let child = child_of(&pid);

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        // This is the call of the sidebar on the path that removes a workspace.
        window.app().forget_orchestrator_conversation("chat");
    }

    assert!(
        window.app().orchestrator_dock.session("chat").is_none(),
        "the conversation and its draft went with the workspace"
    );
    assert_eq!(window.app().orchestrator_dock.running_bridges(), 0);
    #[cfg(target_os = "linux")]
    wait_until_no_process("the program of the removed workspace", child);
    let _ = child;
}
