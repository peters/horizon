//! What the person sees of shared terminals: the section of the sidebar, the
//! chips of the title bars and the menu of a follow panel.

use std::time::{Duration, Instant};

use egui::{Event, PointerButton, Pos2};
use horizon_core::{Panel, PanelKind};
use horizon_sync::{LiveComputer, LiveEvent, LivePanel, LiveState};

use super::{HERE, PATIENCE, THERE, Window, follower_of};
use crate::app::test_support::{drawn_text, drew_text, raw_input, run_app_frame_with_input};

/// The size of the window of these tests.
const SIZE: [f32; 2] = [1600.0, 1000.0];

fn shared(id: &str, title: &str, kind: &str, state: &str) -> LivePanel {
    LivePanel {
        id: id.to_owned(),
        title: title.to_owned(),
        kind: kind.to_owned(),
        state: state.to_owned(),
        workspace: "Work".to_owned(),
        cols: 80,
        rows: 24,
        typing: true,
    }
}

/// The list of a server with this window and the other computer, which shares
/// `panels`.
fn list(panels: Vec<LivePanel>) -> LiveEvent {
    let computer = |device: &str, label: &str, panels, own| LiveComputer {
        device: device.to_owned(),
        label: label.to_owned(),
        panels,
        own,
    };
    LiveEvent::Computers {
        computers: vec![
            computer(HERE, "Here", Vec::new(), true),
            computer(THERE, "There", panels, false),
        ],
    }
}

/// One frame of the window at [`SIZE`] with `events`.
fn frame(window: &mut Window, events: Vec<Event>) -> egui::FullOutput {
    let mut input = raw_input(SIZE, Some([0.0, 0.0]));
    input.events = events;
    run_app_frame_with_input(&window.ctx, &mut window.app, input)
}

/// The texts that a frame drew.
fn texts(output: &egui::FullOutput) -> Vec<&str> {
    let shapes = output.shapes.iter().map(|clipped| &clipped.shape);
    shapes
        .filter_map(|shape| match shape {
            egui::epaint::Shape::Text(shape) => Some(shape.galley.text()),
            _ => None,
        })
        .collect()
}

/// The galley that a frame drew for the text `text`.
fn galley_of(output: &egui::FullOutput, text: &str) -> Option<std::sync::Arc<egui::Galley>> {
    output.shapes.iter().find_map(|clipped| match &clipped.shape {
        egui::epaint::Shape::Text(drawn) if drawn.galley.text() == text => Some(drawn.galley.clone()),
        _ => None,
    })
}

