use egui::{Pos2, Rect, Shape, Vec2};
use horizon_core::{
    AgentLink, BlockPanelState, Board, CanvasViewState, Config, PanelId, PanelKind, PanelOptions, RuntimeState,
    SshConnection, StartupDecision, WorkspaceId,
};

use super::{
    AgentLines, CLEAR_ALPHA, FAINT_ALPHA, LINE_WIDTH, OVERVIEW_ALPHA, OVERVIEW_ZOOM, Side, focus_agent, line_alpha,
    line_paths,
};
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_config_and_startup};
use crate::theme::ResolvedTheme;

const DARK: ResolvedTheme = ResolvedTheme::Dark;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
}

/// A canvas that holds the places of the tests below.
fn canvas() -> Rect {
    rect(0.0, 0.0, 1_600.0, 900.0)
}

fn view(zoom: f32) -> CanvasViewState {
    CanvasViewState::new([0.0, 0.0], zoom)
}

fn link(agent: u64, target: u64, accent: usize) -> AgentLink {
    AgentLink {
        agent: PanelId(agent),
        target: PanelId(target),
        accent,
    }
}

/// Whether `point` lies inside `rect` and not on its edge.
fn inside(rect: Rect, point: Pos2) -> bool {
    rect.min.x < point.x && point.x < rect.max.x && rect.min.y < point.y && point.y < rect.max.y
}

/// The paths of one pair of panels.
fn path_of(agent_rect: Rect, target_rect: Rect) -> super::LinePath {
    let places = vec![(PanelId(1), Some(agent_rect)), (PanelId(2), Some(target_rect))];
    let paths = line_paths(&[link(1, 2, 0)], &places, canvas(), None, 1.0, DARK);
    assert_eq!(paths.len(), 1, "one pair draws one path");
    paths[0]
}

/// A line leaves the side that faces the other panel, whichever side that is.
#[test]
fn a_line_leaves_the_side_that_faces_the_other_panel() {
    let agent = rect(0.0, 0.0, 100.0, 80.0);
    let cases = [
        (rect(300.0, 0.0, 100.0, 80.0), Side::Right),
        (rect(-300.0, 0.0, 100.0, 80.0), Side::Left),
        (rect(0.0, 300.0, 100.0, 80.0), Side::Bottom),
        (rect(0.0, -300.0, 100.0, 80.0), Side::Top),
    ];
    for (target, side) in cases {
        assert_eq!(Side::facing(agent, target), side, "the facing side of {target:?}");
        let path = path_of(agent, target);
        let start = side.middle(agent);
        assert!(
            (path.start.x - start.x).abs() <= f32::EPSILON && (path.start.y - start.y).abs() <= f32::EPSILON,
            "one line starts at the middle of that side, at {start:?}, and not at {:?}",
            path.start
        );
        assert_eq!(path.end, side.opposite().middle(target));
    }
}

/// A start lies on the edge of the agent panel and an end on the edge of the
/// panel that it controls: neither lies inside a panel.
#[test]
fn a_line_starts_and_ends_on_the_edges_of_its_panels() {
    let agent = rect(0.0, 0.0, 100.0, 80.0);
    for target in [
        rect(300.0, 0.0, 100.0, 80.0),
        rect(-300.0, 40.0, 100.0, 80.0),
        rect(20.0, 300.0, 100.0, 80.0),
        rect(-40.0, -300.0, 100.0, 80.0),
    ] {
        let path = path_of(agent, target);
        assert!(
            agent.expand(0.01).contains(path.start),
            "the start is on the agent panel"
        );
        assert!(!inside(agent, path.start), "the start is not inside the agent panel");
        assert!(!inside(target, path.start), "the start is not inside the other panel");
        assert!(target.expand(0.01).contains(path.end), "the end is on the other panel");
        assert!(!inside(target, path.end), "the end is not inside the other panel");
        assert!(!inside(agent, path.end), "the end is not inside the agent panel");
    }
}

