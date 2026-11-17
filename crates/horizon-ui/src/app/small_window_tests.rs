//! Horizon in a small window: whole-app frames at the smallest window that the
//! build opens, 800 by 600 points, and at the sizes between it and a wide screen.

use egui::{Event, Rect};
use horizon_core::{RuntimeState, StartupDecision, WorkspaceState};

use super::HorizonApp;
use super::test_support::{
    editor_workspace_state, presets_of, primary_modifier_press, raw_input, run_app_frame_with_input,
    test_app_with_startup,
};

/// The window of a whole application of one size, with the clock of its frames.
struct Window {
    ctx: egui::Context,
    app: HorizonApp,
    size: [f32; 2],
    time: f64,
    _temp: tempfile::TempDir,
}

/// One text that a frame painted.
#[derive(Clone, Debug)]
struct Drawn {
    text: String,
    rect: Rect,
}

impl Window {
    /// A window of `size` points on a board of `workspaces`, after the frames
    /// that settle its view.
    fn new(size: [f32; 2], workspaces: Vec<WorkspaceState>) -> Self {
        let (temp, ctx, app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces,
                ..RuntimeState::default()
            }),
        });
        let mut window = Self {
            ctx,
            app,
            size,
            time: 1.0,
            _temp: temp,
        };
        window.app.theme_applied = true;
        window.app.initial_pan_done = true;
        window.app.startup_workspace_organization_pending = false;
        window.app.root_viewport_stabilizer = None;
        window.ctx.enable_accesskit();
        for _ in 0..3 {
            let _ = window.frame(Vec::new());
        }
        window
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 0.016;
        let mut input = raw_input(self.size, None);
        input.time = Some(self.time);
        input.events = events;
        run_app_frame_with_input(&self.ctx, &mut self.app, input)
    }
}

/// Every text that `output` painted.
fn texts(output: &egui::FullOutput) -> Vec<Drawn> {
    fn walk(shape: &egui::Shape, out: &mut Vec<Drawn>) {
        match shape {
            egui::Shape::Text(text) => out.push(Drawn {
                text: text.galley.text().to_owned(),
                rect: Rect::from_min_size(text.pos, text.galley.size()),
            }),
            egui::Shape::Vec(inner) => inner.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// The chord of the command palette in the default settings, as the keyboard of
/// this system sends it.
fn palette_chord() -> Event {
    key(
        egui::Key::K,
        egui::Modifiers {
            shift: true,
            ..primary_modifier_press()
        },
    )
}

/// The box of the one text `text` of `drawn`.
fn text_of<'a>(drawn: &'a [Drawn], text: &str) -> &'a Drawn {
    drawn
        .iter()
        .find(|entry| entry.text == text)
        .unwrap_or_else(|| panic!("the frame draws {text:?}: {drawn:?}"))
}

#[test]
fn the_palette_of_the_shortcut_marks_and_orders_the_presets_as_the_panel_list_does() {
    let mut window = Window::new([1400.0, 900.0], vec![editor_workspace_state("beta", [520.0, 80.0])]);
    let folder = tempfile::tempdir().expect("a folder of the test");
    horizon_core::stand_in_program_in(folder.path(), "claude", "exit 0\n").expect("the program of a test");
    window.app.presets = presets_of(folder.path());

    let _ = window.frame(vec![palette_chord()]);
    assert!(window.app.command_palette.is_some(), "the chord opens the palette");

    // The search of the programs runs on the worker, so the list is unmarked
    // until its answer is here.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut rows;
    loop {
        rows = texts(&window.frame(Vec::new()));
        if rows.iter().any(|row| row.text == "not installed") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the palette marks the preset whose program this computer lacks: {:?}",
            rows.iter().map(|row| row.text.as_str()).collect::<Vec<_>>()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let (shell, claude, codex) = (
        text_of(&rows, "Shell").rect,
        text_of(&rows, "Claude Code").rect,
        text_of(&rows, "Codex").rect,
    );
    assert!(
        shell.top() < claude.top() && claude.top() < codex.top(),
        "a preset without its program stands last: {shell:?} {claude:?} {codex:?}"
    );
    let marks: Vec<Rect> = rows
        .iter()
        .filter(|row| row.text == "not installed")
        .map(|row| row.rect)
        .collect();
    assert_eq!(marks.len(), 1, "one preset without its program: {marks:?}");
    assert!(
        (marks[0].center().y - codex.center().y).abs() < 1.0 && marks[0].left() > codex.right(),
        "the words stand at the right of the row of Codex: {marks:?} {codex:?}"
    );
}
