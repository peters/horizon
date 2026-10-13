//! Synthetic cloud cards for a debug build. Deploy logs live only in memory, so an
//! isolated viewer has nothing to scroll unless this is asked for, and an idle stop
//! needs a real worker and half an hour, a stuck Docker would stop every other
//! container of the computer, and a running Hetzner server bills. It never runs in a
//! release build, only on an ephemeral session, and it leaves a session alone when
//! that session already has a cloud.
use super::{Deployment, Stage, idle::Report};
use crate::app::HorizonApp;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use std::time::{Duration, Instant};

const PREVIEW_ISSUE: u32 = 8_800_001;
const DOCKER_STUCK_ISSUES: [u32; 2] = [8_800_005, 8_800_006];
const REBUILD_ISSUE: u32 = 8_800_007;
/// The profile of the synthetic cloud whose rebuild reads a repository's latest commit.
const REBUILD_PROFILE: &str = "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    build:\n      context: .\n      dockerfile: Dockerfile\n    capabilities:\n      agents: [claude]\n      browsers: [chromium]\n";

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
    // Minutes the synthetic Hetzner server has run.
    if let Some(minutes) = std::env::var("HORIZON_CLOUD_COST_PREVIEW")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
    {
        seed_cost(app, Duration::from_mins(minutes));
    }
    // A Git repository whose latest commit changes the settings of `REBUILD_PROFILE`.
    if let Some(repository) = std::env::var_os("HORIZON_CLOUD_REBUILD_PREVIEW") {
        seed_rebuild(app, repository.into());
    }
}

/// A ready synthetic `RunPod` cloud of `repository` on `REBUILD_PROFILE`, whose rebuild
/// confirmation lists what the repository's latest commit changes. Its record lives in
/// memory only, so a rebuild that starts finds no record and changes nothing.
fn seed_rebuild(app: &mut HorizonApp, repository: std::path::PathBuf) -> bool {
    if !super::preview::accepts(app) {
        return false;
    }
    let Some(profile) = CloudConfig::parse(REBUILD_PROFILE)
        .ok()
        .and_then(|mut config| config.profiles.remove("dev"))
    else {
        return false;
    };
    let Ok(state) = serde_json::from_value::<Deployment>(serde_json::json!({
        "version": 1, "cloud_id": "synthetic-rebuild", "repository": repository, "revision": "a".repeat(40),
        "profile": profile, "stage": "Ready", "operation": {"state": "bound", "worker_id": "synthetic1"},
        "spec": null, "sessions": [], "source_ready": true,
        "worker": {"id": "synthetic1", "name": "synthetic-rebuild", "imageName": "example.invalid/worker",
            "desiredStatus": "RUNNING"}
    })) else {
        return false;
    };
    let mut group = CloudGroup::new(
        REBUILD_ISSUE,
        "Synthetic rebuild".into(),
        "synthetic".into(),
        repository,
        [40.0, 600.0],
    );
    group.size = [1140.0, 620.0];
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "synthetic-rebuild".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    let runtime = app
        .cloud_prototype
        .production
        .runtimes
        .entry(REBUILD_ISSUE)
        .or_default();
    runtime.progress.stage(Stage::Validate, Instant::now());
    runtime.progress.stage(Stage::Ready, Instant::now());
    runtime.stage = Some(Stage::Ready);
    runtime.state = Some(state);
    // Connected, as after a deployment; the open channel carries no event.
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime.sender = Some(sender);
    runtime.receiver = Some(receiver);
    true
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
    if !super::preview::accepts(app) {
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

/// A ready synthetic Hetzner cloud whose server was created `ran` ago, priced at €0.0240
/// an hour, so its card bills in euros and its run advances. No provider is asked: the
/// record binds no worker, so billing and worker operations have nothing to reach.
fn seed_cost(app: &mut HorizonApp, ran: Duration) -> bool {
    if !super::preview::accepts(app) {
        return false;
    }
    let created = std::time::SystemTime::now().checked_sub(ran).and_then(utc);
    let Some((launch, ready)) = created.and_then(|created| cost_launch(&created)) else {
        return false;
    };
    // A workspace of its own, so the sidebar lists the cloud and its rate.
    let workspace = app.board.create_workspace("Synthetic Hetzner cost");
    let Some(workspace) = app.board.workspace(workspace).map(|item| item.local_id.clone()) else {
        return false;
    };
    let mut group = CloudGroup::new(
        PREVIEW_ISSUE,
        "Synthetic Hetzner cost".into(),
        workspace,
        "/synthetic".into(),
        [40.0, 60.0],
    );
    group.size = [1140.0, 620.0];
    group.environment.provider = Some(launch.profile.provider.clone());
    group.environment.profile = Some(launch.profile_name.clone());
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
    // The card stays connected for as long as Horizon runs.
    let (connection, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    runtime.sender = Some(connection);
    true
}

/// A running cx33 in hel1 as Horizon describes a Hetzner server, created at `created`.
fn cost_launch(created: &str) -> Option<(CloudLaunch, Deployment)> {
    let config = CloudConfig::parse(
        "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: hetzner\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    gpu: false\n",
    )
    .ok()?;
    let profile = config.profiles.get("cpu")?.clone();
    let state = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "synthetic-cost", "repository": "/synthetic", "revision": "a".repeat(40),
        "profile": profile, "stage": "Ready", "operation": {"state": "prepared"},
        "spec": null, "sessions": [], "source_ready": true,
        "worker": {"id": "synthetic1", "name": "synthetic-cost", "imageName": "example.invalid/worker",
            "desiredStatus": "RUNNING", "publicIp": "192.0.2.10", "portMappings": {"22": 22},
            "vcpuCount": 4, "memoryInGb": 8, "dataCenterId": "hel1", "costPerHr": 0.024, "lastStartedAt": created}
    }))
    .ok()?;
    let launch = CloudLaunch {
        deployment_started: true,
        id: "synthetic-cost".into(),
        revision: "a".repeat(40),
        profile_name: "cpu".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    };
    Some((launch, state))
}