/// Several lines of one agent leave its side spread out in the order of their
/// ends, so they do not cross each other near the agent.
#[test]
fn the_starts_of_three_lines_keep_the_order_of_their_ends() {
    let agent = rect(0.0, 0.0, 100.0, 400.0);
    // The ends in the order of the board, and where they sit along the side.
    let targets = [
        rect(400.0, 300.0, 100.0, 60.0),
        rect(400.0, 0.0, 100.0, 60.0),
        rect(400.0, 150.0, 100.0, 60.0),
    ];
    let links: Vec<AgentLink> = (0..3).map(|index| link(1, 10 + index, 0)).collect();
    let places: Vec<(PanelId, Option<Rect>)> = std::iter::once((PanelId(1), Some(agent)))
        .chain(
            targets
                .iter()
                .enumerate()
                .map(|(index, rect)| (PanelId(10 + index as u64), Some(*rect))),
        )
        .collect();

    let paths = line_paths(&links, &places, canvas(), None, 1.0, DARK);
    assert_eq!(paths.len(), 3, "three pairs draw three paths");

    // The ends of the lines, in the order that the lines left the agent panel.
    let ends: Vec<f32> = paths.iter().map(|path| path.end.y).collect();
    assert!(
        ends[0] < ends[1] && ends[1] < ends[2],
        "the lines keep the order of their ends"
    );

    // One place of four along the side for each of three lines, in that order.
    for (index, path) in paths.iter().enumerate() {
        let expected = agent.min.y + agent.height() * [0.25_f32, 0.5, 0.75][index];
        assert!(
            (path.start.y - expected).abs() <= 0.01,
            "the start of line {index} sits at {expected}, not at {}",
            path.start.y
        );
        assert!(
            (path.start.x - agent.max.x).abs() <= f32::EPSILON,
            "every start of that side sits on the side"
        );
    }
    assert_eq!(
        ends,
        vec![30.0, 180.0, 330.0],
        "the line to the end that sits highest leaves first"
    );
}

/// A line is 1.5 points wide at every zoom, and its marks are 3 points wide.
#[test]
fn a_line_is_the_same_width_at_every_zoom() {
    let agent = rect(0.0, 0.0, 100.0, 80.0);
    let places = vec![
        (PanelId(1), Some(agent)),
        (PanelId(2), Some(rect(400.0, 0.0, 100.0, 80.0))),
    ];
    let links = [link(1, 2, 0)];
    for zoom in [0.5, 1.0, 2.0] {
        let mut cache = AgentLines::default();
        cache.links.extend_from_slice(&links);
        let shapes = cache.shapes(canvas(), view(zoom), None, DARK, &places).to_vec();
        assert_eq!(shapes.len(), 3, "one curve, one dot and one ring at zoom {zoom}");
        let Shape::Path(curve) = &shapes[0] else {
            panic!("the curve of zoom {zoom} is a path");
        };
        assert!(
            (curve.stroke.width - LINE_WIDTH).abs() <= f32::EPSILON,
            "the curve is {LINE_WIDTH} points wide at zoom {zoom}, not {}",
            curve.stroke.width
        );
        let Shape::Circle(dot) = &shapes[1] else {
            panic!("the dot of zoom {zoom} is a circle");
        };
        assert!(
            (dot.radius - 3.0).abs() <= f32::EPSILON,
            "the dot is 3 points wide at zoom {zoom}"
        );
        let Shape::Circle(ring) = &shapes[2] else {
            panic!("the ring of zoom {zoom} is a circle");
        };
        assert!(
            (ring.radius - 3.0).abs() <= f32::EPSILON,
            "the ring is 3 points wide at zoom {zoom}"
        );
    }
}

