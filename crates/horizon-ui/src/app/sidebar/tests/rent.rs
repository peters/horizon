//! The control `+ Rent a machine` at the end of the machine section.
//!
//! A press on the control sends one message to the orchestrator of the active
//! workspace, through the composer of the dock, and it sends nothing while no
//! workspace is active or while that orchestrator works. It stands under the rows
//! of the machine section with a dashed outline, and it keeps the rules of the
//! button of today.

use egui::{Event, FullOutput, Modifiers, PointerButton, Pos2, Rect};

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::sidebar::agents::AgentPrograms;
use crate::app::sidebar::rent_machine::{FRONT_DESK, LABEL, MESSAGE, NO_WORKSPACE};
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

    fn settled(&self, app: &mut HorizonApp) -> Vec<Drawn> {
        let mut drawn = Vec::new();
        for _ in 0..3 {
            drawn = self.draw(app, Vec::new());
        }
        drawn
    }

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

fn box_of(drawn: &[Drawn], text: &str) -> Rect {
    let rects: Vec<&Rect> = drawn
        .iter()
        .filter(|(painted, _)| painted == text)
        .map(|(_, rect)| rect)
        .collect();
    assert_eq!(rects.len(), 1, "the frame draws {text} once: {drawn:?}");
    *rects[0]
}

/// Reads the worker of the dock until the program of `local_id` runs, which the
/// start of a press does on that worker.
fn wait_for_bridge(app: &mut HorizonApp, local_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !app.orchestrator_dock.running(local_id) {
        app.poll_orchestrator_dock(&egui::Context::default());
        assert!(
            std::time::Instant::now() < deadline,
            "the press started the program of that conversation"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// An app whose search of the programs answered and whose orchestrator runs
/// `program` instead of the agent program of a person.
fn app_with_program(program: std::path::PathBuf) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.orchestrator_dock = true;
    app.orchestrator_dock = OrchestratorDock::with_program(true, program);
    app.agent_programs = AgentPrograms::answered([true; 4]);
    (temp, app)
}

#[test]
fn the_rent_button_sends_one_message_to_the_orchestrator_of_the_active_workspace() {
    let folder = tempfile::tempdir().expect("a folder for the stand-in");
    let program = horizon_core::stand_in_program_in(folder.path(), "stand-in.sh", SILENT)
        .expect("the stand-in program of the test");
    let (_temp, mut app) = app_with_program(program);
    let workspace = app.board.create_workspace("alpha");
    app.board.focus_workspace(workspace);
    // The dock shows the conversation of that workspace, as a press on its row
    // does: a machine belongs to one work item.
    app.orchestrator_dock.front_desk_mut().show_workspace();
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
    // The button is drawn, and its hover says what a press would need: the front
    // desk is the conversation of the dock, and it owns no machine.
    assert_eq!(FRONT_DESK, "Open a work item first");
    assert_eq!(NO_WORKSPACE, "Open a workspace first");
}
