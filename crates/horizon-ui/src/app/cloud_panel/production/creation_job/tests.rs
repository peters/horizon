use super::*;
use crate::app::test_support::test_app;
use horizon_core::cloud_panel::{CHILD_SIZE, CloudConfig};
use horizon_core::{PanelKind, PanelOptions, WorkspaceId, WorkspaceLayout};

fn pending(app: &mut HorizonApp) -> std::sync::mpsc::Sender<cloud_runtime::Result<Resolved>> {
    let workspace = app.board.ensure_workspace();
    pending_in(app, workspace)
}

fn pending_in(
    app: &mut HorizonApp,
    workspace: WorkspaceId,
) -> std::sync::mpsc::Sender<cloud_runtime::Result<Resolved>> {
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n").unwrap();
    let (sender, receiver) = channel();
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.pending_creation = Some(Pending {
        receiver,
        cancel: CancelOnDrop(cloud_runtime::Cancellation::default()),
        session: app.active_session.as_ref().map(|session| session.session_id.clone()),
        workspace: app.board.workspace(workspace).unwrap().local_id.clone(),
        title: "Captured title".into(),
        launch: CloudLaunch {
            publishing_selection: None,
            deployment_started: false,
            id: cloud_runtime::new_id(),
            revision: String::new(),
            profile_name: "dev".into(),
            profile: config.profiles["dev"].clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        },
        siblings: Vec::new(),
        working_copy: None,
    });
    sender
}

fn resolved(path: &std::path::Path) -> Resolved {
    Resolved {
        repository: path.into(),
        revision: "a".repeat(40),
    }
}

#[test]
fn pending_resolution_is_nonblocking_and_cancel_discards_late_results() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let sender = pending(&mut app);
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.production.pending_creation.is_some());
    assert!(app.cloud_prototype.groups.0.is_empty());
    app.cloud_prototype.production.creating = false;
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn completion_uses_captured_workspace_title_profile_and_revision() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let sender = pending(&mut app);
    let destination = app
        .cloud_prototype
        .production
        .pending_creation
        .as_ref()
        .unwrap()
        .workspace
        .clone();
    let _ = app.board.create_workspace("Another workspace");
    app.cloud_prototype.production.title = "Changed title".into();
    app.cloud_prototype.production.selected_profile = "changed".into();
    // The uncommitted files listed when Start was pressed go with the cloud.
    let confirmed = horizon_core::cloud_runtime::repository::uncommitted::Confirmed {
        hash: Some("e".repeat(64)),
    };
    let build = serde_json::from_value(serde_json::json!({"context": ".horizon", "dockerfile": ".horizon/Dockerfile"}))
        .unwrap();
    let listed = (build, confirmed.clone());
    app.cloud_prototype
        .production
        .pending_creation
        .as_mut()
        .unwrap()
        .working_copy = Some(listed);
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    let group = &app.cloud_prototype.groups.0[0];
    assert_eq!(group.workspace, destination);
    assert_eq!(group.title, "Captured title");
    let launch = group.remote.as_ref().unwrap();
    assert_eq!(launch.profile_name, "dev");
    assert_eq!(launch.revision, "a".repeat(40));
    assert!(!launch.deployment_started);
    assert_eq!(group.working_copy, Some(confirmed));
    assert!(app.cloud_prototype.production.runtimes.is_empty());
}

