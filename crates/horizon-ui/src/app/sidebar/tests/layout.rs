//! The order and the geometry of the sidebar of the design, at the width of the
//! sidebar of the window.
//!
//! The groups stand in one order from top to bottom, the footer keeps the bottom,
//! and every mark of the sidebar keeps the inset and the size that the design
//! names. A frame that changes nothing lays out no text again.

use egui::Rect;

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::OrchestratorDock;
use crate::app::sidebar::agents::{AgentPrograms, chip_rects};
use crate::app::sidebar::caption;
use crate::app::sidebar::machines;
use crate::app::sidebar::rent_machine::{Dashes, HEIGHT as RENT_HEIGHT, INSET as RENT_INSET};
use crate::app::sidebar::rows;
use crate::app::sidebar::{Group, SidebarActions, fold, footer, front_desk_row};
use crate::test_egui::DiscardTextures;

/// The width of the sidebar of these tests.
const WIDTH: f32 = 214.0;

/// The drawn texts of one frame of the sidebar, with their rectangles.
fn drawn(app: &mut HorizonApp) -> Vec<(String, Rect)> {
    let ctx = egui::Context::default();
    let mut drawn = Vec::new();
    for _ in 0..3 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        drawn = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.job.text.clone(),
                    Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect();
    }
    drawn
}

fn top_of(drawn: &[(String, Rect)], text: &str) -> f32 {
    drawn
        .iter()
        .find(|(painted, _)| painted == text)
        .map_or_else(|| panic!("the frame draws {text}: {drawn:?}"), |(_, rect)| rect.top())
}

#[test]
fn the_groups_stand_in_the_order_of_the_design() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.orchestrator_dock = true;
    app.orchestrator_dock = OrchestratorDock::following(true);
    app.agent_programs = AgentPrograms::answered([true; 4]);
    let workspace = app.board.create_workspace("Build");
    // A question of a card of the board, so the group `Needs you` stands there too.
    let _ = crate::app::sidebar::inbox::asking_card(&mut app, workspace, "Run the plan?", &[("go", "Go")]);
    app.refresh_sidebar_rows(std::time::Instant::now());

    let drawn = drawn(&mut app);
    let order = [
        front_desk_row::LABEL,
        caption::NEEDS_YOU,
        caption::WORK_ITEMS,
        caption::MACHINES,
        caption::AGENTS,
    ];
    let mut last = f32::NEG_INFINITY;
    for text in order {
        let top = top_of(&drawn, text);
        assert!(top > last, "{text} stands under the group before it");
        last = top;
    }
    assert!(
        top_of(&drawn, footer::NO_SERVER) > last,
        "the footer stands under every group"
    );
}

/// The caption of a group: its name 16 in from the left edge, the count 8 after
/// it, and its icon controls at the right edge.
#[test]
fn a_caption_keeps_its_inset_and_its_controls() {
    let ctx = egui::Context::default();
    let mut name_left = None;
    let mut count_left = None;
    for _ in 0..2 {
        let input = crate::app::test_support::raw_input([WIDTH, 200.0], None);
        let output = ctx
            .run_ui(input, |ui| {
                ui.set_max_width(WIDTH);
                let controls = [
                    caption::Control::new(caption::Glyph::Fit, caption::FIT_HOVER),
                    caption::Control::new(caption::Glyph::Plus, caption::NEW_HOVER),
                ];
                caption::render(ui, caption::WORK_ITEMS, Some("2"), &controls);
            })
            .discard_textures();
        let texts: Vec<&egui::epaint::TextShape> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        for text in texts {
            if text.galley.job.text == caption::WORK_ITEMS {
                name_left = Some(text.pos.x);
            }
            if text.galley.job.text == "2" {
                count_left = Some(text.pos.x);
            }
        }
        if name_left.is_some() && count_left.is_some() {
            break;
        }
    }
    let name_left = name_left.expect("the name of the caption");
    let count_left = count_left.expect("the count of the caption");
    assert!((name_left - 16.0).abs() < 0.5, "the caption insets 16: {name_left}");
    assert!(
        count_left > name_left,
        "the count stands 8 after the name: {count_left} {name_left}"
    );
    let fit = caption::control_rect(WIDTH, 0.0, 0);
    let plus = caption::control_rect(WIDTH, 0.0, 1);
    assert!((fit.max.x - (WIDTH - 12.0)).abs() < f32::EPSILON);
    assert!((fit.width() - 22.0).abs() < f32::EPSILON);
    assert!((fit.min.x - plus.max.x - 4.0).abs() < f32::EPSILON);
    assert!((caption::HEIGHT - 22.0).abs() < f32::EPSILON);
}

