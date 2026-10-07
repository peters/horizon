use super::*;
use crate::app::cloud_panel::production::{Deployment, Runtime};
use crate::app::test_support::test_app;
use horizon_core::{
    Board, PanelState, RuntimeState, WorkspaceState,
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
};

const MEMBERS: [&str; 2] = ["one", "two"];

/// A ready cloud with two shell members that wait to attach, as after a restart.
fn ready_cloud() -> (tempfile::TempDir, HorizonApp) {
    let (temp, app) = test_app();
    ready_cloud_in(temp, app, [0.0, 0.0])
}

fn ready_cloud_in(temp: tempfile::TempDir, mut app: HorizonApp, origin: [f32; 2]) -> (tempfile::TempDir, HorizonApp) {
    let profile = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap()
    .profiles["dev"]
        .clone();
    let mut group = CloudGroup::new(1, "Cloud".into(), "workspace".into(), temp.path().into(), origin);
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: profile.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    group.panels = MEMBERS.iter().map(|local| (*local).to_owned()).collect();
    let mut saved = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".into(),
            name: "Fixture".into(),
            panels: MEMBERS
                .iter()
                .map(|local| PanelState {
                    local_id: (*local).into(),
                    kind: PanelKind::Shell,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }],
        ..Default::default()
    };
    saved.cloud_groups.0.push(group);
    app.board = Board::from_runtime_state(&saved).unwrap();
    app.cloud_prototype.groups = saved.cloud_groups.clone();
    app.cloud_prototype.root = Some(temp.path().into());
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"fixture","repository":temp.path(),"revision":"a".repeat(40),"profile":profile,
        "stage":"Ready","operation":{"state":"bound","worker_id":"fixture"},"spec":null,
        "worker":{"id":"fixture","name":"fixture","imageName":"example/worker","desiredStatus":"RUNNING","publicIp":"127.0.0.1","portMappings":{"22":9}},
        "sessions":[],"source_ready":true
    }))
    .unwrap();
    let store = cloud_runtime::state::Store::lock(&temp.path().join("fixture")).unwrap();
    store.save(&state).unwrap();
    drop(store);
    std::fs::write(
        temp.path().join("settings.json"),
        serde_json::to_vec(&serde_json::json!({
            "runpod_key_file":temp.path().join("unused-key"),"ssh_identity_file":temp.path().join("absent-identity"),
            "docker_config":temp.path().join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut runtime = Runtime {
        state: Some(state),
        stage: Some(Stage::Ready),
        needs_attach: true,
        ..Default::default()
    };
    runtime.parking.policy = ParkPolicy {
        attach_dwell: Duration::ZERO,
        park_grace: Duration::ZERO,
    };
    app.cloud_prototype.production.runtimes.insert(1, runtime);
    (temp, app)
}

fn member(app: &HorizonApp, local: &str) -> PanelId {
    app.board.panel_id_by_local_id(local).unwrap()
}

fn wait_of(app: &HorizonApp, local: &str) -> Option<CloudWait> {
    app.board.panel(member(app, local)).unwrap().cloud_wait()
}

/// Whether the member runs the SSH client that attaches its tmux session.
fn attached(app: &HorizonApp, local: &str) -> bool {
    let panel = app.board.panel(member(app, local)).unwrap();
    panel.cloud_wait().is_none() && panel.launch_command.as_deref() == Some("ssh")
}

fn show(app: &mut HorizonApp, local: &str) {
    let id = member(app, local);
    app.panel_screen_rects
        .insert(id, egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10.0, 10.0)));
}

fn runtime(app: &HorizonApp) -> &Runtime {
    &app.cloud_prototype.production.runtimes[&1]
}

#[test]
fn a_cloud_out_of_view_at_ready_parks_without_opening_a_connection() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    for local in MEMBERS {
        assert_eq!(wait_of(&app, local), Some(CloudWait::Parked), "{local}");
    }
    assert!(runtime(&app).pending_member_attachments.is_empty());
    assert!(runtime(&app).parking.tracker.is_some_and(|tracker| tracker.is_parked()));
    // A later reconnecting message does not hide why the member waits.
    app.sync_cloud_member_waits();
    assert_eq!(wait_of(&app, "one"), Some(CloudWait::Parked));
}

