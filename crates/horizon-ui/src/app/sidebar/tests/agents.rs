//! The `Agents` group of the sidebar and the button `+ Rent a machine` at the end
//! of the machine section.
//!
//! The group stands there only while the orchestrator dock is on, and it says
//! which of the four agent programs of `horizon_core::agents` this computer has.
//! A press on the button sends one message to the orchestrator of the active
//! workspace, and it sends nothing while no workspace is active or while that
//! orchestrator works.

use egui::{Event, FullOutput, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::PanelKind;

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::sidebar::agents::{AgentPrograms, FOUND, GROUP, LOOKING, NOT_FOUND, PROGRAMS};
use crate::app::sidebar::rent_machine::{LABEL, MESSAGE, NO_WORKSPACE};
use crate::test_egui::DiscardTextures;

/// The stand-in of an orchestrator that names its session and then waits for the
/// lines of the person, so its turn stays open while the dock holds it.
const SILENT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
while IFS= read -r line; do :; done
"#;

/// One text that a frame of the sidebar painted, with its rectangle.
type Drawn = (String, Rect);

/// The frames of the sidebar of `app` at the default size of these tests.
struct Sidebar {
    ctx: egui::Context,
}

impl Sidebar {
    fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
        }
    }

    /// Draws the sidebar with `events` and answers the texts that it painted.
    fn draw(&self, app: &mut HorizonApp, events: Vec<Event>) -> Vec<Drawn> {
        let mut input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        input.events = events;
        let output: FullOutput = self
            .ctx
            .run_ui(input, |ui| app.render_sidebar(ui.ctx()))
            .discard_textures();
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.job.text.clone(),
                    Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    /// Draws the sidebar until its rows stand, and answers the last frame.
    fn settled(&self, app: &mut HorizonApp) -> Vec<Drawn> {
        let mut drawn = Vec::new();
        for _ in 0..3 {
            drawn = self.draw(app, Vec::new());
        }
        drawn
    }

    /// Clicks at `pos`: the pointer arrives, presses and releases.
    fn click(&self, app: &mut HorizonApp, pos: Pos2) {
        let _ = self.draw(app, vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            let _ = self.draw(
                app,
                vec![Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }],
            );
        }
    }
}

/// Reads the worker of the dock until the program of `local_id` runs, which the
/// start of a press does on that worker.
fn wait_for_bridge(app: &mut HorizonApp, local_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !app.orchestrator_dock.running(local_id) {
        app.poll_orchestrator_dock();
        assert!(
            std::time::Instant::now() < deadline,
            "the press started the program of that conversation"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Every rectangle where `drawn` painted `text`.
fn rects_of<'a>(drawn: &'a [Drawn], text: &str) -> Vec<&'a Rect> {
    drawn
        .iter()
        .filter(|(painted, _)| painted == text)
        .map(|(_, rect)| rect)
        .collect()
}

/// The box of the one text `text` of `drawn`.
fn box_of(drawn: &[Drawn], text: &str) -> Rect {
    let rects = rects_of(drawn, text);
    assert_eq!(rects.len(), 1, "the frame draws {text} once: {drawn:?}");
    *rects[0]
}

/// An app whose setting `features.orchestrator_dock` holds `dock_on` and whose
/// search of the programs answered `found`.
fn app_with(found: [bool; 4], dock_on: bool) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.orchestrator_dock = dock_on;
    app.orchestrator_dock = OrchestratorDock::following(dock_on);
    app.agent_programs = AgentPrograms::answered(found);
    (temp, app)
}

/// An app of [`app_with`] whose orchestrator runs `program` instead of the agent
/// program of a person.
fn app_with_program(program: std::path::PathBuf) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = app_with([true; 4], true);
    app.orchestrator_dock = OrchestratorDock::with_program(true, program);
    (temp, app)
}

#[test]
fn the_group_of_the_agents_is_absent_while_the_dock_is_off() {
    let (_temp, mut app) = app_with([true; 4], false);
    let drawn = Sidebar::new().settled(&mut app);

    assert!(rects_of(&drawn, GROUP).is_empty(), "no group: {drawn:?}");
    assert!(rects_of(&drawn, FOUND).is_empty(), "no row of a program: {drawn:?}");
    assert!(rects_of(&drawn, LABEL).is_empty(), "no button: {drawn:?}");
}