/// Runs frames until one of them draws `words`, and answers that frame.
fn until_drawn(window: &mut Window, words: &str) -> egui::FullOutput {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let output = frame(window, Vec::new());
        if drew_text(&output, words) {
            return output;
        }
        assert!(Instant::now() < deadline, "no frame drew {words}: {:?}", texts(&output));
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// One press of `button` at `at`, and the frame after its release.
fn press(window: &mut Window, at: Pos2, button: PointerButton) -> egui::FullOutput {
    let pressed = |pressed| Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(window, vec![Event::PointerMoved(at)]);
    frame(window, vec![pressed(true)]);
    frame(window, vec![pressed(false)]);
    frame(window, Vec::new())
}

/// A real run showed a publishing computer in the sidebar with no session under
/// it: the window told the link that it reads the mirror only when its dock
/// followed a session, and the list of the sessions never came.
#[test]
fn a_window_reads_the_mirror_from_its_first_passes_with_no_session_in_the_dock() {
    let mut window = Window::new();
    window.link.relay().events.push(list(Vec::new()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while window.link.relay().mirror_opens == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the pump tells the link that the window reads the mirror"
        );
        let _ = frame(&mut window, Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        window.link.relay().mirror_opens,
        1,
        "one open for one link, also over many passes"
    );
}

/// A real run showed a mirrored session in the dock with its header and no
/// entry: the choice of the dock alone did not reach the pump, so the link never
/// followed the session.
#[test]
fn a_session_that_the_dock_shows_is_followed_and_the_next_choice_ends_the_follow() {
    use crate::app::session_mirror::MirrorKey;
    use horizon_core::session_mirror::MirrorProgram;

    let mut window = Window::new();
    window.link.relay().events.push(list(Vec::new()));
    let key = MirrorKey {
        device: THERE.to_owned(),
        program: MirrorProgram::Claude,
        session: "3f0c2a52-7d1e-4f5e-9a59-0b9a3e1f6c11".to_owned(),
    };
    let wanted = format!("{THERE}/{}/{}", MirrorProgram::Claude.key(), key.session);
    window.app.orchestrator_dock.front_desk_mut().show_mirror(key);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !window.link.relay().mirror_follows.contains(&wanted) {
        assert!(
            std::time::Instant::now() < deadline,
            "the link follows the session that the dock shows"
        );
        let _ = frame(&mut window, Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // The dock shows the front desk again, and the follow ends.
    window.app.orchestrator_dock.front_desk_mut().show();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !window.link.relay().mirror_follows.is_empty() {
        assert!(std::time::Instant::now() < deadline, "the next choice ends the follow");
        let _ = frame(&mut window, Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A real run showed a session with no entry after its publisher started again:
/// the server ended the follow, the session came back in the list, and the window
/// did not follow it again.
#[test]
fn a_follow_that_the_server_ended_starts_again_when_the_list_names_the_session_again() {
    use crate::app::session_mirror::MirrorKey;
    use horizon_core::session_mirror::{MirrorProgram, MirrorSession};
    use horizon_sync::{LiveEndReason, MirrorComputer};

    let session = "3f0c2a52-7d1e-4f5e-9a59-0b9a3e1f6c12";
    let listed = || LiveEvent::MirrorComputers {
        computers: vec![MirrorComputer {
            device: THERE.to_owned(),
            label: "There".to_owned(),
            sessions: vec![MirrorSession {
                program: MirrorProgram::Claude,
                session: session.to_owned(),
                title: "made-up session".to_owned(),
                folder: "repo".to_owned(),
                parent: "~/github".to_owned(),
                updated_at: 1_700_000_000_000,
                live: true,
            }],
            unreadable: Vec::new(),
            own: false,
        }],
    };
    let wanted = format!("{THERE}/{}/{session}", MirrorProgram::Claude.key());
    let follows = |window: &Window| window.link.relay().mirror_follows.contains(&wanted);
    let until = |window: &mut Window, what: &str, done: &dyn Fn(&Window) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done(window) {
            assert!(std::time::Instant::now() < deadline, "{what}");
            let _ = frame(window, Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };

    let mut window = Window::new();
    window.link.relay().events.push(listed());
    window.app.orchestrator_dock.front_desk_mut().show_mirror(MirrorKey {
        device: THERE.to_owned(),
        program: MirrorProgram::Claude,
        session: session.to_owned(),
    });
    until(&mut window, "the link follows the session of the dock", &follows);

    // The publisher left: the server ended the follow, and the link forgot it.
    {
        let mut relay = window.link.relay();
        relay.mirror_follows.clear();
        relay.events.push(LiveEvent::MirrorEnded {
            owner: THERE.to_owned(),
            program: MirrorProgram::Claude.key().to_owned(),
            session: session.to_owned(),
            reason: LiveEndReason::Offline,
        });
        relay.events.push(LiveEvent::MirrorComputers { computers: Vec::new() });
    }
    for _ in 0..40 {
        let _ = frame(&mut window, Vec::new());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !follows(&window),
        "a session that the list does not name is not asked for again"
    );

    // The publisher is back, and its list names the session again.
    window.link.relay().events.push(listed());
    until(&mut window, "the window follows the session again", &follows);
}

#[test]
fn the_sidebar_lists_the_other_computers_and_a_press_opens_the_follow_panel() {
    let mut window = Window::new();
    // This computer alone is no section, and neither is a link that never opened.
    window.link.relay().events.push(list(Vec::new()));
    window.link.relay().events.clear();
    let output = frame(&mut window, Vec::new());
    assert!(!drew_text(&output, "COMPUTERS"), "no other computer is listed");

    let panels = vec![
        shared("p-build", "build", "shell", ""),
        shared("p-review", "review the change", "claude", "working"),
    ];
    window.link.relay().events.push(list(panels));
    let output = until_drawn(&mut window, "COMPUTERS");
    for words in ["There", "build", "review the change", "Working"] {
        assert!(drew_text(&output, words), "the section draws {words}");
    }
    assert!(!drew_text(&output, "Here"), "this computer is not in its own section");

    // The row is a press target with a hover text of its own.
    let row = drawn_text(&output, "review the change").center();
    let hint = "Claude: review the change, working\nOn There, you can type in it";
    let mut hovered = frame(&mut window, vec![Event::PointerMoved(row)]);
    for seconds in [1000.0, 1001.0, 1001.5] {
        let mut input = raw_input(SIZE, Some([0.0, 0.0]));
        input.time = Some(seconds);
        hovered = run_app_frame_with_input(&window.ctx, &mut window.app, input);
    }
    assert!(
        drew_text(&hovered, hint),
        "the row shows its hover text: {:?}",
        texts(&hovered)
    );
    press(&mut window, row, PointerButton::Primary);
    let follow = follower_of(&window.app).expect("the press opened a follow panel");
    let panel = window.app.board.panel(follow).expect("the follow panel");
    let spec = panel.follow_spec().expect("its stream");
    assert_eq!(
        (spec.device.as_str(), spec.panel.as_str(), spec.kind, panel.kind),
        (THERE, "p-review", PanelKind::Claude, PanelKind::Shell),
        "the kind of the owner panel decides the keyboard protocol, and the panel itself runs nothing"
    );
    assert_eq!(window.app.board.focused, Some(follow));

    // The menu of the follow panel of an agent terminal offers no restart.
    let titlebar = window.app.panel_screen_rects[&follow].min + egui::vec2(80.0, 14.0);
    let menu = press(&mut window, titlebar, PointerButton::Secondary);
    assert!(drew_text(&menu, "New Workspace"), "the menu of the panel is open");
    for entry in ["Restart", "Reconnect"] {
        assert!(!drew_text(&menu, entry), "a follow panel has no {entry} entry");
    }

    // A link that is down says why in the section, and lists no computer. The
    // reason is a text of its own under the line, so the sidebar shows all of it.
    let reason = "live_replaced: another link took this name";
    window
        .link
        .relay()
        .events
        .push(LiveEvent::State(LiveState::Down(reason.to_owned())));
    let output = until_drawn(&mut window, "Shared terminals: no link");
    let galley = galley_of(&output, reason).expect("the reason of the down link is drawn under the line");
    assert!(!galley.elided, "the person reads the whole reason in the sidebar");
    assert_eq!(galley.rows.len(), 2, "the reason wraps under its own line");
    assert!(
        galley.rect.width() <= crate::app::SIDEBAR_WIDTH,
        "the reason stays inside the sidebar: {:?}",
        galley.rect
    );
    assert!(drew_text(&output, "COMPUTERS") && !drew_text(&output, "build"));
    let ended = window.app.board.panel(follow).and_then(Panel::follow_ended_line);
    assert!(ended.is_some(), "the follow panel says that its stream ended");
}

#[test]
fn the_title_bars_say_who_watches_a_terminal_and_where_a_followed_one_runs() {
    let mut window = Window::new();
    // The first frame gives the window its size, which a reveal reads.
    frame(&mut window, Vec::new());
    let owner = window.owner([200.0, 200.0]);
    let follow = window.follow(owner);
    window.shows(follow, "READY");

    window.app.reveal_selected_panel(&window.ctx.clone(), owner);
    until_drawn(&mut window, "seen by Here");
    window.app.reveal_selected_panel(&window.ctx.clone(), follow);
    until_drawn(&mut window, "on There");

    window.app.template_config.server.remote_typing = false;
    until_drawn(&mut window, "on There \u{00b7} view only");
}
