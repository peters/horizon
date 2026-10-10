//! Synthetic cloud cards for a debug build. Deploy logs live only in memory, so an
//! isolated viewer has nothing to scroll unless this is asked for, and an idle stop
//! needs a real worker and half an hour, and a stuck Docker would stop every other
//! container of the computer. It never runs in a release build, only on
//! an ephemeral session, and it leaves a session alone when that session already
//! has a cloud.
use super::{Deployment, Stage, idle::Report};
use crate::app::HorizonApp;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use std::time::{Duration, Instant};

const PREVIEW_ISSUE: u32 = 8_800_001;
const DOCKER_STUCK_ISSUES: [u32; 2] = [8_800_005, 8_800_006];

pub(super) fn seed(app: &mut HorizonApp, ctx: &egui::Context) {
    if let Some(count) = std::env::var("HORIZON_CLOUD_LOG_PREVIEW")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
    {
        seed_lines(app, count);
    }
    // Seconds until the synthetic worker stops itself.
    if let Some(seconds) = std::env::var("HORIZON_CLOUD_IDLE_STOP_PREVIEW")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
    {
        seed_idle_stop(app, Duration::from_secs(seconds), ctx);
    }
    if std::env::var_os("HORIZON_CLOUD_DOCKER_STUCK_PREVIEW").is_some() {
        seed_docker_stuck(app);
    }
}

fn seed_lines(app: &mut HorizonApp, count: usize) -> bool {
    let count = count.clamp(1, horizon_core::PANEL_SCROLLBACK_LIMIT);
    if !super::preview::accepts(app) {
        return false;
    }
    let Some(launch) = preview_launch() else {
        return false;
    };
    let mut group = CloudGroup::new(
        PREVIEW_ISSUE,
        "Synthetic".into(),
        "synthetic".into(),
        "/synthetic".into(),
        [24.0, 24.0],
    );
    group.size = [1400.0, 860.0];
    group.remote = Some(launch);
    app.cloud_prototype.groups.0.push(group);
    let runtime = app
        .cloud_prototype
        .production
        .runtimes
        .entry(PREVIEW_ISSUE)
        .or_default();
    let now = std::time::Instant::now();
    runtime.progress.stage(Stage::Validate, now);
    runtime.progress.stage(Stage::Build, now);
    runtime.progress.stage(Stage::Push, now);
    runtime.stage = Some(Stage::Push);
    for index in 1..=count {
        runtime.push_log(format!(
            "synthetic-line-{index:05}  deploy output kept with the shell scrollback"
        ));
    }
    true
}

/// A ready synthetic `RunPod` cloud with a 30-minute idle stop. Its idle watch reads 29
/// idle minutes, and after `delay` the provider confirms that the worker stopped
/// itself, as the watch of a real worker reports it. No provider is asked: the record
/// binds no worker, so billing and worker operations have nothing to reach.
fn seed_idle_stop(app: &mut HorizonApp, delay: Duration, ctx: &egui::Context) -> bool {
    if !super::preview::accepts(app) {
        return false;
    }
    let Some((launch, ready)) = idle_launch() else {
        return false;
    };
    let mut stopped = ready.clone();
    stopped.stage = Stage::Stopped;
    stopped.stop_requested = true;
    if let Some(worker) = &mut stopped.worker {
        worker.desired_status = "EXITED".into();
    }
    let mut group = CloudGroup::new(
        PREVIEW_ISSUE,
        "Synthetic idle stop".into(),
        "synthetic".into(),
        "/synthetic".into(),
        // Below the terminals of the isolated fixture's workspace.
        [40.0, 600.0],
    );
    group.size = [1140.0, 420.0];
    group.remote = Some(launch);
    app.cloud_prototype.groups.0.push(group);
    let runtime = app
        .cloud_prototype
        .production
        .runtimes
        .entry(PREVIEW_ISSUE)
        .or_default();
    runtime.progress.stage(Stage::Validate, Instant::now());
    runtime.progress.stage(Stage::Ready, Instant::now());
    runtime.stage = Some(Stage::Ready);
    runtime.state = Some(ready);
    let (connection, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    let reports = runtime.listen_idle();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        // The card stays connected until the stop.
        let _connection = connection;
        let _ = reports.send(Report::Sampled(horizon_core::cloud_runtime::lifecycle::IdleSample {
            idle: Duration::from_mins(29),
            limit: Duration::from_mins(30),
            read_at: Instant::now(),
        }));
        std::thread::sleep(delay);
        let _ = reports.send(Report::StoppedOutside(Box::new(stopped)));
        ctx.request_repaint();
    });
    true
}