#[test]
fn a_cloud_in_view_at_ready_attaches_as_before() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    show(&mut app, "two");
    app.sync_cloud_presentations();
    for local in MEMBERS {
        assert!(attached(&app, local), "{local}");
    }
    assert!(
        runtime(&app)
            .parking
            .tracker
            .is_some_and(|tracker| !tracker.is_parked())
    );
}

#[test]
fn a_cloud_parks_out_of_view_and_attaches_again_in_view() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    show(&mut app, "one");
    app.sync_cloud_presentations();
    assert!(attached(&app, "one"));

    app.panel_screen_rects.clear();
    app.sync_cloud_parking();
    app.sync_cloud_parking();
    for local in MEMBERS {
        assert_eq!(wait_of(&app, local), Some(CloudWait::Parked), "{local}");
    }
    // The status read of the parked sessions fails here: the fixture worker refuses SSH.
    // An older status must not stay on screen as if it were current.
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .parking
        .statuses
        .insert(
            "one".into(),
            SessionStatus {
                id: "one".into(),
                activity: SessionActivity::Working,
                quiet_for: None,
                lines: vec!["old".into()],
            },
        );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while runtime(&app).parking.error.is_none() {
        assert!(std::time::Instant::now() < deadline, "the status read must finish");
        app.sync_cloud_parking();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.parked_strip_text("one"), "Parked · status unavailable");

    show(&mut app, "one");
    app.sync_cloud_parking();
    app.sync_cloud_parking();
    assert_eq!(runtime(&app).pending_member_attachments.len(), MEMBERS.len());
    app.sync_cloud_presentations();
    for local in MEMBERS {
        assert!(attached(&app, local), "{local}");
    }
    assert!(runtime(&app).pending_member_attachments.is_empty());
}

#[test]
fn focus_attaches_a_parked_cloud_at_once_and_a_stop_overrides_parking() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .parking
        .policy
        .attach_dwell = Duration::from_hours(1);
    app.board.focused = Some(member(&app, "two"));
    app.sync_cloud_parking();
    assert_eq!(runtime(&app).pending_member_attachments.len(), MEMBERS.len());

    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().stage = Some(Stage::Stopped);
    app.sync_cloud_member_waits();
    for local in MEMBERS {
        assert_eq!(wait_of(&app, local), Some(CloudWait::Stopped), "{local}");
    }
}

#[test]
fn a_middle_click_paste_into_a_parked_member_attaches_it_and_waits_for_its_terminal() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert_eq!(wait_of(&app, "two"), Some(CloudWait::Parked));
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .parking
        .policy
        .attach_dwell = Duration::from_hours(1);
    let paste = crate::primary_selection::PrimarySelectionPaste {
        panel_id: member(&app, "two"),
        text: "synthetic paste".into(),
    };
    app.deliver_primary_pastes(vec![paste]);
    assert_eq!(
        app.board.focused,
        Some(member(&app, "two")),
        "the paste brings the member into use"
    );
    assert_eq!(
        app.primary_selection.held.len(),
        1,
        "the placeholder must not take the paste"
    );
    app.deliver_primary_pastes(Vec::new());
    assert_eq!(
        app.primary_selection.held.len(),
        1,
        "it waits while the member is parked"
    );

    app.sync_cloud_parking();
    app.sync_cloud_presentations();
    assert!(attached(&app, "two"));
    app.deliver_primary_pastes(Vec::new());
    assert!(
        app.primary_selection.held.is_empty(),
        "the attached terminal takes the paste"
    );
}

#[test]
fn the_strip_names_the_reported_status() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert_eq!(
        app.parked_strip_text("one"),
        "Parked · the agent continues on the worker"
    );
    let parking = &mut app.cloud_prototype.production.runtimes.get_mut(&1).unwrap().parking;
    parking.statuses.insert(
        "one".into(),
        SessionStatus {
            id: "one".into(),
            activity: SessionActivity::Working,
            quiet_for: None,
            lines: vec!["✻ Working… (3s · esc to interrupt)".into()],
        },
    );
    parking.error = Some("unreachable".into());
    assert_eq!(
        app.parked_strip_text("one"),
        "Parked · Working · ✻ Working… (3s · esc to interrupt)"
    );
    assert_eq!(app.parked_strip_text("two"), "Parked · status unavailable");
}