/// `at` in RFC 3339 form in UTC, with whole seconds.
fn utc(at: std::time::SystemTime) -> Option<String> {
    let seconds = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let (days, time) = (seconds / 86_400, seconds % 86_400);
    // The civil date of a day count since 1970-01-01, in 400-year eras of 146,097 days.
    let shifted = days + 719_468;
    let (era, day_of_era) = (shifted / 146_097, shifted % 146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    ))
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
    fn a_rebuild_preview_is_a_ready_bound_cloud_of_the_given_repository() {
        let (temp, mut app) = test_app();
        assert!(seed_rebuild(&mut app, temp.path().into()));
        let runtime = &app.cloud_prototype.production.runtimes[&REBUILD_ISSUE];
        let state = runtime.state.as_ref().unwrap();
        assert_eq!(state.repository, temp.path());
        assert!(matches!(
            state.operation,
            horizon_core::cloud_runtime::CreateState::Bound { .. }
        ));
        assert_eq!(runtime.stage, Some(Stage::Ready));
        assert!(!runtime.busy());
        assert!(
            !seed_rebuild(&mut app, temp.path().into()),
            "one synthetic cloud at most"
        );
    }

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
        assert!(!seed_cost(&mut app, Duration::ZERO));
        assert!(app.cloud_prototype.groups.0.is_empty());
        assert!(app.board.cloud_groups.0.is_empty());
    }

    #[test]
    fn the_cost_preview_bills_a_connected_hetzner_worker_in_euros_without_a_provider() {
        let (_temp, mut app) = test_app();
        assert!(seed_cost(&mut app, Duration::from_mins(32)));
        assert!(!seed_cost(&mut app, Duration::ZERO), "one preview only");
        let runtime = &app.cloud_prototype.production.runtimes[&PREVIEW_ISSUE];
        assert!(runtime.connected_ready());
        let state = runtime.state.as_ref().expect("record");
        assert_eq!(state.operation, horizon_core::cloud_runtime::CreateState::Prepared);
        let run = runtime.current_run_cost(std::time::SystemTime::now()).expect("a run");
        assert!(run.elapsed >= Duration::from_mins(32) && run.elapsed < Duration::from_mins(33));
        assert_eq!(
            runtime.cost_badge(std::time::SystemTime::now()).as_deref(),
            Some("€0.01 run")
        );
        assert_eq!(
            utc(std::time::UNIX_EPOCH + Duration::from_mins(29_863_230)).as_deref(),
            Some("2026-10-12T08:30:00Z")
        );
        assert_eq!(
            utc(std::time::UNIX_EPOCH + Duration::from_hours(264_384)).as_deref(),
            Some("2000-02-29T00:00:00Z"),
            "a leap day"
        );
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
