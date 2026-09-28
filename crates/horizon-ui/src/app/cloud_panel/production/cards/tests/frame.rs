//! Per-frame card state: output follow and the drawer of a collapsed cloud.
use super::*;

#[test]
fn a_view_no_longer_shown_stops_holding_new_output_aside() {
    let mut runtime = super::super::super::Runtime {
        verbose_unpinned: true,
        ..Default::default()
    };
    // A frame in which no output view reported being scrolled up.
    output::begin_frame(&mut runtime);
    assert!(!runtime.verbose_unpinned);
    runtime.push_log("fresh line".into());
    assert_eq!(runtime.logs.back().map(|line| line.text.as_str()), Some("fresh line"));
    assert!(runtime.pending_logs.is_empty());
    runtime.unpinned_views = 2;
    output::begin_frame(&mut runtime);
    assert!(runtime.verbose_unpinned, "a view still scrolled up keeps holding lines");
}

#[test]
#[cfg(unix)]
fn opening_the_drawer_of_a_collapsed_cloud_expands_it() {
    let (temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        7,
        "Collapsed".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    group.remote = Some(size_launch());
    group.collapsed = true;
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.production.runtimes.entry(7).or_default();
    let ctx = egui::Context::default();
    app.apply_strip_action(7, strip::StripAction::Primary(status::Primary::Stop), &ctx);
    assert!(!app.cloud_prototype.groups.0[0].collapsed);
    let runtime = &app.cloud_prototype.production.runtimes[&7];
    assert_eq!(runtime.drawer, Some(Tab::Manage));
    assert!(runtime.confirmation == Confirmation::Stop);
}

#[test]
fn a_layer_keeps_one_updating_progress_line() {
    let mut runtime = super::super::super::Runtime {
        stage: Some(Stage::Push),
        ..Default::default()
    };
    for line in [
        "docker push registry.example/worker:tag",
        "5f70bf18a086: Preparing",
        "e1a4ac3b0b25: Preparing",
        "5f70bf18a086: Pushing [==>      ]  10MB/80MB",
        "5f70bf18a086: Pushing [=====>   ]  40MB/80MB",
        "e1a4ac3b0b25: Pushed",
        "5f70bf18a086: Pushed",
        "tag: digest: sha256:b41c size: 4096",
    ] {
        runtime.push_log(line.into());
    }
    let texts: Vec<_> = runtime.logs.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "docker push registry.example/worker:tag",
            "5f70bf18a086: Pushed",
            "e1a4ac3b0b25: Pushed",
            "tag: digest: sha256:b41c size: 4096",
        ]
    );
    // Another step's lines for the same layer are its own.
    runtime.stage = Some(Stage::Readiness);
    runtime.push_log("5f70bf18a086: Pull complete".into());
    assert_eq!(runtime.logs.len(), 5);
}

#[test]
fn a_layer_updated_while_scrolled_up_replaces_its_visible_line() {
    let mut runtime = super::super::super::Runtime {
        stage: Some(Stage::Push),
        ..Default::default()
    };
    runtime.push_log("5f70bf18a086: Pushing [==>      ]  10MB/80MB".into());
    runtime.verbose_unpinned = true;
    for _ in 0..3 {
        runtime.push_log("5f70bf18a086: Pushing [=====>   ]  40MB/80MB".into());
    }
    runtime.push_log("5f70bf18a086: Pushed".into());
    runtime.verbose_unpinned = false;
    runtime.accept_followed_logs();
    let texts: Vec<_> = runtime.logs.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(texts, ["5f70bf18a086: Pushed"]);
}

#[test]
fn a_retry_is_diagnosed_from_its_own_output() {
    let mut runtime = super::super::super::Runtime {
        stage: Some(Stage::Push),
        ..Default::default()
    };
    runtime.push_log("error from registry: denied".into());
    // A new attempt that fails without printing its own failure line.
    runtime.progress.reset();
    runtime.push_log("docker push registry.example/worker:tag".into());
    runtime.error = Some("Uploading image failed; inspect deployment output".into());
    let status = status::of(&runtime, status::Occupancy::default(), std::time::SystemTime::now());
    let failure = status.failure.unwrap();
    assert_eq!(failure.cause, None, "the earlier attempt's denial is not this failure");
    assert_eq!(status.numbers, "Uploading image failed; inspect deployment output");
    assert_eq!(runtime.logs.len(), 2, "the earlier output is still shown");
}

#[test]
fn a_retry_of_the_same_image_keeps_the_earlier_attempts_layer_lines() {
    let mut runtime = super::super::super::Runtime {
        stage: Some(Stage::Push),
        ..Default::default()
    };
    runtime.push_log("5f70bf18a086: Pushing [==>      ]  10MB/80MB".into());
    runtime.progress.reset();
    runtime.push_log("5f70bf18a086: Pushed".into());
    let texts: Vec<_> = runtime.logs.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        texts,
        ["5f70bf18a086: Pushing [==>      ]  10MB/80MB", "5f70bf18a086: Pushed"]
    );
}
