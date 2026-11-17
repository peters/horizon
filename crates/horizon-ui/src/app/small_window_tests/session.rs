//! A session switch inside one window: the window keeps its size and its place,
//! and only the start of the program takes them from the settings.

use egui::{Event, ViewportCommand};

use super::{Window, key, texts};
use crate::app::test_support::{editor_workspace_state, primary_modifier_press};

/// The size of the window of the person, which is not the size of the settings.
const SIZE: [f32; 2] = [1100.0, 700.0];

/// The commands of `output` that give the window another size or another place.
fn geometry_commands(output: &egui::FullOutput) -> Vec<ViewportCommand> {
    output
        .viewport_output
        .values()
        .flat_map(|viewport| viewport.commands.iter())
        .filter(|command| {
            matches!(
                command,
                ViewportCommand::InnerSize(_) | ViewportCommand::OuterPosition(_)
            )
        })
        .cloned()
        .collect()
}

#[test]
fn a_new_session_of_the_sessions_overlay_keeps_the_size_of_the_window() {
    let mut window = Window::new(SIZE, vec![editor_workspace_state("old", [40.0, 40.0])]);
    let settings = window.app.template_config.window.clone();
    assert!(
        (settings.width - SIZE[0]).abs() > 100.0 && (settings.height - SIZE[1]).abs() > 100.0,
        "the window of the test is not the window of the settings: {settings:?}"
    );
    assert!(
        (window.app.window_config.width - SIZE[0]).abs() <= 1.0,
        "the frames before the switch read the size of the window: {:?}",
        window.app.window_config
    );

    // The chord of the Sessions overlay in the default settings.
    let chord = key(
        egui::Key::J,
        egui::Modifiers {
            shift: true,
            ..primary_modifier_press()
        },
    );
    let _ = window.frame(vec![chord]);
    window.app.settle_session_jobs();
    // The card of the overlay takes its height in the frames after the list.
    for _ in 0..3 {
        let _ = window.frame(Vec::new());
    }
    let drawn = texts(&window.frame(Vec::new()));
    let button = drawn
        .iter()
        .find(|entry| entry.text == "Open New Session")
        .unwrap_or_else(|| panic!("the overlay draws its button: {drawn:?}"))
        .rect
        .center();
    let press = |pressed| Event::PointerButton {
        pos: button,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };

    // The create of the session and the end of the old board run on workers, so
    // the frames go on until the board of the new session stands.
    let mut commands = Vec::new();
    let mut events = vec![vec![Event::PointerMoved(button)], vec![press(true)], vec![press(false)]];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while window.app.board.workspace_id_by_local_id("old").is_some() || window.app.session_manager.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the new session replaces the board of the old one"
        );
        let output = window.frame(if events.is_empty() {
            Vec::new()
        } else {
            events.remove(0)
        });
        commands.extend(geometry_commands(&output));
        window.app.settle_session_jobs();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // The frames after the switch, which a window uses to settle its view.
    window.app.root_viewport_stabilizer = None;
    for _ in 0..3 {
        commands.extend(geometry_commands(&window.frame(Vec::new())));
    }

    assert!(
        commands.is_empty(),
        "a switch inside one window sends the window no size and no place: {commands:?}"
    );
    let kept = &window.app.window_config;
    assert!(
        (kept.width - SIZE[0]).abs() <= 1.0 && (kept.height - SIZE[1]).abs() <= 1.0,
        "the new session keeps the size of the window that shows it: {kept:?}"
    );
}