/// The lines of the agent in focus are clear, the lines of every other agent are
/// faint, and a zoom at or under the threshold gives all of them the middle
/// strength.
#[test]
fn the_strength_of_a_line_follows_the_focus_and_the_zoom() {
    let first = PanelId(1);
    let second = PanelId(2);
    assert_eq!(line_alpha(first, Some(first), 1.0), CLEAR_ALPHA);
    assert_eq!(line_alpha(second, Some(first), 1.0), FAINT_ALPHA);
    assert_eq!(
        line_alpha(first, None, 1.0),
        FAINT_ALPHA,
        "no agent in focus leaves every line faint"
    );
    assert_eq!(line_alpha(second, Some(first), OVERVIEW_ZOOM), OVERVIEW_ALPHA);
    assert_eq!(
        line_alpha(first, Some(first), OVERVIEW_ZOOM),
        OVERVIEW_ALPHA,
        "the middle strength of a zoomed out canvas wins over the focus"
    );
    assert_eq!(line_alpha(first, Some(first), OVERVIEW_ZOOM / 2.0), OVERVIEW_ALPHA);
    assert_eq!(
        line_alpha(first, Some(first), 0.85),
        CLEAR_ALPHA,
        "the fit floor is above the threshold"
    );
    const { assert!(CLEAR_ALPHA > OVERVIEW_ALPHA && OVERVIEW_ALPHA > FAINT_ALPHA) };
}

/// Two agent panels of one workspace, each with a terminal that it opened.
fn two_agents() -> (Board, PanelId, PanelId, PanelId, PanelId) {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let claude = agent(&mut board, workspace, PanelKind::Claude);
    let codex = agent(&mut board, workspace, PanelKind::Codex);
    let claude_terminal = opened_terminal(&mut board, workspace, "Claude");
    let codex_terminal = opened_terminal(&mut board, workspace, "Codex");
    (board, claude, codex, claude_terminal, codex_terminal)
}

