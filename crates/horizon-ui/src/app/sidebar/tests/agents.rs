//! The chips of the group `Agents` of the sidebar.
//!
//! The group says which of the four agent programs of `horizon_core::agents` this
//! computer has, in one chip for each program. A chip wraps under the caption of
//! its group, so the group takes one or two lines instead of eight rows, and its
//! accessible name says what the dot of the chip means.

use egui::{Event, FullOutput, Rect};

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::sidebar::agents::{AgentPrograms, CHIP_HEIGHT, PROGRAMS};
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
}

/// Every rectangle where `drawn` painted `text`.
fn rects_of<'a>(drawn: &'a [Drawn], text: &str) -> Vec<&'a Rect> {
    drawn
        .iter()
        .filter(|(painted, _)| painted == text)
        .map(|(_, rect)| rect)
        .collect()
}

/// The one chip of the program of `name`.
fn chip_of(drawn: &[Drawn], name: &str) -> Rect {
    let rects = rects_of(drawn, name);
    assert_eq!(rects.len(), 1, "the frame draws {name} once: {drawn:?}");
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

#[test]
fn the_group_of_the_agents_is_absent_while_the_dock_is_off() {
    let (_temp, mut app) = app_with([true; 4], false);
    let drawn = Sidebar::new().settled(&mut app);

    assert!(
        rects_of(&drawn, crate::app::sidebar::caption::AGENTS).is_empty(),
        "no caption: {drawn:?}"
    );
    assert!(rects_of(&drawn, "Claude").is_empty(), "no chip: {drawn:?}");
}

#[test]
fn the_chips_name_the_four_programs_in_the_order_of_the_design() {
    let (_temp, mut app) = app_with([true, false, true, false], true);
    let drawn = Sidebar::new().settled(&mut app);

    assert_eq!(
        rects_of(&drawn, crate::app::sidebar::caption::AGENTS).len(),
        1,
        "one caption: {drawn:?}"
    );
    let names: Vec<&str> = PROGRAMS
        .into_iter()
        .map(horizon_core::PanelKind::display_name)
        .collect();
    let boxes: Vec<Rect> = names.iter().map(|name| chip_of(&drawn, name)).collect();
    // The first line runs from left to right, and the line under it as well.
    assert!(
        boxes[1].top() < boxes[0].bottom() && boxes[0].left() < boxes[1].left(),
        "the first two chips share their line: {boxes:?}"
    );
    assert!(
        boxes[2].top() >= boxes[0].bottom(),
        "the third chip wraps under them: {boxes:?}"
    );
    assert!(
        boxes[3].top() < boxes[2].bottom() && boxes[2].left() < boxes[3].left(),
        "the last two chips share their line: {boxes:?}"
    );
    assert!((CHIP_HEIGHT - 24.0).abs() < f32::EPSILON, "a chip is 24 high");
}

/// The chips wrap, so the four programs take two lines at the width of the
/// sidebar and never leave it.
#[test]
fn the_chips_wrap_into_two_lines_inside_the_sidebar() {
    let (_temp, mut app) = app_with([true; 4], true);
    let drawn = Sidebar::new().settled(&mut app);
    let names: Vec<&str> = PROGRAMS
        .into_iter()
        .map(horizon_core::PanelKind::display_name)
        .collect();
    let boxes: Vec<Rect> = names.iter().map(|name| chip_of(&drawn, name)).collect();
    let first_top = boxes[0].top();
    let lines: Vec<f32> = {
        let mut tops: Vec<f32> = boxes.iter().map(Rect::top).collect();
        tops.sort_by(f32::total_cmp);
        tops.dedup_by(|left, right| (*left - *right).abs() < 0.5);
        tops
    };
    assert_eq!(
        lines.len(),
        2,
        "four chips take two lines in a narrow sidebar: {boxes:?}"
    );
    assert!(boxes.iter().all(|rect| rect.top() >= first_top - 0.5));
    assert!(
        boxes
            .iter()
            .all(|rect| rect.right() <= crate::app::SIDEBAR_WIDTH - 13.0),
        "no chip leaves the sidebar: {boxes:?}"
    );
}

/// A chip says in its accessible name what the dot and the colour of its name
/// mean: this computer has the program, it does not have it, or the search still
/// looks for it.
#[test]
fn a_chip_says_in_its_accessible_name_what_this_computer_has() {
    let (_temp, mut app) = app_with([true; 4], true);
    app.agent_programs = AgentPrograms::default();
    let sidebar = Sidebar::new();
    let _ = sidebar.settled(&mut app);
    app.agent_programs = AgentPrograms::answered([true, false, true, false]);
    let _ = sidebar.settled(&mut app);

    sidebar.ctx.enable_accesskit();
    let mut labels: Vec<String> = Vec::new();
    for _ in 0..2 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = sidebar
            .ctx
            .run_ui(input, |ui| app.render_sidebar(ui.ctx()))
            .discard_textures();
        let nodes = output.platform_output.accesskit_update.expect("an accessibility tree");
        labels = nodes
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().or_else(|| node.value()))
            .map(str::to_owned)
            .collect();
    }
    for name in ["Claude: on this computer", "Codex: not found on this computer"] {
        assert!(
            labels.iter().any(|label| label == name),
            "the accessible name of a chip says what it means: {name} in {labels:?}"
        );
    }
    assert!(
        labels.iter().any(|label| label == "Pi: on this computer")
            && labels.iter().any(|label| label == "Gemini: not found on this computer"),
        "every chip says its own state: {labels:?}"
    );
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
    assert_eq!(
        rects_of(&drawn, crate::app::sidebar::caption::AGENTS).len(),
        1,
        "the group arrives with it: {drawn:?}"
    );
    let _ = Sidebar::new();
}