#[test]
fn completed_cloud_starts_with_grid_and_arranges_panels_added_later() {
    let (temp, mut app) = test_app();
    let sender = pending(&mut app);
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&egui::Context::default());
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    let group = &app.cloud_prototype.groups.0[0];
    assert_eq!(group.layout, Some(WorkspaceLayout::Grid));
    let workspace = app.board.workspace_id_by_local_id(&group.workspace).unwrap();
    let ids: Vec<_> = (0..3)
        .map(|_| {
            let options = PanelOptions {
                kind: PanelKind::Usage,
                position: Some(app.cloud_prototype.groups.0[0].next_position(&app.board)),
                size: Some(CHILD_SIZE),
                ..PanelOptions::default()
            };
            app.create_cloud_member(0, options, workspace).unwrap()
        })
        .collect();
    let placed: Vec<_> = ids
        .iter()
        .map(|id| app.board.panel(*id).unwrap().layout.position)
        .collect();
    // Manual placement would line all three up in one row.
    assert_eq!(placed[1][1].to_bits(), placed[0][1].to_bits());
    assert!(placed[1][0] > placed[0][0]);
    assert_eq!(placed[2][0].to_bits(), placed[0][0].to_bits());
    assert!(placed[2][1] > placed[0][1]);
}

#[test]
fn creation_captures_only_an_offered_size_for_a_cpu_profile() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let form = &mut app.cloud_prototype.production;
    form.title = "Sized".into();
    form.profiles = Some(CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n  unoffered:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 3\n    memory_gb: 8\n  accelerated:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 8\n    memory_gb: 32\n    gpu: true\n").unwrap());
    for (profile, size) in [
        ("dev", Some((3, 8))),
        ("dev", Some((4, 64))),
        ("accelerated", Some((0, 32))),
        ("unoffered", None),
    ] {
        app.cloud_prototype.production.selected_profile = profile.into();
        app.cloud_prototype.production.size = size;
        assert!(
            app.create_production_cloud(&ctx).is_err(),
            "{profile} accepted {size:?}"
        );
        assert!(app.cloud_prototype.production.pending_creation.is_none());
    }
    app.cloud_prototype.production.selected_profile = "dev".into();
    app.cloud_prototype.production.size = Some((16, 64));
    app.create_production_cloud(&ctx).unwrap();
    let pending = app.cloud_prototype.production.pending_creation.as_ref().unwrap();
    assert_eq!((pending.launch.profile.cpu, pending.launch.profile.memory_gb), (16, 64));
    assert_eq!(pending.launch.profile_name, "dev");
}

#[test]
fn creation_captures_changed_gpu_host_minimums() {
    let (_temp, mut app) = test_app();
    let form = &mut app.cloud_prototype.production;
    form.title = "GPU minimums".into();
    form.profiles = Some(CloudConfig::parse("version: 1\ndefault: gpu\nprofiles:\n  gpu:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n").unwrap());
    form.selected_profile = "gpu".into();
    form.size = Some((3, 17));
    form.placement.gpu_types = vec!["fixture-gpu".into()];
    app.create_production_cloud(&egui::Context::default()).unwrap();
    let pending = app.cloud_prototype.production.pending_creation.as_ref().unwrap();
    assert_eq!((pending.launch.profile.cpu, pending.launch.profile.memory_gb), (3, 17));
    assert!(pending.launch.profile.gpu);
    assert_eq!(pending.launch.placement.gpu_types, ["fixture-gpu"]);
}

#[test]
fn stale_session_or_detached_destination_cannot_create_a_cloud() {
    for stale_session in [true, false] {
        let (temp, mut app) = test_app();
        let sender = pending(&mut app);
        if stale_session {
            app.cloud_prototype
                .production
                .pending_creation
                .as_mut()
                .unwrap()
                .session = Some("old session".into());
        } else {
            let workspace = app.board.ensure_workspace();
            app.detach_workspace(workspace);
        }
        sender.send(Ok(resolved(temp.path()))).unwrap();
        app.poll_cloud_creation(&egui::Context::default());
        assert!(app.cloud_prototype.groups.0.is_empty());
        assert!(app.cloud_prototype.production.runtimes.is_empty());
        assert!(app.cloud_prototype.production.pending_creation.is_none());
    }
}