/// Two clouds whose image build failed because Docker stopped answering, as a stuck
/// daemon fails a real one, so both cards offer the same Restart Docker. No record
/// binds a worker, so nothing reaches a provider.
fn seed_docker_stuck(app: &mut HorizonApp) -> bool {
    if !accepts_preview(app) {
        return false;
    }
    let Some(launch) = preview_launch() else {
        return false;
    };
    // Side by side below the terminals of the isolated fixture's workspace.
    for (issue, origin) in DOCKER_STUCK_ISSUES.into_iter().zip([[40.0, 520.0], [700.0, 520.0]]) {
        let mut group = CloudGroup::new(
            issue,
            "Synthetic stuck Docker".into(),
            "synthetic".into(),
            "/synthetic".into(),
            origin,
        );
        group.size = [620.0, 760.0];
        group.remote = Some(launch.clone());
        app.cloud_prototype.groups.0.push(group);
        let runtime = app.cloud_prototype.production.runtimes.entry(issue).or_default();
        let now = Instant::now();
        runtime.progress.stage(Stage::Validate, now);
        runtime.progress.stage(Stage::Build, now);
        runtime.stage = Some(Stage::Build);
        runtime.push_log("starting the synthetic contract container".into());
        runtime.push_log("Docker did not answer docker version within 5 s".into());
        runtime.progress.finish(now);
        let stuck = horizon_core::cloud_runtime::Error::DockerNotResponding("worker image contract creation");
        runtime.error = Some(stuck.to_string());
    }
    true
}

fn idle_launch() -> Option<(CloudLaunch, Deployment)> {
    let config = CloudConfig::parse(
        "version: 1\ndefault: idle\nprofiles:\n  idle:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n    idle_stop_minutes: 30\n",
    )
    .ok()?;
    let profile = config.profiles.get("idle")?.clone();
    let state = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "synthetic-idle", "repository": "/synthetic", "revision": "a".repeat(40),
        // Never bound, so nothing reads billing or acts on this worker through a provider.
        "profile": profile, "stage": "Ready", "operation": {"state": "prepared"},
        "spec": null, "sessions": [], "source_ready": true,
        "worker": {"id": "synthetic1", "name": "synthetic-idle", "imageName": "example.invalid/worker",
            "desiredStatus": "RUNNING"}
    }))
    .ok()?;
    let launch = CloudLaunch {
        deployment_started: true,
        id: "synthetic-idle".into(),
        revision: "a".repeat(40),
        profile_name: "idle".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    };
    Some((launch, state))
}