/// The row `Front desk` of the design: 8 in at each side, 38 high, its glyph 10 in
/// from the left of its card.
#[test]
fn the_front_desk_row_keeps_its_inset() {
    let parts = front_desk_row::parts(WIDTH, 0.0);
    assert!((parts.card.min.x - 8.0).abs() < f32::EPSILON);
    assert!((parts.card.max.x - (WIDTH - 8.0)).abs() < f32::EPSILON);
    assert!((parts.card.height() - 38.0).abs() < f32::EPSILON);
    assert!((parts.glyph.width() - 16.0).abs() < f32::EPSILON);
    assert!((parts.glyph.min.x - 18.0).abs() < f32::EPSILON);
    assert!((parts.label_left - 44.0).abs() < f32::EPSILON);
}

/// A work item row of the design: 8 in at each side, 46 high, its texts end 8 in
/// from the card and the bar of the active row stands at the left edge of the
/// sidebar.
#[test]
fn a_work_item_row_keeps_the_texts_and_the_bar_of_the_design() {
    let row = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH, rows::ROW_HEIGHT));
    let card = rows::card(row);
    assert!((rows::ROW_HEIGHT - 46.0).abs() < f32::EPSILON);
    assert!((card.min.x - 8.0).abs() < f32::EPSILON);
    assert!((card.max.x - (WIDTH - 8.0)).abs() < f32::EPSILON);
    assert!((rows::text_right(card, fold::Mark::Hidden) - (WIDTH - 16.0)).abs() < f32::EPSILON);
    let bar = rows::active_bar(row);
    assert!(
        (bar.min.x - 0.0).abs() < f32::EPSILON,
        "the bar stands at the left edge"
    );
    assert!((bar.width() - 3.0).abs() < f32::EPSILON);
    assert!((bar.min.y - 10.0).abs() < f32::EPSILON);
    assert!((bar.max.y - (46.0 - 10.0)).abs() < f32::EPSILON);
}

/// A machine row of the design: 8 in at each side, 40 high and a 24 box 10 in from
/// the card.
#[test]
fn a_machine_row_keeps_its_box() {
    let row = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH, machines::ROW_HEIGHT));
    let card = machines::card(row);
    assert!((machines::ROW_HEIGHT - 40.0).abs() < f32::EPSILON);
    assert!((card.min.x - 8.0).abs() < f32::EPSILON);
    let box_rect = machines::box_rect(card);
    assert!((box_rect.width() - 24.0).abs() < f32::EPSILON);
    assert!((box_rect.height() - 24.0).abs() < f32::EPSILON);
    assert!((box_rect.min.x - 18.0).abs() < f32::EPSILON);
}

/// The control that rents a machine: 12 in at each side, 34 high, and a dashed
/// outline of dashes of 4 with a gap of 3.
#[test]
fn the_control_that_rents_a_machine_is_dashed() {
    let row = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH, RENT_HEIGHT));
    let rect = row.shrink2(egui::vec2(RENT_INSET, 0.0));
    assert!((rect.width() - (WIDTH - 2.0 * 12.0)).abs() < f32::EPSILON);
    assert!((RENT_HEIGHT - 34.0).abs() < f32::EPSILON);

    let mut dashes = Dashes::default();
    let shapes = dashes.shapes(rect).to_vec();
    assert!(shapes.len() > 10, "the outline is a line of dashes: {}", shapes.len());
    let mut drawn_length = 0.0;
    for shape in &shapes {
        let egui::Shape::LineSegment { points, .. } = shape else {
            panic!("a dash is a line segment: {shape:?}");
        };
        for point in points {
            assert!(
                rect.expand(1.0).contains(*point),
                "a dash stays on the outline: {point:?} of {rect:?}"
            );
        }
        drawn_length += (points[1] - points[0]).length();
    }
    // The perimeter of the rounded rectangle, of which the dashes cover 4 of 7.
    let perimeter =
        2.0 * (rect.width() - 2.0 * 10.0) + 2.0 * (rect.height() - 2.0 * 10.0) + 2.0 * std::f32::consts::PI * 10.0;
    let expected = perimeter * 4.0 / 7.0;
    assert!(
        (drawn_length - expected).abs() < 0.15 * expected,
        "the dashes cover the outline at 4 of 7: {drawn_length} of {expected}"
    );
}