#[test]
fn resolution_failure_keeps_form_open_without_creating_compute() {
    let (_temp, mut app) = test_app();
    let sender = pending(&mut app);
    sender
        .send(Err(cloud_runtime::Error::Invalid("Missing revision")))
        .unwrap();
    app.poll_cloud_creation(&egui::Context::default());
    assert!(app.cloud_prototype.production.creating);
    assert!(app.cloud_prototype.error.as_ref().unwrap().contains("Missing revision"));
    assert!(app.cloud_prototype.groups.0.is_empty());
    assert!(app.cloud_prototype.production.runtimes.is_empty());
}

#[test]
fn saving_before_initialization_or_after_session_switch_preserves_board_groups() {
    for (initialized, ready, stale) in [(false, false, false), (true, false, false), (true, true, true)] {
        let (temp, mut app) = test_app();
        let workspace = app.board.ensure_workspace();
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        app.board.cloud_groups.0.push(CloudGroup::new(
            1,
            "Saved cloud".into(),
            local,
            temp.path().into(),
            [0.0, 0.0],
        ));
        app.cloud_prototype.initialized = initialized;
        app.cloud_prototype.ready = ready;
        if stale {
            app.cloud_prototype.production.session_id = Some("previous session".into());
        }
        app.save_cloud_prototype();
        let state = horizon_core::RuntimeState::from_board(&app.board, app.window_config.clone(), app.canvas_view);
        assert_eq!(state.cloud_groups.0.len(), 1);
        assert_eq!(state.cloud_groups.0[0].title, "Saved cloud");
    }
}

#[test]
fn initialized_current_session_can_persist_removing_the_last_cloud() {
    let (temp, mut app) = test_app();
    let workspace = app.board.ensure_workspace();
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    app.board.cloud_groups.0.push(CloudGroup::new(
        1,
        "Removed".into(),
        local,
        temp.path().into(),
        [0.0, 0.0],
    ));
    app.cloud_prototype.initialized = true;
    app.cloud_prototype.ready = true;
    app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|session| session.session_id.clone());
    app.save_cloud_prototype();
    assert!(app.board.cloud_groups.0.is_empty());
}

#[test]
fn a_parked_dialog_still_lands_the_start_it_checked() {
    use crate::test_egui::DiscardTextures;
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let sender = pending(&mut app);
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx
            .run_ui(input(), |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures();
    }
    sender.send(Ok(resolved(temp.path()))).unwrap();
    let mut event = input();
    event.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx
        .run_ui(event, |ui| app.render_cloud_creation(ui.ctx()))
        .discard_textures();
    assert!(
        app.cloud_prototype.production.creating,
        "Escape parks the dialog; it does not cancel the start"
    );
    assert!(app.cloud_prototype.production.pending_creation.is_some());
    // The parked draft keeps checking, so the cloud it started appears.
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    assert!(
        app.cloud_prototype.production.draft.is_none(),
        "the cloud's card replaces it"
    );
}

#[test]
fn cancelled_pending_job_signals_its_worker_and_permit_releases_only_on_exit() {
    let (_temp, mut app) = test_app();
    let _sender = pending(&mut app);
    let cancel = app
        .cloud_prototype
        .production
        .pending_creation
        .as_ref()
        .unwrap()
        .cancel
        .0
        .clone();
    let busy = app.cloud_prototype.production.creation_busy.clone();
    busy.store(true, Ordering::Release);
    let permit = WorkPermit(busy.clone());
    app.cloud_prototype.production.pending_creation = None;
    assert!(cancel.is_cancelled());
    assert!(busy.load(Ordering::Acquire));
    drop(permit);
    assert!(!busy.load(Ordering::Acquire));
}