#[test]
fn a_status_read_asks_for_the_sessions_that_the_saved_record_holds() {
    let session = |panel: &str, tmux: &str| super::super::Session {
        panel_id: panel.into(),
        agent: "shell".into(),
        tmux: tmux.into(),
        branch: String::new(),
        worktree: "/workspace/checkout".into(),
    };
    let sessions = vec![
        session("one", "tmux-1"),
        session("two", "tmux-2"),
        session("other", "tmux-3"),
    ];
    let parked = parked_sessions(sessions, &["one".into(), "two".into(), "absent".into()]);
    assert_eq!(parked.len(), 2);
    // More sessions than one command reads are all kept; the read goes in batches.
    let many: Vec<_> = (0..150).map(|n| session(&format!("p{n}"), &format!("t{n}"))).collect();
    let locals: Vec<String> = (0..150).map(|n| format!("p{n}")).collect();
    assert_eq!(parked_sessions(many, &locals).len(), 150);
    assert_eq!(parked["tmux-1"], "one");
    assert_eq!(parked["tmux-2"], "two");
}

#[test]
fn a_pending_browser_discovery_does_not_keep_terminals_parked() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert_eq!(wait_of(&app, "one"), Some(CloudWait::Parked));
    // Discovery that never answers leaves a browser attachment pending.
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .pending_browser_attachments
        .insert("browser".into());
    app.board.focused = Some(member(&app, "one"));
    app.sync_cloud_parking();
    assert_eq!(runtime(&app).pending_member_attachments.len(), MEMBERS.len());
}

#[test]
fn a_terminal_that_starts_in_a_parked_cloud_out_of_view_parks_too() {
    let (temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    let id = member(&app, "two");
    let workspace = app.board.panel(id).unwrap().workspace_id;
    let live = horizon_core::Panel::spawn(
        id,
        workspace,
        horizon_core::PanelOptions {
            kind: PanelKind::Shell,
            local_id: Some("two".into()),
            command: Some("/bin/sh".into()),
            args: vec!["-c".into(), "sleep 30".into()],
            cwd: Some(temp.path().into()),
            ..Default::default()
        },
    )
    .unwrap();
    *app.board.panel_mut(id).unwrap() = live;
    assert_eq!(wait_of(&app, "two"), None);
    app.sync_cloud_parking();
    assert_eq!(wait_of(&app, "two"), Some(CloudWait::Parked));
}

#[test]
fn a_pending_desktop_attachment_does_not_hold_the_terminals() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    // A Device panel whose desktop tunnel is not ready stays pending.
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .pending_member_attachments
        .insert("desktop".into());
    app.board.focused = Some(member(&app, "one"));
    app.sync_cloud_parking();
    assert!(
        runtime(&app)
            .pending_member_attachments
            .iter()
            .any(|local| local == "one")
    );
}

#[test]
fn a_session_recreated_while_its_cloud_is_hidden_parks_and_keeps_the_focus() {
    let (_temp, mut app) = ready_cloud();
    let elsewhere = app.board.create_workspace("elsewhere");
    app.board.focused = None;
    app.board.active_workspace = Some(elsewhere);
    // A saved session whose panel is not on the board, as after it was closed locally.
    let cloud = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    cloud.state.as_mut().unwrap().sessions.push(super::super::Session {
        panel_id: "three".into(),
        agent: "shell".into(),
        tmux: "three".into(),
        branch: String::new(),
        worktree: "/workspace/checkout".into(),
    });
    app.sync_cloud_presentations();
    let three = app
        .board
        .panel_id_by_local_id("three")
        .expect("the missing session gets a panel");
    assert_eq!(app.board.panel(three).unwrap().cloud_wait(), Some(CloudWait::Parked));
    assert_ne!(
        app.board.panel(three).unwrap().launch_command.as_deref(),
        Some("ssh"),
        "a hidden cloud starts no SSH client for a recreated session"
    );
    for local in MEMBERS {
        assert_eq!(wait_of(&app, local), Some(CloudWait::Parked), "{local}");
    }
    assert_eq!(app.board.focused, None, "a hidden cloud does not take the focus");
    assert_eq!(app.board.active_workspace, Some(elsewhere));
    assert!(runtime(&app).pending_member_attachments.is_empty());
}

