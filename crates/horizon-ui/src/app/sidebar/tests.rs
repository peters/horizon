use super::{
    SidebarPanelEntry, SidebarWorkspaceInsert, sidebar_workspace_drop_should_dock, sidebar_workspace_insert_dock_side,
    sidebar_workspace_shows_panels,
};
use horizon_core::{WorkspaceDockSide, stand_in_program};

#[test]
fn sidebar_drop_docks_attached_workspace_against_attached_target() {
    assert!(sidebar_workspace_drop_should_dock(false));
}

#[test]
fn sidebar_drop_preserves_detached_workspace_reposition_against_attached_target() {
    assert!(sidebar_workspace_drop_should_dock(false));
}

#[test]
fn sidebar_drop_skips_board_docking_when_target_workspace_is_detached() {
    assert!(!sidebar_workspace_drop_should_dock(true));
}

#[test]
fn sidebar_insert_side_maps_to_expected_dock_side() {
    assert_eq!(
        sidebar_workspace_insert_dock_side(SidebarWorkspaceInsert::Before),
        WorkspaceDockSide::Left
    );
    assert_eq!(
        sidebar_workspace_insert_dock_side(SidebarWorkspaceInsert::After),
        WorkspaceDockSide::Right
    );
}

#[test]
fn sidebar_keeps_panels_expanded_when_accordion_is_disabled() {
    assert!(sidebar_workspace_shows_panels(false, false));
    assert!(sidebar_workspace_shows_panels(true, false));
}

#[test]
fn sidebar_hides_panels_for_inactive_workspaces_when_accordion_is_enabled() {
    assert!(!sidebar_workspace_shows_panels(false, true));
}

#[test]
fn sidebar_shows_panels_for_the_active_workspace_when_accordion_is_enabled() {
    assert!(sidebar_workspace_shows_panels(true, true));
}

// `is_focused` is decorative in these tests: the reveal helpers read the
// board's own focus state, not the sidebar entry flag.
fn sidebar_entry(id: horizon_core::PanelId, is_focused: bool) -> SidebarPanelEntry {
    SidebarPanelEntry {
        id,
        title: "panel".to_string(),
        label: "panel".to_string(),
        kind: horizon_core::PanelKind::Editor,
        is_focused,
        working: false,
    }
}

fn editor_panel_options(name: &str) -> horizon_core::PanelOptions {
    horizon_core::PanelOptions {
        name: Some(name.to_string()),
        kind: horizon_core::PanelKind::Editor,
        command: Some("seed".to_string()),
        ..horizon_core::PanelOptions::default()
    }
}

#[test]
fn workspace_reveal_panel_prefers_the_focused_panel_of_the_workspace() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("a");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    app.board.focus(second);

    let panels = [sidebar_entry(first, false), sidebar_entry(second, true)];
    assert_eq!(app.workspace_reveal_panel(workspace, &panels), Some(second));
}

#[test]
fn workspace_reveal_panel_falls_back_to_first_panel() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("a");
    let other = app.board.create_workspace("b");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    let outsider = app
        .board
        .create_panel(editor_panel_options("other"), other)
        .expect("panel");

    // Focus lives in another workspace -> the workspace's first panel.
    app.board.focus(outsider);
    let panels = [sidebar_entry(first, false), sidebar_entry(second, false)];
    assert_eq!(app.workspace_reveal_panel(workspace, &panels), Some(first));

    // No focus at all -> first panel; no panels -> nothing to reveal.
    app.board.focused = None;
    assert_eq!(app.workspace_reveal_panel(workspace, &panels), Some(first));
    assert_eq!(app.workspace_reveal_panel(other, &[]), None);
}

#[test]
fn workspace_row_reveal_gate_covers_accordion_and_panel_count() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("a");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    let solo = app.board.create_workspace("solo");
    let only = app
        .board
        .create_panel(editor_panel_options("only"), solo)
        .expect("panel");
    let empty = app.board.create_workspace("empty");

    // Creation leaves focus on the last created panel (`only`), so put
    // it back in `workspace` for the focused-in-workspace case.
    app.board.focus(second);
    let two = [sidebar_entry(first, false), sidebar_entry(second, true)];
    let one = [sidebar_entry(only, false)];

    // Accordion rows always reveal a panel: focused-in-workspace for
    // multi-panel, the only panel for single-panel, nothing for empty.
    assert_eq!(app.workspace_row_reveal(true, workspace, &two), Some(second));
    assert_eq!(app.workspace_row_reveal(true, solo, &one), Some(only));
    assert_eq!(app.workspace_row_reveal(true, empty, &[]), None);

    // Flat rows only reveal single-panel workspaces; multi-panel rows
    // keep panning to the workspace bounds (None).
    assert_eq!(app.workspace_row_reveal(false, solo, &one), Some(only));
    assert_eq!(app.workspace_row_reveal(false, workspace, &two), None);
}

