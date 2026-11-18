//! The close of a panel, and of every panel of a workspace, of a machine,
//! through the sidebar that a person uses.

use horizon_core::{Machine, MachineRegistry, Machines, PanelOptions, WorkspacePlace, stand_in_program};

use super::super::SidebarActions;

/// A Horizon whose workspace is placed on `build-1`, with one terminal of it,
/// which runs the stand-in program so the test starts no ssh.
fn app_with_a_machine_terminal() -> (tempfile::TempDir, crate::app::HorizonApp, horizon_core::PanelId) {
    let (temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("work");
    app.board
        .workspaces
        .iter_mut()
        .find(|candidate| candidate.id == workspace)
        .expect("the workspace")
        .local_id = "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a".to_owned();
    app.board
        .set_workspace_place(workspace, WorkspacePlace::Machine("build-1".to_owned()));
    app.machine_list = Machines::with_records(
        MachineRegistry::from_home(app.session_store.home()),
        vec![Machine {
            name: "build-1".to_owned(),
            host: "203.0.113.10".to_owned(),
            ..Machine::default()
        }],
    );
    let panel = app
        .create_panel_with_options(
            PanelOptions {
                launch_wrapper: vec![stand_in_program().to_owned()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a terminal of the machine");
    (temp, app, panel)
}

#[test]
fn the_close_of_the_row_of_a_panel_of_a_machine_asks_before_the_session_ends() {
    let (_temp, mut app, panel) = app_with_a_machine_terminal();
    let ctx = egui::Context::default();

    // The close control of a row and the "Close" of its menu both hand this
    // action to the sidebar, which applies it here.
    let actions = SidebarActions {
        close_panel: Some(panel),
        ..SidebarActions::default()
    };
    app.apply_sidebar_actions(&ctx, &actions);
    assert!(app.board.panel(panel).is_some(), "the close of the row asks first");
    assert_eq!(app.close_panel_confirm, Some(panel), "the row asks for the one close");
    assert!(app.session_kills.is_empty(), "no session ends before the answer");

    app.answer_close_panel(panel, true);
    assert!(app.board.panel(panel).is_none(), "the answered close ends the panel");
    assert_eq!(
        app.session_kills
            .iter()
            .map(|kill| kill.session.as_str())
            .collect::<Vec<_>>(),
        ["hz-c8dfe3af-1"],
        "the answered close ends the session of the panel"
    );
}

// The close of every panel of a workspace asks the same question as the close of
// one panel, and the question stands over the window. These cases run frames of
// the whole window, so a press that a workspace toolbar lies under is a press
// that the person makes.

use egui::{Context, Pos2};

use crate::app::HorizonApp;
use crate::app::test_support::{raw_input, run_app_frame_with_input, toolbar_under};

/// One frame of the whole window, as eframe runs it, with `events` as its input.
fn frame(ctx: &Context, app: &mut HorizonApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = events;
    run_app_frame_with_input(ctx, app, input)
}

fn pointer(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One press of `pos`: the frame that takes it down and the frame that lets it up.
fn press(ctx: &Context, app: &mut HorizonApp, pos: Pos2) -> egui::FullOutput {
    frame(ctx, app, vec![pointer(pos, true)]);
    frame(ctx, app, vec![pointer(pos, false)])
}

/// `frames` frames of the window with no input, and the output of the last one.
fn settle(ctx: &Context, app: &mut HorizonApp, frames: usize) -> egui::FullOutput {
    let mut output = frame(ctx, app, Vec::new());
    for _ in 1..frames {
        output = frame(ctx, app, Vec::new());
    }
    output
}

/// The drawn texts of `output` with their centre.
fn texts(output: &egui::FullOutput) -> impl Iterator<Item = (&str, Pos2)> {
    output.shapes.iter().filter_map(|shape| match &shape.shape {
        egui::Shape::Text(text) => Some((text.galley.text(), text.pos + text.galley.size() * 0.5)),
        _ => None,
    })
}

/// The line of the question about the sessions of a workspace, when the frame
/// draws it.
fn workspace_question(output: &egui::FullOutput) -> Option<&str> {
    texts(output)
        .map(|(text, _)| text)
        .find(|text| text.ends_with("this workspace's machine?"))
}

/// The centre of the button `label` of the question: the one nearest to its line.
fn question_button(output: &egui::FullOutput, label: &str) -> Pos2 {
    let line = texts(output)
        .find(|(text, _)| text.ends_with("this workspace's machine?"))
        .map(|(_, centre)| centre)
        .expect("the question says what ends before it offers a button");
    texts(output)
        .filter(|(text, _)| *text == label)
        .map(|(_, centre)| centre)
        .min_by(|left, right| left.distance(line).total_cmp(&right.distance(line)))
        .unwrap_or_else(|| panic!("the question offers {label}"))
}

/// The centre of the widget that speaks `label` to a screen reader.
fn spoken_centre(ctx: &Context, app: &mut HorizonApp, label: &str) -> Pos2 {
    ctx.enable_accesskit();
    // The tree of a frame holds the widgets of the frame before it.
    let mut bounds = None;
    for _ in 0..2 {
        let output = frame(ctx, app, Vec::new());
        let nodes = output
            .platform_output
            .accesskit_update
            .expect("the accessibility tree of the window");
        bounds = nodes.nodes.iter().find_map(|(_, node)| {
            let text = node.label().or_else(|| node.value())?;
            // A row speaks its name and its panel count, as `work, 1 panel`.
            (text == label || text.starts_with(&format!("{label},")))
                .then(|| node.bounds())
                .flatten()
        });
    }
    let bounds = bounds.unwrap_or_else(|| panic!("the window speaks {label:?}"));
    #[allow(clippy::cast_possible_truncation)]
    Pos2::new(
        f64::midpoint(bounds.x0, bounds.x1) as f32,
        f64::midpoint(bounds.y0, bounds.y1) as f32,
    )
}

/// The question that the menu of the workspace row `label` opens, as the frame
/// that draws it draws it.
fn open_the_question(ctx: &Context, app: &mut HorizonApp, label: &str) -> egui::FullOutput {
    let row = spoken_centre(ctx, app, label);
    let secondary = |pressed| egui::Event::PointerButton {
        pos: row,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(ctx, app, vec![secondary(true)]);
    frame(ctx, app, vec![secondary(false)]);
    let entry = spoken_centre(ctx, app, "Close All Panels");
    press(ctx, app, entry);
    settle(ctx, app, 1)
}

#[test]
fn the_menu_of_a_workspace_of_a_machine_opens_the_question_over_the_window() {
    let (_temp, mut app, panel) = app_with_a_machine_terminal();
    let workspace = app.board.panel(panel).expect("the panel").workspace_id;
    let ctx = Context::default();

    let output = open_the_question(&ctx, &mut app, "work");

    assert_eq!(
        app.close_workspace_confirm,
        Some(workspace),
        "the entry of the menu opens the question"
    );
    let drawn: Vec<&str> = texts(&output).map(|(text, _)| text).collect();
    assert!(
        !drawn.contains(&"Arrange Panels"),
        "the menu closed, so the question stands alone: {drawn:?}"
    );
    assert_eq!(
        workspace_question(&output),
        Some("End 1 tmux session(s) of this workspace's machine?"),
        "the question keeps the words that the menu showed"
    );
    assert!(
        app.board.panel(panel).is_some(),
        "the question ends no panel before the person answers"
    );
    assert!(
        app.session_kills.is_empty(),
        "the question ends no session before the person answers"
    );
}

#[test]
fn the_question_of_a_close_all_takes_a_press_over_a_workspace_toolbar() {
    // The toolbar of a workspace is a layer of the window that stands over the
    // middle of the canvas, where the question draws its buttons: a press on a
    // button that a toolbar covers belongs to the question.
    for (label, closes) in [("Close All Panels", true), ("Cancel", false)] {
        let (_temp, mut app, panel) = app_with_a_machine_terminal();
        let ctx = Context::default();
        let output = open_the_question(&ctx, &mut app, "work");
        let button = question_button(&output, label);
        toolbar_under(&ctx, &mut app, button);

        press(&ctx, &mut app, button);

        assert_eq!(
            app.close_workspace_confirm, None,
            "the press on {label} arrives where a workspace toolbar stands"
        );
        assert_eq!(
            app.board.panel(panel).is_some(),
            !closes,
            "the answer of the press on {label}"
        );
        let ended: Vec<&str> = app.session_kills.iter().map(|kill| kill.session.as_str()).collect();
        assert_eq!(
            ended,
            if closes { vec!["hz-c8dfe3af-1"] } else { Vec::new() },
            "the press on {label} ends the sessions only when it closes"
        );
        // The test reads the kill instead of letting the next frame hand it to
        // the worker that runs ssh.
        app.session_kills.clear();
    }
}

#[test]
fn the_question_of_a_close_all_ends_on_escape_and_on_a_press_beside_it() {
    for escape in [true, false] {
        let (_temp, mut app, panel) = app_with_a_machine_terminal();
        let ctx = Context::default();
        let output = open_the_question(&ctx, &mut app, "work");
        let line = texts(&output)
            .find(|(text, _)| text.ends_with("this workspace's machine?"))
            .map(|(_, centre)| centre)
            .expect("the question draws its line");

        if escape {
            frame(
                &ctx,
                &mut app,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        } else {
            // A press beside the question is the press on its backdrop.
            press(&ctx, &mut app, Pos2::new(line.x - 260.0, line.y));
        }

        assert_eq!(
            app.close_workspace_confirm, None,
            "the escape key or a press beside the question cancels it"
        );
        assert!(
            app.board.panel(panel).is_some(),
            "a question that is dropped keeps the panel"
        );
        assert!(
            app.session_kills.is_empty(),
            "a question that is dropped keeps the session"
        );
    }
}

#[test]
fn a_workspace_with_no_panel_of_a_machine_closes_at_once_with_no_question() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("work");
    let panel = app
        .create_panel_with_options(
            PanelOptions {
                launch_wrapper: vec![stand_in_program().to_owned()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a terminal of this computer");
    let ctx = Context::default();

    let output = open_the_question(&ctx, &mut app, "work");

    assert_eq!(
        app.close_workspace_confirm, None,
        "a workspace of this computer asks nothing"
    );
    assert_eq!(
        workspace_question(&output),
        None,
        "a workspace of this computer draws no question"
    );
    assert!(
        app.board.panel(panel).is_none(),
        "the entry closes every panel of this computer at once"
    );
    assert!(
        app.session_kills.is_empty(),
        "a panel of this computer holds no session"
    );
}
