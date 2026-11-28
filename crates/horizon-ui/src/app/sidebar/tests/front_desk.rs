//! The row `Front desk` at the top of the sidebar.
//!
//! The row stands there only while the orchestrator dock is visible, it is the
//! selected row while the dock shows the front desk, and a press on it shows that
//! conversation, which is the call of the back control of the header.

use egui::{Event, FullOutput, Modifiers, PointerButton, Pos2, Rect};

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::sidebar::agents::AgentPrograms;
use crate::app::sidebar::front_desk_row::LABEL;
use crate::test_egui::DiscardTextures;

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

fn box_of(drawn: &[Drawn], text: &str) -> Option<Rect> {
    drawn.iter().find(|(painted, _)| painted == text).map(|(_, rect)| *rect)
}

/// An app with the dock of `visible` and one work item.
fn app_with_dock(visible: bool) -> (tempfile::TempDir, HorizonApp) {
    let (temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.orchestrator_dock = visible;
    app.orchestrator_dock = OrchestratorDock::following(visible);
    app.agent_programs = AgentPrograms::answered([true; 4]);
    let _ = app.board.create_workspace("Build");
    (temp, app)
}

#[test]
fn the_row_is_absent_while_the_dock_is_off() {
    let (_temp, mut app) = app_with_dock(false);
    let drawn = Sidebar::new().settled(&mut app);

    assert!(
        box_of(&drawn, LABEL).is_none(),
        "a window whose dock is off draws no row: {drawn:?}"
    );
}

#[test]
fn the_row_stands_at_the_top_and_is_selected_with_the_front_desk() {
    let (_temp, mut app) = app_with_dock(true);
    let sidebar = Sidebar::new();
    let drawn = sidebar.settled(&mut app);
    let row = box_of(&drawn, LABEL).expect("the row of the front desk");
    let work_items = box_of(&drawn, crate::app::sidebar::caption::WORK_ITEMS).expect("the caption");
    assert!(
        row.top() < work_items.top(),
        "the row stands above the groups: {row:?} {work_items:?}"
    );
    assert!(
        app.orchestrator_dock.front_desk().shows(&app.board),
        "the start of the window shows the front desk"
    );

    // A press on a work item row shows that workspace, and the row of the front
    // desk stops being the selected one.
    let workspace_row = box_of(&drawn, "Build").expect("the row of the work item");
    sidebar.click(&mut app, workspace_row.center());
    let _ = sidebar.settled(&mut app);
    assert!(
        !app.orchestrator_dock.front_desk().shows(&app.board),
        "a press on a work item row shows its conversation"
    );
}

#[test]
fn a_press_on_the_row_shows_the_front_desk() {
    let (_temp, mut app) = app_with_dock(true);
    let workspace = app.board.workspaces[0].id;
    app.board.focus_workspace(workspace);
    app.orchestrator_dock.front_desk_mut().show_workspace();
    let sidebar = Sidebar::new();
    let drawn = sidebar.settled(&mut app);
    assert!(
        !app.orchestrator_dock.front_desk().shows(&app.board),
        "the dock shows the workspace"
    );

    let row = box_of(&drawn, LABEL).expect("the row of the front desk");
    sidebar.click(&mut app, row.center());

    assert!(
        app.orchestrator_dock.front_desk().shows(&app.board),
        "the press is the call of the back control of the header"
    );
}

#[test]
fn the_row_says_no_state_word_while_the_front_desk_started_no_program() {
    let (_temp, mut app) = app_with_dock(true);
    let drawn = Sidebar::new().settled(&mut app);

    // The chip of the header says `not connected`, and the row says nothing.
    assert_eq!(crate::app::sidebar::front_desk_row::state_word_for_test(&app), None);
    assert!(!drawn.iter().any(|(text, _)| text == "ready"), "{drawn:?}");
}