fn workspace_entry(row: horizon_core::workspace_list::Row, detached: bool) -> super::WorkspaceSidebarEntry {
    super::WorkspaceSidebarEntry {
        id: horizon_core::WorkspaceId(7),
        name: "sample".to_owned(),
        place: horizon_core::WorkspacePlace::Local,
        machine_panels: 0,
        color: egui::Color32::WHITE,
        is_active: false,
        detached,
        capabilities: crate::app::workspace::WorkspaceLayoutCapabilities {
            can_arrange: true,
            can_detach: false,
            can_remove_empty: false,
        },
        panels: Vec::new(),
        row,
    }
}

#[test]
fn a_dragged_workspace_drops_only_on_a_docked_row() {
    use horizon_core::workspace_list::Row;
    let docked = workspace_entry(Row::local(false, None), false);
    assert!(super::list::accepts_drop(&docked));
    assert!(!super::list::accepts_drop(&workspace_entry(
        Row::local(false, None),
        true
    )));
}

#[test]
fn the_new_window_badge_gives_way_to_the_name_at_the_minimum_sidebar() {
    use super::rows::name_right;
    // The minimum sidebar is 168 wide: the name starts at 64 and the first line ends at 150.
    let (right, badge) = name_right(64.0, 150.0, Some(7.0), Some(58.0));
    assert!(!badge, "a narrow row keeps the room for the name");
    assert!((right - 135.0).abs() <= f32::EPSILON);
    // The default sidebar is 210 wide: the badge fits and the name keeps 49.
    let (right, badge) = name_right(64.0, 192.0, Some(7.0), Some(58.0));
    assert!(badge);
    assert!((right - 113.0).abs() <= f32::EPSILON);
    // Without a count or a badge the name runs to the right edge.
    assert_eq!(name_right(64.0, 192.0, None, None), (192.0, false));
}

#[test]
fn the_badge_ring_takes_the_color_of_the_card_in_every_state() {
    use super::rows::{card_fill, row_surface};
    use crate::theme;
    let accent = egui::Color32::from_rgb(137, 180, 250);
    assert_eq!(
        row_surface(card_fill(accent, false, false, false)),
        theme::BG_ELEVATED()
    );
    assert_eq!(
        row_surface(card_fill(accent, true, false, false)),
        theme::PANEL_BG_ALT()
    );
    let hovered = row_surface(card_fill(accent, false, true, false));
    assert_eq!(
        hovered,
        theme::composite_over(theme::BG_ELEVATED(), theme::alpha(theme::PANEL_BG_ALT(), 170))
    );
    assert_ne!(hovered, theme::BG_ELEVATED());
    assert_eq!(
        row_surface(card_fill(accent, true, true, true)),
        theme::blend(theme::PANEL_BG_ALT(), accent, 0.22)
    );
}

#[test]
fn keyboard_focus_on_a_panel_row_shows_its_close_button() {
    use crate::test_egui::DiscardTextures;
    use egui::epaint::Shape;

    let (_temp, mut app) = crate::app::test_support::test_app();
    app.template_config.features.sidebar_accordion = true;
    let workspace = app.board.create_workspace("Quiet zone");
    let first = app
        .board
        .create_panel(editor_panel_options("first"), workspace)
        .expect("panel");
    let second = app
        .board
        .create_panel(editor_panel_options("second"), workspace)
        .expect("panel");
    app.board.active_workspace = Some(workspace);
    app.board.focus(first);
    let ctx = egui::Context::default();
    let frame = |app: &mut crate::app::HorizonApp| {
        let input = crate::app::test_support::raw_input([1400.0, 900.0], None);
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    let crosses_on = |output: &egui::FullOutput, label: &str| {
        let texts: Vec<(String, egui::Pos2)> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) => Some((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()).center(),
                )),
                _ => None,
            })
            .collect();
        let row = texts
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, at)| *at)
            .expect(label);
        texts
            .iter()
            .any(|(text, at)| text == "\u{00D7}" && (at.y - row.y).abs() < 4.0)
    };
    // An area shows from its second frame on.
    frame(&mut app);
    assert!(
        !crosses_on(&frame(&mut app), "second"),
        "a resting row hides its close button"
    );

    ctx.memory_mut(|memory| memory.request_focus(super::sidebar_panel_row_id(second)));
    frame(&mut app);
    assert!(
        crosses_on(&frame(&mut app), "second"),
        "a keyboard-focused row shows it"
    );
}