fn agent(board: &mut Board, workspace: WorkspaceId, kind: PanelKind) -> PanelId {
    board
        .create_panel_waiting(
            PanelOptions {
                kind,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("an agent panel")
}

fn opened_terminal(board: &mut Board, workspace: WorkspaceId, mark: &str) -> PanelId {
    board
        .create_panel_waiting(
            PanelOptions {
                kind: PanelKind::Ssh,
                opened_by: Some(mark.to_owned()),
                ssh_connection: Some(SshConnection {
                    host: "192.0.2.10".to_owned(),
                    ..SshConnection::default()
                }),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a terminal that an agent opened")
}

/// The agent in focus follows the pointer while it rests on a panel of an agent,
/// and the keyboard otherwise.
#[test]
fn the_agent_in_focus_follows_the_pointer_and_the_keyboard() {
    let (mut board, claude, codex, claude_terminal, _codex_terminal) = two_agents();
    let links = board.agent_links();

    assert_eq!(
        focus_agent(&board, &links, Some(claude)),
        Some(claude),
        "the pointer rest on the agent panel itself"
    );
    assert_eq!(
        focus_agent(&board, &links, Some(claude_terminal)),
        Some(claude),
        "the pointer rest on a panel that the agent controls"
    );
    assert_eq!(
        focus_agent(&board, &links, Some(codex)),
        Some(codex),
        "the pointer over another agent wins over the keyboard"
    );

    board.focused = Some(claude);
    assert_eq!(
        focus_agent(&board, &links, None),
        Some(claude),
        "the agent holds the keyboard"
    );
    assert_eq!(
        focus_agent(&board, &links, Some(claude_terminal)),
        Some(claude),
        "the pointer on a panel of the same agent keeps that agent"
    );
    assert_eq!(
        focus_agent(&board, &links, Some(codex)),
        Some(codex),
        "the pointer on another agent takes the focus from the keyboard"
    );

    let person = board
        .create_panel_waiting(PanelOptions::default(), board.workspaces[0].id)
        .expect("a panel of the person");
    board.focused = Some(person);
    assert_eq!(
        focus_agent(&board, &links, None),
        None,
        "a panel of no agent leaves every line faint"
    );
    assert_eq!(
        focus_agent(&board, &links, Some(person)),
        None,
        "a pointer over a panel of no agent leaves every line faint"
    );
}

/// A board of 40 panels with 60 pairs: the first frame builds three shapes for
/// each pair, and a second frame with no change builds none.
///
/// The board holds 20 agent panels and 60 cards, because a panel names one owner:
/// 60 pairs need 60 panels that an agent controls.
#[test]
#[expect(clippy::cast_precision_loss, reason = "a row and a column of at most twenty panels")]
fn a_second_frame_with_no_change_builds_no_shape() {
    let mut board = Board::new();
    let workspace = board.create_workspace("Work");
    let agents: Vec<PanelId> = (0..20).map(|_| agent(&mut board, workspace, PanelKind::Pi)).collect();
    for (index, agent_id) in agents.iter().enumerate() {
        let actor = board
            .panel(*agent_id)
            .map(|panel| horizon_core::browser_actor(&panel.local_id))
            .expect("the agent panel");
        for _ in 0..3 {
            let state = BlockPanelState {
                owner_actor: Some(actor.clone()),
                ..BlockPanelState::new(BlockPanelState::empty_document())
            };
            board
                .create_panel_waiting(
                    PanelOptions {
                        kind: PanelKind::Block,
                        block: Some(state),
                        ..PanelOptions::default()
                    },
                    workspace,
                )
                .expect("a card");
        }
        assert_eq!(index + 1, agents.iter().take(index + 1).count());
    }
    let links = board.agent_links();
    assert_eq!(links.len(), 60, "twenty agents with three cards each");

    // The places of a board of 80 panels: the agent column and the cards beside it.
    let mut places: Vec<(PanelId, Option<Rect>)> = Vec::new();
    for (index, link) in links.iter().enumerate() {
        let column = index / 20;
        let row = index % 20;
        let y = row as f32 * 40.0;
        places.push((link.agent, Some(rect(0.0, y, 100.0, 30.0))));
        places.push((link.target, Some(rect(300.0 + column as f32 * 400.0, y, 160.0, 30.0))));
    }

    let mut cache = AgentLines::default();
    cache.links.extend_from_slice(&links);
    let first = cache.shapes(canvas(), view(1.0), None, DARK, &places).len();
    assert_eq!(
        first,
        3 * 60,
        "one curve, one dot and one ring for each of the 60 pairs"
    );
    assert_eq!(cache.builds, 1);
    let builds_after_first = cache.builds;

    let second = cache.shapes(canvas(), view(1.0), None, DARK, &places).len();
    assert_eq!(second, first);
    assert_eq!(
        cache.builds, builds_after_first,
        "a frame where nothing changed builds no shape and keeps its entry"
    );

    let moved: Vec<(PanelId, Option<Rect>)> = places
        .iter()
        .map(|(id, place)| (*id, place.map(|rect| rect.translate(Vec2::new(12.0, 0.0)))))
        .collect();
    cache.shapes(canvas(), view(1.0), None, DARK, &moved);
    assert_eq!(
        cache.builds,
        builds_after_first + 1,
        "a pan of the canvas builds the shapes again"
    );
}

/// A pair that this canvas does not draw builds nothing, and a frame with no pair
/// keeps no cache at all.
#[test]
fn a_pair_outside_the_canvas_and_a_board_without_a_pair_draw_nothing() {
    let places = vec![(PanelId(1), Some(rect(0.0, 0.0, 100.0, 80.0))), (PanelId(2), None)];
    assert!(line_paths(&[link(1, 2, 0)], &places, canvas(), None, 1.0, DARK).is_empty());

    // Both ends and the curve of this pair are off the canvas, to the right of it.
    let off_canvas = vec![
        (PanelId(1), Some(rect(2_000.0, 200.0, 100.0, 80.0))),
        (PanelId(2), Some(rect(2_600.0, 200.0, 100.0, 80.0))),
    ];
    assert!(
        line_paths(&[link(1, 2, 0)], &off_canvas, canvas(), None, 1.0, DARK).is_empty(),
        "a pair that the view does not hold builds no shape"
    );

    // A line between two panels off the canvas is drawn while it crosses the canvas.
    let across_the_canvas = vec![
        (PanelId(1), Some(rect(-300.0, 200.0, 100.0, 80.0))),
        (PanelId(2), Some(rect(2_000.0, 200.0, 100.0, 80.0))),
    ];
    assert_eq!(
        line_paths(&[link(1, 2, 0)], &across_the_canvas, canvas(), None, 1.0, DARK).len(),
        1,
        "the part of a line that crosses the canvas is drawn"
    );

    let mut cache = AgentLines::default();
    cache.links.extend_from_slice(&[link(1, 2, 0)]);
    assert!(cache.shapes(canvas(), view(1.0), None, DARK, &places).is_empty());
    assert_eq!(cache.builds, 1, "the frame built the empty set of shapes");

    cache.forget();
    assert!(cache.built.is_none(), "a board without a pair keeps no cache");

    let overlapping = vec![
        (PanelId(1), Some(rect(0.0, 0.0, 100.0, 80.0))),
        (PanelId(2), Some(rect(50.0, 20.0, 100.0, 80.0))),
    ];
    assert!(
        line_paths(&[link(1, 2, 0)], &overlapping, canvas(), None, 1.0, DARK).is_empty(),
        "two panels that cover a common area leave no room for a curve"
    );
}

/// Two agents of one workspace take different accents, in both themes, so their
/// lines differ.
#[test]
fn two_agents_of_one_workspace_take_different_colours() {
    for theme in [ResolvedTheme::Dark, ResolvedTheme::Light] {
        let first = crate::theme::workspace_accent_for(theme, 0);
        let second = crate::theme::workspace_accent_for(theme, 1);
        assert_ne!(first, second, "the first two workspace accents differ in {theme:?}");
        assert_ne!(first, crate::theme::workspace_accent_for(other_theme(theme), 0));
    }
}

const fn other_theme(theme: ResolvedTheme) -> ResolvedTheme {
    match theme {
        ResolvedTheme::Dark => ResolvedTheme::Light,
        ResolvedTheme::Light => ResolvedTheme::Dark,
    }
}

/// A board of an app that draws two agents and the terminals that they opened.
fn app_with_two_agents() -> (tempfile::TempDir, egui::Context, crate::app::HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("Work");
    let mut panel = |kind: PanelKind, x: f32, opened_by: Option<&str>| {
        let options = PanelOptions {
            kind,
            position: Some([x, 40.0]),
            size: Some([320.0, 240.0]),
            opened_by: opened_by.map(str::to_owned),
            ssh_connection: opened_by.map(|_| SshConnection {
                host: "192.0.2.10".to_owned(),
                ..SshConnection::default()
            }),
            ..PanelOptions::default()
        };
        app.board
            .create_panel_waiting(options, workspace)
            .expect("a panel of the test board");
    };
    panel(PanelKind::Claude, 40.0, None);
    panel(PanelKind::Codex, 40.0, None);
    panel(PanelKind::Ssh, 420.0, Some("Claude"));
    panel(PanelKind::Ssh, 420.0, Some("Codex"));
    // No process of this test starts: the panels of the board wait for a
    // preparation that no worker of the test reads.
    app.board.take_pending_starts();
    (temp, ctx, app)
}

/// The frame draws the lines of the board, and the setting that turns them off
/// leaves no cache and no shape.
#[test]
fn the_frame_draws_the_lines_and_the_setting_turns_them_off() {
    let (_temp, ctx, mut app) = app_with_two_agents();
    assert_eq!(app.board.agent_links().len(), 2, "two terminals that two agents opened");

    for _ in 0..3 {
        run_app_frame_with_input(&ctx, &mut app, canvas_input());
    }
    let builds = app.agent_lines.builds;
    let built = app.agent_lines.built.as_ref().expect("a frame of a board with pairs");
    assert_eq!(built.shapes.len(), 6, "two pairs draw three shapes each");

    run_app_frame_with_input(&ctx, &mut app, canvas_input());
    assert_eq!(
        app.agent_lines.builds, builds,
        "a frame that changes nothing builds no shape"
    );

    app.template_config.canvas.agent_lines = false;
    run_app_frame_with_input(&ctx, &mut app, canvas_input());
    assert!(
        app.agent_lines.built.is_none(),
        "the setting off keeps no cache and draws nothing"
    );
}

/// A window of the test, large enough that the panels of the board do not overlap.
fn canvas_input() -> egui::RawInput {
    raw_input([1_600.0, 900.0], None)
}