/// Remove the only cloud from the workspace an open creation targets.
fn remove_cloud_from_creation_target(app: &mut HorizonApp, temp: &std::path::Path) -> WorkspaceId {
    let workspace = app.board.ensure_workspace();
    let local = app.board.workspace(workspace).unwrap().local_id.clone();
    app.cloud_prototype.production.launch.workspace = Some(local.clone());
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n").unwrap();
    let mut group = CloudGroup::new(1, "Removed".into(), local, temp.into(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: false,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(temp.into());
    app.remove_deleted_cloud(1, &egui::Context::default());
    app.settle_cloud_removals();
    assert!(app.cloud_prototype.groups.0.is_empty());
    workspace
}

/// The per-frame release check followed by the next frame's empty-workspace cleanup.
fn next_frames(app: &mut HorizonApp) {
    let ctx = egui::Context::default();
    app.release_workspaces_after_creation(&ctx);
    // A worker thread reads the drafts of the session, and an empty workspace stays until that
    // read says whether it holds a card, so these frames wait for it.
    for _ in 0..200 {
        app.normalize_workspace_state(&ctx);
        if !app.cloud_drafts_hold_workspaces() {
            return;
        }
        // The worker thread that reads the records needs the processor to answer.
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the drafts of the session are not read");
}

fn press_escape_in_creation(app: &mut HorizonApp) {
    use crate::test_egui::DiscardTextures;
    let ctx = egui::Context::default();
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0))),
        ..Default::default()
    };
    for _ in 0..3 {
        let _ = ctx
            .run_ui(input(), |ui| app.render_cloud_creation(ui.ctx()))
            .discard_textures();
    }
    let mut escape = input();
    escape.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx
        .run_ui(escape, |ui| app.render_cloud_creation(ui.ctx()))
        .discard_textures();
    assert!(!app.cloud_prototype.production.creating);
}

#[test]
fn a_creation_keeps_a_removed_clouds_workspace_until_it_ends() {
    for submitted in [true, false] {
        let (temp, mut app) = test_app();
        let _sender = pending(&mut app);
        if !submitted {
            app.cloud_prototype.production.pending_creation = None;
        }
        let workspace = remove_cloud_from_creation_target(&mut app, temp.path());
        for _ in 0..2 {
            next_frames(&mut app);
        }
        assert!(app.board.workspace(workspace).is_some(), "submitted: {submitted}");

        press_escape_in_creation(&mut app);
        next_frames(&mut app);
        assert!(app.board.workspace(workspace).is_none(), "submitted: {submitted}");
    }
}

#[test]
fn failed_or_stale_creation_releases_a_removed_clouds_workspace_when_it_ends() {
    for stale_session in [false, true] {
        let (temp, mut app) = test_app();
        let sender = pending(&mut app);
        let workspace = remove_cloud_from_creation_target(&mut app, temp.path());
        if stale_session {
            app.cloud_prototype
                .production
                .pending_creation
                .as_mut()
                .unwrap()
                .session = Some("old session".into());
        } else {
            sender
                .send(Err(cloud_runtime::Error::Invalid("Missing revision")))
                .unwrap();
        }
        app.poll_cloud_creation(&egui::Context::default());
        assert!(app.cloud_prototype.production.pending_creation.is_none());
        next_frames(&mut app);
        if !stale_session {
            assert!(
                app.board.workspace(workspace).is_some(),
                "the form stays open on its target after a failure"
            );
            press_escape_in_creation(&mut app);
            next_frames(&mut app);
        }
        assert!(app.board.workspace(workspace).is_none(), "stale: {stale_session}");
    }
}

#[test]
fn cloud_settings_that_resume_a_creation_keep_its_target() {
    let (temp, mut app) = test_app();
    let _sender = pending(&mut app);
    let workspace = remove_cloud_from_creation_target(&mut app, temp.path());
    app.open_cloud_accounts(&egui::Context::default(), true);
    assert!(!app.cloud_prototype.production.creating);
    next_frames(&mut app);
    assert!(app.board.workspace(workspace).is_some());

    app.cloud_prototype.production.setup.open = false;
    next_frames(&mut app);
    assert!(app.board.workspace(workspace).is_none());
}