/// The four chips take two lines at the width of the sidebar.
#[test]
fn the_chips_of_the_agents_take_two_lines() {
    let rects = chip_rects(WIDTH, [40.0; 4]);
    let mut tops: Vec<f32> = rects.iter().map(Rect::top).collect();
    tops.sort_by(f32::total_cmp);
    tops.dedup_by(|left, right| (*left - *right).abs() < 0.5);
    assert_eq!(tops.len(), 2, "four chips take two lines: {rects:?}");
    assert!(rects.iter().all(|rect| rect.max.x <= WIDTH - 14.0 + f32::EPSILON));
}

/// The footer of the design: 16 in at each side, 10 above its text and its line
/// across the sidebar.
#[test]
fn the_footer_keeps_its_inset() {
    let (footer_rect, dot, link_right) = footer::parts(WIDTH, 400.0);
    assert!((footer_rect.min.y - 400.0).abs() < f32::EPSILON);
    assert!((footer_rect.height() - footer::HEIGHT).abs() < f32::EPSILON);
    assert!((dot.x - 19.5).abs() < f32::EPSILON);
    assert!((link_right - (WIDTH - 16.0)).abs() < f32::EPSILON);
}

/// The icon controls of a caption take the press of the text buttons that they
/// replace, on the same action values.
#[test]
fn the_icon_controls_take_the_press_of_the_text_buttons() {
    let mut actions = SidebarActions::default();
    actions.take_caption_press(Group::WorkItems, caption::Press::Plus);
    assert!(actions.create_workspace, "the plus makes a new work item");

    let mut actions = SidebarActions::default();
    actions.take_caption_press(Group::WorkItems, caption::Press::Fit);
    assert!(actions.fit_active_workspace, "the fit fits every work item in view");
    assert!(!actions.create_workspace);

    let mut actions = SidebarActions::default();
    actions.take_caption_press(Group::Machines, caption::Press::Plus);
    assert!(actions.press_set_up, "the plus of the machines presses Set up");
}

/// A frame in which nothing changed draws the texts of the frame before it, with
/// the same sizes: no count of a caption, no word of a state, no hover text of a
/// chip and no name of a row is built again.
///
/// The toolkit of this window holds no galley across two passes, so the galleys of
/// two frames are two pictures of one text. The sidebar keeps the strings and the
/// widths that it draws, which is what this test reads.
#[test]
fn a_frame_with_no_change_draws_the_texts_of_the_frame_before_it() {
    let (_temp, ctx, mut app) =
        crate::app::test_support::test_app_with_startup(horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(horizon_core::RuntimeState::default()),
        });
    app.template_config.features.orchestrator_dock = true;
    app.orchestrator_dock = OrchestratorDock::following(true);
    app.agent_programs = AgentPrograms::answered([true; 4]);
    let _ = app.board.create_workspace("Build");
    let mut before: Vec<(String, egui::Vec2)> = Vec::new();
    for frame in 0..6 {
        let input = crate::app::test_support::raw_input([1200.0, 900.0], None);
        let output = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
        let drawn: Vec<(String, egui::Vec2)> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.galley.size())),
                _ => None,
            })
            .collect();
        if frame == 4 {
            before = drawn;
            continue;
        }
        if frame == 5 {
            assert_eq!(drawn, before, "a frame with no change draws the same texts");
        }
    }
}
