//! Synthetic deploy output for a debug build. Deploy logs live only in memory, so an
//! isolated viewer has nothing to scroll unless this is asked for. It never runs in a
//! release build, and it leaves a session alone when that session already has a cloud.
use super::Stage;
use crate::app::HorizonApp;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};

const PREVIEW_ISSUE: u32 = 8_800_001;

pub(super) fn seed(app: &mut HorizonApp) {
    let Ok(raw) = std::env::var("HORIZON_CLOUD_LOG_PREVIEW") else {
        return;
    };
    let Ok(count) = raw.parse::<usize>() else {
        return;
    };
    seed_lines(app, count);
}

fn seed_lines(app: &mut HorizonApp, count: usize) -> bool {
    let count = count.clamp(1, horizon_core::PANEL_SCROLLBACK_LIMIT);
    // A local card has no deployment yet. Any cloud already on the board is left alone.
    if !app.cloud_prototype.groups.0.is_empty() {
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
}