#[test]
fn the_group_of_the_agents_says_which_programs_this_computer_has() {
    // The dock of this app follows the setting of Settings, which the test turns
    // on in the configuration of the window, so the group arrives without a
    // restart.
    let (_temp, mut app) = app_with([true, false, true, false], true);
    let drawn = Sidebar::new().settled(&mut app);

    assert_eq!(rects_of(&drawn, GROUP).len(), 1, "one header: {drawn:?}");
    assert_eq!(rects_of(&drawn, "4").len(), 1, "the count of the rows: {drawn:?}");
    assert_eq!(rects_of(&drawn, FOUND).len(), 2, "two programs are here: {drawn:?}");
    assert_eq!(rects_of(&drawn, NOT_FOUND).len(), 2, "two are not: {drawn:?}");

    // The rows stand in the order of the design, one under the other, and each
    // name keeps its state line under it.
    let names: Vec<&str> = PROGRAMS.into_iter().map(PanelKind::display_name).collect();
    let boxes: Vec<Rect> = names.iter().map(|name| box_of(&drawn, name)).collect();
    for pair in boxes.windows(2) {
        assert!(pair[0].bottom() < pair[1].top(), "the rows stand in order: {boxes:?}");
    }
    let found = *rects_of(&drawn, FOUND)[0];
    assert!(
        (found.top() - boxes[0].bottom()).abs() < 6.0,
        "the line of the state is under the name of its row: {found:?} {:?}",
        boxes[0]
    );
    assert!(
        (found.left() - boxes[0].left()).abs() < 0.5,
        "the two lines of a row start together: {found:?} {:?}",
        boxes[0]
    );
}

/// A group whose search has not answered says `looking` on every row, and no row
/// claims a program that this computer lacks.
#[test]
fn the_group_of_the_agents_says_looking_until_the_search_answers() {
    let (_temp, mut app) = app_with([true; 4], true);
    app.agent_programs = AgentPrograms::default();
    let drawn = Sidebar::new().settled(&mut app);

    assert_eq!(
        rects_of(&drawn, LOOKING).len(),
        PROGRAMS.len(),
        "every row looks for its program: {drawn:?}"
    );
    assert!(rects_of(&drawn, NOT_FOUND).is_empty(), "{drawn:?}");
    assert!(rects_of(&drawn, FOUND).is_empty(), "{drawn:?}");
}

/// A whole frame reads the setting of Settings, so a person who turns it on in
/// the Settings window reads the group without a restart.
#[test]
fn a_frame_reads_the_setting_of_the_settings_window() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.orchestrator_dock = OrchestratorDock::following(false);
    app.agent_programs = AgentPrograms::answered([true; 4]);
    assert!(
        !app.orchestrator_dock.visible(),
        "the window starts with the setting off"
    );

    app.template_config.features.orchestrator_dock = true;
    let ctx = egui::Context::default();
    let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
    let _ = crate::app::test_support::run_app_frame_with_input(&ctx, &mut app, input).discard_textures();

    assert!(
        app.orchestrator_dock.visible(),
        "the frame reads the setting of the Settings window"
    );
    let drawn = Sidebar::new().settled(&mut app);
    assert_eq!(rects_of(&drawn, GROUP).len(), 1, "the group arrives with it: {drawn:?}");
}

#[test]
fn the_rent_button_sends_one_message_to_the_orchestrator_of_the_active_workspace() {
    let folder = tempfile::tempdir().expect("a folder for the stand-in");
    let program = horizon_core::stand_in_program_in(folder.path(), "stand-in.sh", SILENT)
        .expect("the stand-in program of the test");
    let (_temp, mut app) = app_with_program(program);
    let workspace = app.board.create_workspace("alpha");
    app.board.focus_workspace(workspace);
    let local_id = app
        .board
        .workspace(workspace)
        .expect("the workspace of the press")
        .local_id
        .clone();
    let sidebar = Sidebar::new();
    let drawn = sidebar.settled(&mut app);
    let button = box_of(&drawn, LABEL);

    sidebar.click(&mut app, button.center());

    assert_eq!(
        app.orchestrator_dock.person_messages(&local_id),
        vec![MESSAGE.to_owned()],
        "the conversation of the workspace holds the message of the person once"
    );
    wait_for_bridge(&mut app, &local_id);

    // The turn of that message stays open, so the button is disabled: a second
    // press sends nothing.
    let drawn = sidebar.settled(&mut app);
    let disabled = box_of(&drawn, LABEL);
    sidebar.click(&mut app, disabled.center());
    assert_eq!(
        app.orchestrator_dock.person_messages(&local_id),
        vec![MESSAGE.to_owned()],
        "the second press of a busy orchestrator sends nothing"
    );
}

#[test]
fn the_rent_button_is_disabled_without_an_active_workspace() {
    let folder = tempfile::tempdir().expect("a folder for the stand-in");
    let program = horizon_core::stand_in_program_in(folder.path(), "stand-in.sh", SILENT)
        .expect("the stand-in program of the test");
    let (_temp, mut app) = app_with_program(program);
    assert!(
        app.board.active_workspace.is_none(),
        "the window of this test has no active workspace"
    );
    let sidebar = Sidebar::new();
    let drawn = sidebar.settled(&mut app);
    let button = box_of(&drawn, LABEL);

    sidebar.click(&mut app, button.center());

    assert_eq!(
        app.orchestrator_dock.running_bridges(),
        0,
        "no press opens a conversation without a workspace"
    );
    // The button is drawn, and its hover says what a press would need.
    assert_eq!(NO_WORKSPACE, "Open a workspace first");
}
