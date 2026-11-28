//! Horizon in a small window: whole-app frames at the smallest window that the
//! build opens, 800 by 600 points, and at the sizes between it and a wide screen.

use egui::{Event, Pos2, Rect};
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
    temp: tempfile::TempDir,
}

/// One text that a frame painted.
#[derive(Clone, Debug)]
struct Drawn {
    text: String,
    rect: Rect,
    /// The rectangle that the frame cuts the text to.
    clip: Rect,
    /// Whether the text ends in the mark of a cut.
    cut: bool,
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
            temp,
        };
        window.app.theme_applied = true;
        window.app.initial_pan_done = true;
        window.app.startup_workspace_organization_pending = false;
        window.app.root_viewport_stabilizer = None;
        // These windows measure the canvas and the first start page, so they keep
        // no dock at their right edge.
        window.app.orchestrator_dock = crate::app::orchestrator_dock::OrchestratorDock::new(false);
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

    fn screen(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(self.size[0], self.size[1]))
    }
}

/// Every text that `output` painted.
fn texts(output: &egui::FullOutput) -> Vec<Drawn> {
    fn walk(shape: &egui::Shape, clip: Rect, out: &mut Vec<Drawn>) {
        match shape {
            egui::Shape::Text(text) => out.push(Drawn {
                text: text.galley.text().to_owned(),
                rect: Rect::from_min_size(text.pos, text.galley.size()),
                clip,
                cut: text.galley.elided,
            }),
            egui::Shape::Vec(inner) => inner.iter().for_each(|shape| walk(shape, clip, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut out);
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

/// The card of the command palette: the one filled rectangle with its corners.
fn palette_card(output: &egui::FullOutput) -> Rect {
    fn walk(shape: &egui::Shape, out: &mut Option<Rect>) {
        match shape {
            egui::Shape::Rect(rect)
                if rect.corner_radius == egui::CornerRadius::same(20) && rect.fill != egui::Color32::TRANSPARENT =>
            {
                *out = Some(rect.rect);
            }
            egui::Shape::Vec(inner) => inner.iter().for_each(|shape| walk(shape, out)),
            _ => {}
        }
    }
    let mut card = None;
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut card);
    }
    card.expect("the frame draws the card of the palette")
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

#[test]
fn the_palette_and_the_preset_list_stay_inside_a_small_window() {
    for size in [[1024.0, 640.0], [800.0, 600.0]] {
        for presets_only in [false, true] {
            let mut window = Window::new(size, vec![editor_workspace_state("beta", [520.0, 80.0])]);
            let title = if presets_only {
                window.app.open_preset_palette();
                "Presets"
            } else {
                let _ = window.frame(vec![palette_chord()]);
                "Command Palette"
            };
            let _ = window.frame(Vec::new());
            let output = window.frame(Vec::new());
            let screen = window.screen();
            let card = palette_card(&output);
            assert!(
                screen.contains_rect(card),
                "the {title} card {card:?} lies inside the window {screen:?}"
            );
            let drawn = texts(&output);
            for whole in [title, "Type to search..."] {
                let text = text_of(&drawn, whole);
                assert!(
                    card.contains_rect(text.rect) && text.clip.contains_rect(text.rect),
                    "the {title} card {card:?} shows {whole:?} whole: {text:?}"
                );
            }
            // The rows are cut to the list, which ends inside the card, so a list
            // that is longer than the card scrolls there.
            let row = text_of(&drawn, "Shell");
            assert!(
                card.contains_rect(row.clip),
                "the list of {title} {:?} ends inside the card {card:?}",
                row.clip
            );
        }
    }
}

/// The states that a panel row names, with the ledger of each.
const STATES: [(horizon_core::agent_work::TurnState, u32, &str); 4] = [
    (horizon_core::agent_work::TurnState::Working, 0, "Working"),
    (horizon_core::agent_work::TurnState::Finished, 1, "Background"),
    (horizon_core::agent_work::TurnState::Finished, 0, "Done"),
    (horizon_core::agent_work::TurnState::Blocked, 0, "Needs you"),
];

/// The line that the stand-in agent prints on its screen.
const SCREEN_LINE: &str = "synthetic line of the screen";
/// The stand-in agent: it prints one line and stays until the panel closes.
const AGENT_PROGRAM: &str = "printf 'synthetic line of the screen\\n'; exec cat\n";

/// A window of 800 by 600 points with one agent panel of a long name, which
/// the keyboard is not in.
fn window_with_an_agent() -> (Window, horizon_core::PanelId) {
    agent_window([800.0, 600.0])
}

/// A window of `size` points with one agent panel of a long name and of the
/// default size of a panel, which the keyboard is not in.
fn agent_window(size: [f32; 2]) -> (Window, horizon_core::PanelId) {
    let mut window = Window::new(size, Vec::new());
    // A program that stays until the panel closes, so the state of the row is
    // the state of the ledger and not the end of a program.
    let program = horizon_core::stand_in_program_in(window.temp.path(), "claude", AGENT_PROGRAM)
        .expect("the program of the test");
    let workspace = window.app.board.create_workspace("Lane");
    let panel = window
        .app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                name: Some("Atlas checkout redesign lane".to_owned()),
                kind: horizon_core::PanelKind::Claude,
                command: Some(program.display().to_string()),
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .expect("the panel of the test");
    window.app.board.focused = None;
    // The panel rows of the work item stand under an open fold.
    let local_id = window
        .app
        .board
        .workspace(workspace)
        .expect("the workspace")
        .local_id
        .clone();
    window.app.sidebar_chrome.folds.toggle(&local_id, false);
    (window, panel)
}

#[test]
fn the_sidebar_of_a_small_window_shows_its_header_whole() {
    let (mut window, _panel) = window_with_an_agent();
    let canvas_left = window.app.canvas_rect(&window.ctx).left();
    let drawn = texts(&window.frame(Vec::new()));
    let header = text_of(&drawn, "WORK ITEMS");
    assert!(!header.cut, "the caption shows whole: {header:?}");
    let count = text_of(&drawn, "1");
    assert!(
        header.rect.right() < count.rect.left() && count.rect.right() < canvas_left,
        "the count stands after the caption and inside the sidebar: {header:?} {count:?}"
    );
}

#[test]
fn a_panel_row_of_a_small_window_holds_its_whole_state_word() {
    let (mut window, panel) = window_with_an_agent();
    let local = window.app.board.panel(panel).expect("the panel").local_id.clone();
    let canvas_left = window.app.canvas_rect(&window.ctx).left();
    let now = horizon_core::current_unix_millis();
    for (index, (state, background_tasks, word)) in STATES.into_iter().enumerate() {
        let mut ledger = horizon_core::agent_work::TurnLedger::default();
        ledger.session_id = "session".into();
        ledger.prompt_id = Some("prompt".into());
        ledger.state = state;
        ledger.background_tasks = background_tasks;
        // The panel saw a turn before the one that ends here, so its end is unseen.
        ledger.updated_at_millis = now - 4 + i64::try_from(index).expect("index");
        let ctx = window.ctx.clone();
        window
            .app
            .apply_attention_values(vec![(panel, local.clone(), Some(ledger))], &ctx);
        let _ = window.frame(Vec::new());
        let drawn = texts(&window.frame(Vec::new()));
        let name = drawn
            .iter()
            .find(|entry| entry.text.starts_with("Atlas") && entry.rect.right() < canvas_left)
            .unwrap_or_else(|| panic!("the sidebar draws the name of the panel: {drawn:?}"));
        let row = name.rect.top()..=name.rect.bottom();
        let state = drawn
            .iter()
            .find(|entry| entry.text.starts_with(&word[..2]) && row.contains(&entry.rect.center().y))
            .unwrap_or_else(|| panic!("the row draws the word {word:?} of its state: {drawn:?}"));
        assert!(
            !state.cut && state.text == word,
            "the row holds its whole state word {word:?}: {state:?}"
        );
        assert!(
            name.cut && name.rect.right() < state.rect.left() && state.rect.right() < canvas_left,
            "the name is cut before the word of the state, inside the sidebar: {name:?} {state:?}"
        );
    }
}

/// Whether `output` holds the minimap, by the name that it tells a reader.
fn draws_the_minimap(output: &egui::FullOutput) -> bool {
    let update = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("the window tells its controls");
    update
        .nodes
        .iter()
        .any(|(_, node)| node.label() == Some("Workspace minimap"))
}

#[test]
fn the_minimap_leaves_a_small_canvas_and_comes_back_on_a_large_one() {
    let mut window = Window::new([800.0, 600.0], vec![editor_workspace_state("beta", [520.0, 80.0])]);
    for (size, shows) in [
        ([800.0, 600.0], false),
        ([1280.0, 800.0], true),
        ([800.0, 600.0], false),
        ([1024.0, 640.0], false),
        ([1206.0, 640.0], true),
    ] {
        window.size = size;
        let _ = window.frame(Vec::new());
        let output = window.frame(Vec::new());
        assert_eq!(
            draws_the_minimap(&output),
            shows,
            "the minimap of a window of {size:?} on the canvas {:?}",
            window.app.canvas_rect(&window.ctx)
        );
        assert_eq!(
            window.app.minimap_overlay_rect(&window.ctx).is_some(),
            shows,
            "the room that the canvas keeps free for the minimap at {size:?}"
        );
    }
}

/// The rectangle of `panel` in the window.
fn panel_rect(window: &Window, panel: horizon_core::PanelId) -> Rect {
    let layout = &window.app.board.panel(panel).expect("the panel").layout;
    let canvas = window.app.canvas_rect(&window.ctx);
    let min = window.app.canvas_to_screen(canvas, Pos2::from(layout.position));
    Rect::from_min_size(min, window.app.canvas_size_to_screen(egui::Vec2::from(layout.size)))
}

#[test]
fn the_minimap_leaves_a_canvas_with_no_room_for_a_panel_beside_it() {
    for size in [[1024.0, 640.0], [1100.0, 700.0]] {
        let (mut window, panel) = agent_window(size);
        // The person moved the canvas: the panel stands whole in the lower right
        // corner of the canvas, which is the corner of the minimap.
        let canvas = window.app.canvas_rect(&window.ctx);
        let layout = window.app.board.panel(panel).expect("the panel").layout;
        window.app.canvas_view.set_zoom(1.0);
        window.app.canvas_view.set_pan_offset([
            canvas.width() - 24.0 - layout.position[0] - layout.size[0],
            canvas.height() - 24.0 - layout.position[1] - layout.size[1],
        ]);
        let _ = window.frame(Vec::new());
        let output = window.frame(Vec::new());
        let (canvas, shown) = (window.app.canvas_rect(&window.ctx), panel_rect(&window, panel));
        assert!(
            canvas.contains_rect(shown),
            "the panel {shown:?} stands whole in the canvas {canvas:?} at {size:?}"
        );
        let map = window.app.minimap_overlay_rect(&window.ctx);
        assert!(
            !draws_the_minimap(&output) && map.is_none(),
            "the minimap {map:?} covers the panel {shown:?} on the canvas {canvas:?} at {size:?}"
        );
    }
    // A canvas of three maps has the room of a jump for a panel of the default
    // size beside the map: the map shows, and a jump puts the panel clear of it.
    for size in [[1206.0, 640.0], [1280.0, 800.0]] {
        let (mut window, panel) = agent_window(size);
        let ctx = window.ctx.clone();
        window.app.reveal_selected_panel(&ctx, panel);
        let _ = window.frame(Vec::new());
        let output = window.frame(Vec::new());
        let (canvas, shown) = (window.app.canvas_rect(&window.ctx), panel_rect(&window, panel));
        let map = window
            .app
            .minimap_overlay_rect(&window.ctx)
            .unwrap_or_else(|| panic!("the canvas {canvas:?} of {size:?} shows the minimap"));
        assert!(
            draws_the_minimap(&output) && canvas.contains_rect(shown) && !map.intersects(shown),
            "a jump shows the panel {shown:?} whole and clear of the minimap {map:?} at {size:?}"
        );
    }
}

#[test]
fn the_foot_line_of_the_first_page_wraps_in_a_small_window() {
    for size in [[800.0, 600.0], [1000.0, 700.0], [1280.0, 800.0]] {
        let mut window = Window::new(size, Vec::new());
        let drawn = texts(&window.frame(Vec::new()));
        let ctx = window.ctx.clone();
        let canvas = window.app.canvas_rect(&ctx);
        let (block_height, _) = window.app.welcome_block_heights_for_test(&ctx);
        // The block of the page keeps 60 points to each edge of the canvas.
        let (left, right) = (canvas.left() + 60.0, canvas.right() - 60.0);
        let heading = text_of(&drawn, "What do you want to do?");
        let primary = super::util::primary_shortcut_label();
        let shortcuts = window.app.shortcuts.clone();
        let mut last = text_of(&drawn, "You know Horizon already?").rect;
        for (label, binding) in [
            ("New terminal", shortcuts.new_terminal),
            ("Quick Nav", shortcuts.command_palette),
            ("Machines you have", shortcuts.open_remote_hosts),
        ] {
            let keys = binding.display_label(primary);
            let (label, keys) = (text_of(&drawn, label), text_of(&drawn, &keys));
            for part in [label, keys] {
                assert!(
                    part.rect.left() >= left - 0.6 && part.rect.right() <= right + 0.6,
                    "the foot line keeps {:?} inside the block {left}..{right} at {size:?}: {:?}",
                    part.text,
                    part.rect
                );
                // A part stands after the part before it on its row, or on a row
                // under it.
                assert!(
                    part.rect.left() > last.right() || part.rect.top() > last.bottom() - 4.0,
                    "the foot line puts {:?} after {last:?} at {size:?}: {:?}",
                    part.text,
                    part.rect
                );
                last = part.rect;
            }
            assert!(
                (label.rect.center().y - keys.rect.center().y).abs() < 4.0,
                "a key stays on the row of its name at {size:?}: {label:?} {keys:?}"
            );
        }
        // The block of the page holds every row of the line, so a page that
        // scrolls shows the last row above its lower pad.
        assert!(
            last.bottom() <= heading.rect.top() - 8.0 + block_height + 0.6,
            "the block of {block_height} points from {} holds the last row {last:?} at {size:?}",
            heading.rect.top() - 8.0
        );
    }
}

#[test]
fn the_second_line_of_a_workspace_row_holds_facts_and_no_screen_text() {
    let (mut window, panel) = window_with_an_agent();
    let local = window.app.board.panel(panel).expect("the panel").local_id.clone();
    let workspace = window.app.board.panel(panel).expect("the panel").workspace_id;
    let canvas_left = window.app.canvas_rect(&window.ctx).left();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let _ = window.frame(Vec::new());
        let screen = window
            .app
            .board
            .panel(panel)
            .and_then(horizon_core::Panel::terminal)
            .map_or_else(String::new, |terminal| terminal.last_lines_text(20));
        if screen.contains(SCREEN_LINE) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the stand-in agent prints its line"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // The rows are read again when they are a second old.
    let sidebar = |window: &mut Window| -> Vec<String> {
        window
            .app
            .refresh_sidebar_rows(std::time::Instant::now() + std::time::Duration::from_secs(2));
        texts(&window.frame(Vec::new()))
            .into_iter()
            .filter(|entry| entry.rect.right() < canvas_left)
            .map(|entry| entry.text)
            .collect()
    };
    let rows = sidebar(&mut window);
    assert!(
        rows.iter().any(|text| text == "Lane") && !rows.iter().any(|text| text.contains("synthetic")),
        "the row of the workspace shows no text of the screen of its panel: {rows:?}"
    );

    // The facts of the workspace: the agents that wait, and its machine.
    let mut ledger = horizon_core::agent_work::TurnLedger::default();
    ledger.session_id = "session".into();
    ledger.prompt_id = Some("prompt".into());
    ledger.state = horizon_core::agent_work::TurnState::Blocked;
    ledger.updated_at_millis = horizon_core::current_unix_millis() - 1;
    let ctx = window.ctx.clone();
    window
        .app
        .apply_attention_values(vec![(panel, local, Some(ledger))], &ctx);
    let rows = sidebar(&mut window);
    assert!(
        rows.iter().any(|text| text == "1 needs you"),
        "the row says that one agent needs the person: {rows:?}"
    );
    window
        .app
        .board
        .set_workspace_place(workspace, horizon_core::WorkspacePlace::Machine("build-1".to_owned()));
    let rows = sidebar(&mut window);
    assert!(
        rows.iter().any(|text| text == "1 needs you \u{00B7} on build-1"),
        "the row says where the workspace runs after the count: {rows:?}"
    );
}

mod jump;
mod resume_banner;
mod session;
