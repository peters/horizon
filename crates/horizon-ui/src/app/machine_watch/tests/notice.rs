//! The desktop notification of an agent of a machine that changed to a state which
//! waits for the person, while no window of Horizon has the focus.

use std::sync::Arc;
use std::time::Instant;

use egui::Context;
use horizon_core::machine_watch::{AgentState, MachineWatch};
use horizon_core::notify::{GROUP, Notice, Sink};

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::notify::Notifications;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};

use super::support::{Server, Ssh, codex_command, record, wait_for, wait_for_unfocused, with_machines};

/// The sink of a test: it records the line that it would show.
#[derive(Clone, Default)]
struct Recorder {
    sent: Arc<std::sync::Mutex<Vec<Notice>>>,
}

impl Sink for Recorder {
    fn send(&self, notice: Notice) {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(notice);
    }

    fn available(&self) -> Option<bool> {
        Some(true)
    }
}

impl Recorder {
    fn lines(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|notice| notice.body.clone())
            .collect()
    }
}

/// The state of the agent of `session` in the snapshot of `app`.
fn agent_state(app: &mut HorizonApp, session: &str) -> Option<AgentState> {
    let snapshot = app.machine_watch.watch.snapshot();
    snapshot
        .machines
        .iter()
        .flat_map(|machine| machine.agents.iter())
        .find(|agent| agent.session == session)
        .map(|agent| agent.state)
}

/// One app with a stand-in `ssh`, the machine `build-1`, one Codex stand-in in a
/// `tmux` pane of the folder `shop`, and a sink that records.
fn app_with_one_agent() -> Option<(tempfile::TempDir, Server, Ssh, Context, HorizonApp, Recorder)> {
    let server = Server::new()?;
    let ssh = Ssh::new(&server)?;
    let folder = tempfile::tempdir().expect("a folder of the test");
    let command = codex_command(folder.path())?;
    let shop = folder.path().join("shop");
    std::fs::create_dir_all(&shop).expect("the folder of the pane");
    server.session_in("hz-shop-1", &shop, &command);
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    settle_machine_load(&mut app);
    app.machine_watch.watch = Arc::new(MachineWatch::with_program(ssh.program()));
    with_machines(&mut app, vec![record("build-1")]);
    let recorder = Recorder::default();
    app.notifications = Notifications::with_sink(Box::new(recorder.clone()));
    Some((folder, server, ssh, ctx, app, recorder))
}

/// Runs one frame of the window with `focused` as the focus of the desktop.
fn frame_with_focus(ctx: &Context, app: &mut HorizonApp, focused: bool) {
    let mut input = raw_input([1400.0, 900.0], None);
    input.focused = focused;
    run_app_frame_with_input(ctx, app, input);
}

#[test]
fn an_agent_of_a_machine_that_waits_sends_one_notice_with_the_name_of_its_machine() {
    let Some((_folder, server, _ssh, ctx, mut app, recorder)) = app_with_one_agent() else {
        return;
    };
    frame_with_focus(&ctx, &mut app, false);
    server.send("hz-shop-1", "ask");
    // The frame that reads the snapshot of the change queues its notice, so the frames
    // that wait for the change run with no window of Horizon in front.
    wait_for_unfocused(&ctx, &mut app, "the approval of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::NeedsYou)
    });
    // The report of the machine can arrive inside a frame, after that frame read the
    // snapshot, so one frame reads the change and queues its notice.
    frame_with_focus(&ctx, &mut app, false);
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert_eq!(
        recorder.lines(),
        vec!["shop needs you \u{00b7} build-1"],
        "the notice names the agent and the machine that it runs on"
    );
}

#[test]
fn an_agent_of_a_machine_that_waits_sends_nothing_while_a_window_is_in_front() {
    let Some((_folder, server, _ssh, ctx, mut app, recorder)) = app_with_one_agent() else {
        return;
    };
    frame_with_focus(&ctx, &mut app, true);
    server.send("hz-shop-1", "ask");
    wait_for(&ctx, &mut app, "the approval of the agent", |app| {
        agent_state(app, "hz-shop-1") == Some(AgentState::NeedsYou)
    });
    // A frame with the window in front reads the snapshot too, and it queues nothing.
    frame_with_focus(&ctx, &mut app, true);
    app.notifications
        .poll(app.board.workspaces.len() > 1, Instant::now() + GROUP, &ctx);
    assert!(
        recorder.lines().is_empty(),
        "the sidebar shows the change to a person who is here: {:?}",
        recorder.lines()
    );
}
