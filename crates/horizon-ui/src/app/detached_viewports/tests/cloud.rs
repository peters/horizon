use super::*;
use horizon_core::cloud_panel::{CloudGroup, CloudGroups};

#[test]
fn cloud_workspaces_cannot_detach_through_the_central_action() {
    let (temp, mut app) = test_app();
    let cloud = app.board.create_workspace("cloud");
    let ordinary = app.board.create_workspace("ordinary");
    let local = app.board.workspace(cloud).unwrap().local_id.clone();
    let group = CloudGroup::new(1, "Cloud".into(), local.clone(), temp.path().into(), [0.0; 2]);
    for persisted in [false, true] {
        app.board.cloud_groups = CloudGroups::default();
        app.cloud_prototype.groups = CloudGroups::default();
        if persisted {
            app.board.cloud_groups.0.push(group.clone());
        } else {
            app.cloud_prototype.groups.0.push(group.clone());
        }
        assert!(!app.workspace_can_detach(cloud));
        app.detach_workspace(cloud);
        assert!(!app.workspace_is_detached(cloud));
        assert!(!app.pending_detached_window_position_restore.contains(&local));
    }
    app.detach_workspace(ordinary);
    assert!(app.workspace_is_detached(ordinary));
}

#[test]
fn saved_detached_cloud_restores_in_main_without_losing_members() {
    let cloud_workspace = editor_workspace_state("cloud", [0.0; 2]);
    let panel_local = cloud_workspace.panels[0].local_id.clone();
    let mut group = CloudGroup::new(1, "Cloud".into(), "cloud".into(), ".".into(), [0.0; 2]);
    group.panels.push(panel_local.clone());
    let runtime = RuntimeState {
        cloud_groups: CloudGroups(vec![group]),
        workspaces: vec![cloud_workspace, editor_workspace_state("ordinary", [900.0, 0.0])],
        detached_workspaces: ["cloud", "ordinary"]
            .into_iter()
            .map(|local| DetachedWorkspaceState {
                workspace_local_id: local.into(),
                window: WindowConfig::default(),
            })
            .collect(),
        ..RuntimeState::default()
    };
    let (_temp, _ctx, app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime),
        },
    );
    assert!(!app.detached_workspaces.contains_key("cloud"));
    assert!(!app.pending_detached_window_position_restore.contains("cloud"));
    assert!(app.detached_workspaces.contains_key("ordinary"));
    assert!(app.board.cloud_groups.contains_workspace("cloud"));
    let member = app.board.panel_id_by_local_id(&panel_local).unwrap();
    let cloud = app.board.workspace_id_by_local_id("cloud").unwrap();
    assert_eq!(app.board.panel_workspace_id(member), Some(cloud));
}