#[test]
fn completed_creation_keeps_the_workspace_for_its_new_cloud() {
    let (temp, mut app) = test_app();
    let sender = pending(&mut app);
    let workspace = remove_cloud_from_creation_target(&mut app, temp.path());
    app.cloud_prototype.production.launch.workspace = None;
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&egui::Context::default());
    assert_eq!(app.cloud_prototype.groups.0.len(), 1);
    for _ in 0..2 {
        next_frames(&mut app);
    }
    assert!(app.board.workspace(workspace).is_some());
    assert!(app.cloud_prototype.creation_holds.is_empty());
}

/// On screen after a cloud is created in a new workspace among the clouds of others.
struct Framed {
    canvas: egui::Rect,
    new_cloud: egui::Rect,
    workspace: egui::Rect,
    others: Vec<(String, egui::Rect)>,
    overlays: [egui::Rect; 2],
    in_new_workspace: bool,
}

/// Clouds in workspaces at `others` (the next free slot for `None`), then a new cloud
/// in a new workspace at `destination`, on a `screen`-sized window.
fn frame_new_cloud(screen: egui::Vec2, others: &[Option<[f32; 2]>], destination: Option<[f32; 2]>) -> Framed {
    use crate::test_egui::DiscardTextures;

    let (temp, mut app) = test_app();
    for (index, at) in others.iter().enumerate() {
        let title = format!("Other {index}");
        let workspace = match at {
            Some(at) => app.board.create_workspace_at(&title, *at),
            None => app.board.create_workspace(&title),
        };
        let local = app.board.workspace(workspace).unwrap().local_id.clone();
        let position = app.cloud_prototype.groups.next_position(&local, &app.board);
        let issue = 201 + u32::try_from(index).unwrap();
        let mut group = CloudGroup::new(issue, title, local, temp.path().into(), position);
        group.reconcile(&mut app.board);
        app.cloud_prototype.groups.0.push(group);
        // The person looks at another workspace while the creation resolves.
        app.board.focus_workspace(workspace);
    }
    app.cloud_prototype.groups.reconcile(&mut app.board);
    let destination = match destination {
        Some(at) => app.board.create_workspace_at("New", at),
        None => app.board.create_workspace("New"),
    };
    let sender = pending_in(&mut app, destination);
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
        ..Default::default()
    });
    let controls = app.canvas_rect(&ctx).min + egui::vec2(24.0, 20.0);
    egui::Area::new(egui::Id::unique("cloud-controls"))
        .fixed_pos(controls)
        .show(&ctx, |ui| {
            ui.allocate_space(egui::vec2(370.0, 58.0));
        });
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    let canvas = app.canvas_rect(&ctx);
    let transform = crate::app::view::canvas_scene_transform(canvas, app.canvas_view);
    let on_screen = |group: &CloudGroup| {
        let (min, max) = group.overview_bounds();
        transform * egui::Rect::from_min_max(min.into(), max.into())
    };
    let new_cloud = on_screen(
        app.cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.title == "Captured title")
            .unwrap(),
    );
    let others = app
        .cloud_prototype
        .groups
        .0
        .iter()
        .filter(|group| group.title != "Captured title")
        .map(|group| (group.title.clone(), on_screen(group)))
        .collect();
    let (position, size) = app.workspace_focus_frame(destination).unwrap();
    let controls = ctx
        .memory(|memory| memory.area_rect(egui::Id::unique("cloud-controls")))
        .unwrap();
    let framed = Framed {
        canvas,
        new_cloud,
        workspace: transform * egui::Rect::from_min_size(position, size),
        others,
        overlays: [controls, app.minimap_overlay_rect(&ctx).unwrap()],
        in_new_workspace: app.board.active_workspace == Some(destination),
    };
    let _ = ctx.end_pass().discard_textures();
    framed
}

fn assert_framed_alone(framed: &Framed, case: &str) {
    assert!(
        framed.canvas.contains_rect(framed.workspace),
        "{case}: the whole workspace is in view"
    );
    assert!(
        framed.canvas.contains_rect(framed.new_cloud),
        "{case}: the new cloud is in view"
    );
    for overlay in framed.overlays {
        assert!(
            !framed.new_cloud.intersects(overlay),
            "{case}: the new cloud clears {overlay:?}"
        );
    }
    assert!(
        framed.in_new_workspace,
        "{case}: the person is in the new cloud's workspace"
    );
}