#[test]
fn the_snapshot_labels_only_the_panel_rows_that_show() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let shown = app.board.create_workspace("shown");
    let hidden = app.board.create_workspace("hidden");
    app.board
        .create_panel(editor_panel_options("first"), shown)
        .expect("panel");
    app.board
        .create_panel(editor_panel_options("second"), hidden)
        .expect("panel");
    app.board.active_workspace = Some(shown);
    let labels = |app: &crate::app::HorizonApp, workspace| {
        app.sidebar_workspace_data()
            .into_iter()
            .find(|entry| entry.id == workspace)
            .expect("workspace")
            .panels
            .into_iter()
            .map(|panel| panel.label)
            .collect::<Vec<_>>()
    };

    app.template_config.features.sidebar_accordion = true;
    assert_eq!(labels(&app, shown), ["first"]);
    assert_eq!(labels(&app, hidden), [""], "a collapsed workspace builds no labels");

    app.template_config.features.sidebar_accordion = false;
    assert_eq!(labels(&app, hidden), ["second"], "flat rows all show");
}

#[test]
fn a_workspace_that_lives_on_a_machine_says_so_on_its_status_line() {
    use crate::test_egui::DiscardTextures;
    use egui::epaint::Shape;

    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("Build");
    let local = app.board.create_workspace("Here");
    app.board
        .set_workspace_place(workspace, horizon_core::WorkspacePlace::Machine("build-1".to_owned()));
    let ctx = egui::Context::default();
    let render = |app: &mut crate::app::HorizonApp| {
        let input = crate::app::test_support::raw_input([1400.0, 900.0], None);
        ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures()
    };
    // An area shows from its second frame on.
    let _ = render(&mut app);
    let output = render(&mut app);
    let texts = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) => Some((
                    text.galley.job.text.clone(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let painted = texts(&output);
    let name = painted
        .iter()
        .find(|(text, _)| text == "Build")
        .expect("the name of the workspace");
    let line = painted
        .iter()
        .find(|(text, _)| text == "on build-1")
        .expect("the status line of the placed workspace names its machine");
    assert!(
        line.1.min.y >= name.1.max.y,
        "the machine stands on the status line under the name, not after the name"
    );
    let entry = |app: &crate::app::HorizonApp, id| {
        app.sidebar_workspace_data()
            .into_iter()
            .find(|entry| entry.id == id)
            .expect("the workspace row")
    };
    assert_eq!(
        entry(&app, workspace).place.machine(),
        Some("build-1"),
        "the row reads the place of the workspace"
    );
    assert_eq!(entry(&app, local).place.machine(), None);
    assert_eq!(entry(&app, workspace).machine_panels, 0, "no panel holds a session yet");
}

#[test]
fn the_row_says_which_panels_hold_a_session_of_the_machine() {
    use horizon_core::{Machine, MachineRegistry, Machines, PanelOptions, WorkspacePlace};

    let (_temp, mut app) = crate::app::test_support::test_app();
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
    // The panel runs the stand-in program in place of ssh, so the test starts no ssh.
    let options = PanelOptions {
        launch_wrapper: vec![stand_in_program().to_owned()],
        ..PanelOptions::default()
    };
    app.create_panel_with_options(options, workspace)
        .expect("a terminal of the machine");
    let entry = app
        .sidebar_workspace_data()
        .into_iter()
        .find(|entry| entry.id == workspace)
        .expect("the workspace row");
    assert_eq!(entry.machine_panels, 1, "the row knows that a close ends one session");
}

#[test]
fn an_empty_workspace_is_removed_from_its_row_and_the_last_one_stays() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let lone = app.board.create_workspace("lone");
    let rows = app.sidebar_workspace_data();
    assert!(
        !rows
            .iter()
            .find(|row| row.id == lone)
            .unwrap()
            .capabilities
            .can_remove_empty,
        "the board keeps one workspace"
    );

    let empty = app.board.create_workspace("empty");
    let with_panel = app.board.create_workspace("with a panel");
    app.board
        .create_panel(editor_panel_options("a panel"), with_panel)
        .unwrap();
    let rows = app.sidebar_workspace_data();
    let capability = |id| {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap()
            .capabilities
            .can_remove_empty
    };
    assert!(capability(empty), "an empty workspace can go");
    assert!(!capability(with_panel), "a workspace with a panel stays");

    app.apply_sidebar_actions(
        &egui::Context::default(),
        &super::SidebarActions {
            remove_empty_workspace: Some(empty),
            ..Default::default()
        },
    );
    assert!(app.board.workspace(empty).is_none(), "the row's action removed it");
    assert!(app.board.workspace(with_panel).is_some());
    assert!(app.board.workspace(lone).is_some());
}
