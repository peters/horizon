use super::rows::sidebar_workspace_name_width;
use super::{
    SidebarPanelEntry, SidebarWorkspaceInsert, sidebar_workspace_drop_should_dock, sidebar_workspace_insert_dock_side,
    sidebar_workspace_shows_panels,
};
use horizon_core::WorkspaceDockSide;

#[test]
#[cfg(feature = "cloud-workspaces")]
fn sidebar_presets_preserve_independent_cloud_layouts() {
    use horizon_core::{WorkspaceLayout, cloud_panel::CloudGroup};
    for layout in [None, Some(WorkspaceLayout::Rows)] {
        let (temp, mut app) = crate::app::test_support::test_app();
        let cloud = app.board.create_workspace("cloud");
        let ordinary = app.board.create_workspace("ordinary");
        let first = app.board.create_panel(editor_panel_options("first"), cloud).unwrap();
        let second = app.board.create_panel(editor_panel_options("second"), cloud).unwrap();
        let local = app.board.workspace(cloud).unwrap().local_id.clone();
        let mut group = CloudGroup::new(1, "Cloud".into(), local, temp.path().into(), [24.0, 128.0]);
        group.attach(&mut app.board, first);
        group.attach(&mut app.board, second);
        group.set_layout(&mut app.board, layout);
        app.cloud_prototype.groups.0.push(group);
        app.board.cloud_groups = app.cloud_prototype.groups.clone();
        let geometry = |app: &crate::app::HorizonApp| {
            [first, second].map(|id| {
                let panel = app.board.panel(id).unwrap();
                (panel.layout.position, panel.layout.size)
            })
        };
        let before = geometry(&app);
        let rows = app.sidebar_workspace_data();
        assert!(rows.iter().find(|r| r.id == cloud).unwrap().capabilities.can_arrange);
        assert!(!rows.iter().find(|r| r.id == cloud).unwrap().capabilities.can_detach);
        assert!(rows.iter().find(|r| r.id == ordinary).unwrap().capabilities.can_arrange);
        for parent in WorkspaceLayout::ALL {
            app.apply_sidebar_actions(
                &egui::Context::default(),
                &super::SidebarActions {
                    arrange_layout: Some((cloud, parent)),
                    clear_layout: Some(cloud),
                    ..Default::default()
                },
            );
            app.cloud_prototype.groups.reconcile(&mut app.board);
            assert_eq!(geometry(&app), before);
            assert_eq!(app.cloud_prototype.groups.0[0].layout, layout);
            assert_eq!(app.board.workspace(cloud).unwrap().layout, Some(parent));
        }
        app.apply_sidebar_actions(
            &egui::Context::default(),
            &super::SidebarActions {
                arrange_layout: Some((ordinary, WorkspaceLayout::Columns)),
                ..Default::default()
            },
        );
        assert_eq!(
            app.board.workspace(ordinary).unwrap().layout,
            Some(WorkspaceLayout::Columns)
        );
    }
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn sidebar_cannot_detach_a_cloud_but_keeps_ordinary_workspace_actions() {
    let (temp, mut app) = crate::app::test_support::test_app();
    let cloud = app.board.create_workspace("cloud");
    let ordinary = app.board.create_workspace("ordinary");
    let local = app.board.workspace(cloud).unwrap().local_id.clone();
    app.cloud_prototype
        .groups
        .0
        .push(horizon_core::cloud_panel::CloudGroup::new(
            1,
            "Cloud".into(),
            local,
            temp.path().into(),
            [0.0; 2],
        ));
    let rows = app.sidebar_workspace_data();
    assert!(
        rows.iter()
            .find(|row| row.id == cloud)
            .unwrap()
            .capabilities
            .can_arrange
    );
    assert!(!rows.iter().find(|row| row.id == cloud).unwrap().capabilities.can_detach);
    assert!(
        rows.iter()
            .find(|row| row.id == ordinary)
            .unwrap()
            .capabilities
            .can_arrange
    );
    assert!(
        rows.iter()
            .find(|row| row.id == ordinary)
            .unwrap()
            .capabilities
            .can_detach
    );
    app.apply_sidebar_actions(
        &egui::Context::default(),
        &super::SidebarActions {
            detach_workspace: Some(cloud),
            ..super::SidebarActions::default()
        },
    );
    assert!(!app.workspace_is_detached(cloud));
}

#[test]
#[cfg(feature = "cloud-workspaces")]
fn sidebar_focus_reveals_empty_and_collapsed_cloud_workspaces() {
    for collapsed in [false, true] {
        let (temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("fixture");
        let mut group = horizon_core::cloud_panel::CloudGroup::new(
            1,
            "Fixture".into(),
            app.board.workspace(workspace).unwrap().local_id.clone(),
            temp.path().into(),
            [1700.0, 900.0],
        );
        group.collapsed = collapsed;
        app.cloud_prototype.groups.0.push(group);
        assert!(app.board.workspace_bounds(workspace).is_none());
        let ctx = egui::Context::default();
        let (position, size) = app.workspace_focus_frame(workspace).unwrap();
        app.pan_to_canvas_pos_aligned(&ctx, position, size, true);
        let expected = app.pan_target.take();
        assert!(expected.is_some());
        app.apply_sidebar_actions(
            &ctx,
            &super::SidebarActions {
                pan_to_workspace: Some(workspace),
                ..Default::default()
            },
        );
        assert_eq!(app.pan_target, expected);
        assert_eq!(app.board.active_workspace, Some(workspace));
    }
}

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

#[test]
fn accordion_name_width_fits_detached_row_at_minimum_sidebar() {
    // 168px sidebar minus 14+3+8 leading chrome leaves 143px for name + badges.
    let width = sidebar_workspace_name_width(143.0, true, true);
    assert!(width < 48.0);
    assert!((width - 29.0).abs() <= f32::EPSILON);
}

#[test]
fn flat_name_width_reserves_only_the_detached_badge() {
    // Flat rows draw no panel count, so the name keeps that room.
    assert!((sidebar_workspace_name_width(143.0, false, false) - 133.0).abs() <= f32::EPSILON);
    assert!((sidebar_workspace_name_width(143.0, true, false) - 57.0).abs() <= f32::EPSILON);
}

#[test]
fn name_width_never_goes_negative() {
    assert!(sidebar_workspace_name_width(20.0, true, true).abs() <= f32::EPSILON);
}

// `is_focused` is decorative in these tests: the reveal helpers read the
// board's own focus state, not the sidebar entry flag.
fn sidebar_entry(id: horizon_core::PanelId, is_focused: bool) -> SidebarPanelEntry {
    SidebarPanelEntry {
        id,
        title: "panel".to_string(),
        kind: horizon_core::PanelKind::Editor,
        is_focused,
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

#[test]
fn a_compact_parked_dot_tells_a_screen_reader_its_status_line() {
    use horizon_core::cloud_list::{Dot, Group, Row};
    let workspace = super::WorkspaceSidebarEntry {
        id: horizon_core::WorkspaceId(7),
        name: "sample".to_owned(),
        color: egui::Color32::WHITE,
        is_active: false,
        detached: false,
        capabilities: crate::app::workspace::WorkspaceLayoutCapabilities {
            can_arrange: true,
            can_detach: false,
        },
        panels: Vec::new(),
        row: Row {
            group: Group::Parked,
            dot: Dot::Parked { working: true },
            line: "tests passed".to_owned(),
            hourly_rate: None,
        },
    };
    let labels = crate::test_egui::accesskit_texts(|ui| {
        ui.horizontal(|ui| super::rows::render_sidebar_workspace_row_contents(ui, &workspace, false));
    });
    assert!(
        labels
            .iter()
            .any(|(label, _)| label == "Parked · an agent is working on the worker\ntests passed"),
        "{labels:?}"
    );
}