fn preview_launch() -> Option<CloudLaunch> {
    let config = CloudConfig::parse(
        "version: 1\ndefault: demo\nprofiles:\n  demo:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 8\n    memory_gb: 32\n    gpu: true\n",
    )
    .ok()?;
    Some(CloudLaunch {
        deployment_started: true,
        id: "synthetic-deploy".into(),
        revision: "a".repeat(40),
        profile_name: "demo".into(),
        profile: config.profiles.get("demo")?.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;

    #[test]
    fn a_preview_keeps_every_requested_line() {
        let (_temp, mut app) = test_app();
        assert!(seed_lines(&mut app, 400));
        let runtime = app
            .cloud_prototype
            .production
            .runtimes
            .get(&PREVIEW_ISSUE)
            .expect("preview runtime");
        assert_eq!(runtime.logs.len(), 400);
        assert_eq!(
            runtime.logs.front().map(|line| line.text.as_str()),
            Some("synthetic-line-00001  deploy output kept with the shell scrollback")
        );
        assert_eq!(
            runtime.logs.back().map(|line| line.text.as_str()),
            Some("synthetic-line-00400  deploy output kept with the shell scrollback")
        );
        assert!(!seed_lines(&mut app, 10), "a second pass must not add another cloud");
        assert_eq!(app.cloud_prototype.groups.0.len(), 1);
        assert_eq!(
            app.cloud_prototype
                .production
                .runtimes
                .get(&PREVIEW_ISSUE)
                .map(|runtime| runtime.logs.len()),
            Some(400)
        );
    }

    #[test]
    fn the_stuck_docker_preview_fails_two_clouds_that_offer_a_restart() {
        use horizon_core::cloud_runtime::diagnosis;
        let (_temp, mut app) = test_app();
        assert!(seed_docker_stuck(&mut app));
        assert!(!seed_docker_stuck(&mut app), "one preview only");
        assert_eq!(app.cloud_prototype.groups.0.len(), 2);
        for issue in DOCKER_STUCK_ISSUES {
            let runtime = &app.cloud_prototype.production.runtimes[&issue];
            assert!(
                runtime.receiver.is_none() && runtime.state.is_none(),
                "nothing reaches a provider"
            );
            let lines = runtime.logs.iter().map(|line| line.text.as_str());
            let found = diagnosis::diagnose(lines, runtime.error.as_deref().unwrap_or_default());
            assert!(
                found
                    .and_then(|found| found.meaning)
                    .is_some_and(diagnosis::restarts_docker)
            );
        }
    }

    #[test]
    fn a_restored_cloud_is_left_alone() {
        let (_temp, mut app) = test_app();
        let mut group = CloudGroup::new(7, "Kept".into(), "workspace".into(), "/kept".into(), [0.0, 0.0]);
        let mut launch = preview_launch().expect("profile");
        launch.deployment_started = false;
        launch.id = "kept".into();
        launch.revision = "b".repeat(40);
        group.remote = Some(launch);
        app.cloud_prototype.groups.0.push(group);
        assert!(!seed_lines(&mut app, 400));
        assert!(!app.cloud_prototype.production.runtimes.contains_key(&PREVIEW_ISSUE));
        assert_eq!(app.cloud_prototype.groups.0[0].title, "Kept");
    }

    #[test]
    fn an_empty_local_cloud_is_left_alone() {
        let (_temp, mut app) = test_app();
        app.cloud_prototype.groups.0.push(CloudGroup::new(
            7,
            "Local".into(),
            "workspace".into(),
            "/local".into(),
            [0.0, 0.0],
        ));
        assert!(!seed_lines(&mut app, 400));
        assert!(app.cloud_prototype.production.runtimes.is_empty());
        assert_eq!(app.cloud_prototype.groups.0.len(), 1);
        assert_eq!(app.cloud_prototype.groups.0[0].title, "Local");
    }

    #[test]
    fn a_saved_session_is_left_alone() {
        let (_temp, mut app) = test_app();
        let session = app
            .session_store
            .create_session_from_runtime(horizon_core::RuntimeState::default())
            .expect("saved session");
        app.activate_persistent_session(&session);
        assert!(!seed_lines(&mut app, 400));
        assert!(!seed_idle_stop(&mut app, Duration::ZERO, &egui::Context::default()));
        assert!(!seed_docker_stuck(&mut app));
        assert!(app.cloud_prototype.groups.0.is_empty());
        assert!(app.board.cloud_groups.0.is_empty());
    }

    #[test]
    fn a_reconnect_waits_while_the_idle_watch_asks_the_provider() {
        let (_temp, mut app) = test_app();
        let ctx = egui::Context::default();
        assert!(seed_idle_stop(&mut app, Duration::from_hours(1), &ctx));
        let runtime = app
            .cloud_prototype
            .production
            .runtimes
            .get_mut(&PREVIEW_ISSUE)
            .expect("preview runtime");
        let (confirming, reports) = std::sync::mpsc::channel();
        runtime.idle_reports = Some(reports);
        confirming.send(Report::Confirming).expect("listening");
        runtime.poll_idle();
        assert!(runtime.checking_provider());
        app.start_production_deployment(PREVIEW_ISSUE, &ctx);
        let runtime = &app.cloud_prototype.production.runtimes[&PREVIEW_ISSUE];
        assert!(
            runtime.connected_ready() && !runtime.state_unavailable && runtime.error.is_none(),
            "nothing read the record the check holds"
        );
    }

    #[test]
    fn the_idle_stop_preview_goes_from_ready_to_stopped_after_its_idle_minutes() {
        let (_temp, mut app) = test_app();
        assert!(seed_idle_stop(&mut app, Duration::ZERO, &egui::Context::default()));
        let runtime = app
            .cloud_prototype
            .production
            .runtimes
            .get_mut(&PREVIEW_ISSUE)
            .expect("preview runtime");
        assert!(runtime.connected_ready());
        assert_eq!(
            runtime.state.as_ref().map(|state| &state.operation),
            Some(&horizon_core::cloud_runtime::CreateState::Prepared),
            "no worker is bound, so nothing reads billing or reaches a provider"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while runtime.stage != Some(Stage::Stopped) {
            assert!(Instant::now() < deadline, "the synthetic worker stops");
            std::thread::sleep(Duration::from_millis(10));
            runtime.poll_idle();
        }
        assert_eq!(
            runtime.stop_cause,
            Some(horizon_core::cloud_runtime::lifecycle::StopCause::Idle {
                limit: Duration::from_mins(30)
            })
        );
        assert!(runtime.error.is_none() && runtime.receiver.is_none());
    }
}