#[test]
fn a_new_cloud_frames_its_own_workspace_and_leaves_other_clouds_out_of_view() {
    for screen in [
        egui::vec2(1280.0, 720.0),
        egui::vec2(1600.0, 1000.0),
        egui::vec2(3840.0, 1600.0),
    ] {
        // New › Cloud: the new workspace takes the next slot after two others.
        let framed = frame_new_cloud(screen, &[None, None], None);
        let case = format!("rightmost at {screen:?}");
        assert_framed_alone(&framed, &case);
        for (title, bounds) in &framed.others {
            assert!(!framed.canvas.intersects(*bounds), "{case}: {title} stays out of view");
        }
    }
}

#[test]
fn a_new_cloud_between_two_workspaces_shows_neither_when_the_window_allows() {
    // A workspace made at a clicked canvas point, between two others.
    let others = [Some([0.0, 40.0]), Some([3440.0, 40.0])];
    let framed = frame_new_cloud(egui::vec2(1280.0, 720.0), &others, Some([1720.0, 40.0]));
    assert_framed_alone(&framed, "between");
    for (title, bounds) in &framed.others {
        assert!(!framed.canvas.intersects(*bounds), "between: {title} stays out of view");
    }
    // Wider than the space between the neighbours, one of them must show; the new
    // cloud still has its own view.
    let framed = frame_new_cloud(egui::vec2(3840.0, 1600.0), &others, Some([1720.0, 40.0]));
    assert_framed_alone(&framed, "between, wide");
}

#[test]
fn a_new_cloud_leaves_the_view_and_focus_of_a_full_screen_cloud() {
    let (temp, mut app) = test_app();
    let other = app.board.create_workspace("Other");
    let local = app.board.workspace(other).unwrap().local_id.clone();
    let position = app.cloud_prototype.groups.next_position(&local, &app.board);
    let mut group = CloudGroup::new(201, "Other".into(), local, temp.path().into(), position);
    group.reconcile(&mut app.board);
    app.cloud_prototype.groups.0.push(group);
    let usage = |position| PanelOptions {
        kind: PanelKind::Usage,
        position: Some(position),
        size: Some(CHILD_SIZE),
        ..PanelOptions::default()
    };
    let position = app.cloud_prototype.groups.0[0].next_position(&app.board);
    let member = app.create_cloud_member(0, usage(position), other).unwrap();
    let destination = app.board.create_workspace("New");
    // A panel the focus would move to if the new cloud took the workspace.
    let at = app.board.workspace(destination).unwrap().position;
    app.board
        .create_panel(usage([at[0] + 40.0, at[1] + 80.0]), destination)
        .unwrap();
    app.board.focus_workspace(other);
    app.board.focus(member);
    let ctx = egui::Context::default();
    app.toggle_cloud_fullscreen(&ctx, 201);
    let before = (app.canvas_view, app.board.focused, app.board.active_workspace);
    assert_eq!(before.1, Some(member), "the full-screen cloud's panel has the focus");
    let sender = pending_in(&mut app, destination);
    sender.send(Ok(resolved(temp.path()))).unwrap();
    app.poll_cloud_creation(&ctx);
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    assert!(
        app.cloud_prototype
            .groups
            .0
            .iter()
            .any(|group| group.title == "Captured title")
    );
    assert_eq!(app.cloud_prototype.fullscreen.as_ref().map(|view| view.id), Some(201));
    assert_eq!(
        app.board.focused, before.1,
        "the full-screen cloud's panel keeps the focus"
    );
    assert_eq!(app.board.active_workspace, before.2, "the active workspace stays");
    assert_eq!(app.canvas_view, before.0, "the view stays");
}