#[test]
fn a_later_session_restore_in_a_parked_cloud_starts_parked_without_focus() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert!(runtime(&app).parking.tracker.is_some_and(|tracker| tracker.is_parked()));
    // A restore that failed at Ready is retried later, when the cloud is parked.
    let cloud = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    cloud.state.as_mut().unwrap().sessions.push(super::super::Session {
        panel_id: "late".into(),
        agent: "shell".into(),
        tmux: "late".into(),
        branch: String::new(),
        worktree: "/workspace/checkout".into(),
    });
    cloud.pending_session_attachments.insert("late".into());
    cloud.next_attachment_attempt = None;
    app.sync_cloud_presentations();
    let late = app
        .board
        .panel_id_by_local_id("late")
        .expect("the session gets a panel");
    let panel = app.board.panel(late).unwrap();
    assert_eq!(panel.cloud_wait(), Some(CloudWait::Parked));
    assert_ne!(panel.launch_command.as_deref(), Some("ssh"));
    assert_eq!(app.board.focused, None);
    app.sync_cloud_parking();
    assert!(
        runtime(&app).parking.tracker.is_some_and(|tracker| tracker.is_parked()),
        "the cloud stays parked"
    );
}

#[test]
fn a_cloud_in_view_at_a_later_ready_restores_missing_sessions_live() {
    let (_temp, mut app) = ready_cloud();
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert!(runtime(&app).parking.tracker.is_some_and(|tracker| tracker.is_parked()));
    // The cloud becomes ready again, as after a resume, now in view and with a
    // saved session whose panel is missing.
    show(&mut app, "one");
    let cloud = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    cloud.needs_attach = true;
    cloud.state.as_mut().unwrap().sessions.push(super::super::Session {
        panel_id: "again".into(),
        agent: "shell".into(),
        tmux: "again".into(),
        branch: String::new(),
        worktree: "/workspace/checkout".into(),
    });
    app.sync_cloud_presentations();
    assert!(attached(&app, "again"), "a cloud in view attaches its restored session");
    for local in MEMBERS {
        assert!(attached(&app, local), "{local}");
    }
    assert!(
        runtime(&app)
            .parking
            .tracker
            .is_some_and(|tracker| !tracker.is_parked())
    );
}

#[test]
fn the_strip_is_painted_over_a_parked_panel_in_view() {
    let (temp, ctx, app) = crate::app::test_support::test_app_with_config_and_startup(
        &horizon_core::Config::default(),
        horizon_core::StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    // Far down the canvas, as in practice: the canvas coordinates of the panel body
    // lie outside the screen rectangle, which a screen clip would cut away.
    let (_temp, mut app) = ready_cloud_in(temp, app, [0.0, 3000.0]);
    app.root_viewport_stabilizer = None;
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert_eq!(wait_of(&app, "one"), Some(CloudWait::Parked));
    // Keep it parked while it is drawn, so the strip shows.
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .parking
        .policy
        .attach_dwell = Duration::from_hours(1);
    let id = member(&app, "one");
    let frame = |app: &mut HorizonApp| {
        let output = crate::app::test_support::run_app_frame_with_input(
            &ctx,
            app,
            crate::app::test_support::raw_input([1400.0, 900.0], None),
        );
        output.shapes.iter().find_map(|clipped| {
            let mut text = String::new();
            collect_text(&clipped.shape, &mut text);
            text.starts_with("Parked ·")
                .then(|| (clipped.shape.visual_bounding_rect(), clipped.clip_rect))
        })
    };
    // Let startup settle the view, then move the panel out of view, as before a pan.
    for _ in 0..3 {
        frame(&mut app);
    }
    app.canvas_view = horizon_core::CanvasViewState::new([-50_000.0, 0.0], 0.8);
    for _ in 0..2 {
        assert!(frame(&mut app).is_none(), "no strip while the panel is out of view");
    }
    app.canvas_view = horizon_core::CanvasViewState::new([0.0, -2300.0], 0.8);
    // The first frame that draws the panel also draws its strip.
    let strip = frame(&mut app);
    let body = app.terminal_body_screen_rects[&id];
    let (bounds, clip) = strip.expect("the strip is painted");
    eprintln!("body {body:?} strip {bounds:?} clip {clip:?}");
    assert!(clip.intersects(bounds), "the strip is not clipped away");
    assert!(
        body.expand(1.0).contains_rect(bounds),
        "the strip lies in the panel body"
    );
    assert!(
        body.bottom() - bounds.bottom() < 12.0,
        "the strip sits at the bottom of the body"
    );
}

fn collect_text(shape: &egui::Shape, text: &mut String) {
    match shape {
        egui::Shape::Text(shape) => text.push_str(&shape.galley.job.text),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect_text(shape, text)),
        _ => {}
    }
}
