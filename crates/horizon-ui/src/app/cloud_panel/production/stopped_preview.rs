//! A synthetic stopped cloud for a debug build. Its member panel waits as a restored
//! member does while Horizon reconnects the cloud, and after a delay the cloud is
//! found stopped. It never runs in a release build, only on an ephemeral session
//! without a cloud, and it asks no provider: the record binds no worker.
use super::{Deployment, Event, Stage};
use crate::app::HorizonApp;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use horizon_core::{CloudWait, Panel, PanelKind, PanelOptions};
use std::time::{Duration, Instant};

const PREVIEW_ISSUE: u32 = 8_800_002;

pub(super) fn seed(app: &mut HorizonApp, ctx: &egui::Context) {
    // Seconds until the synthetic cloud is found stopped.
    if let Some(seconds) = std::env::var("HORIZON_CLOUD_STOPPED_PANEL_PREVIEW")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
    {
        seed_stopped(app, Duration::from_secs(seconds), ctx);
    }
}

fn seed_stopped(app: &mut HorizonApp, delay: Duration, ctx: &egui::Context) -> bool {
    // A saved session would persist the synthetic cloud. Any cloud already on the board is left alone.
    if app.active_session.as_ref().is_none_or(|session| session.persistent) || !app.cloud_prototype.groups.0.is_empty()
    {
        return false;
    }
    let Some((launch, ready)) = preview_launch() else {
        return false;
    };
    let workspace = app.board.create_workspace("Synthetic stopped cloud");
    let Some(workspace_local) = app.board.workspace(workspace).map(|item| item.local_id.clone()) else {
        return false;
    };
    let options = || PanelOptions {
        name: Some("Claude".into()),
        kind: PanelKind::Shell,
        command: Some("/bin/true".into()),
        ..PanelOptions::default()
    };
    let Ok(id) = app.board.create_panel(options(), workspace) else {
        return false;
    };
    let Some(panel) = app.board.panel_mut(id) else {
        return false;
    };
    let member = panel.local_id.clone();
    let Ok(placeholder) = Panel::cloud_placeholder(
        id,
        workspace,
        PanelOptions {
            local_id: Some(member.clone()),
            ..options()
        },
        CloudWait::Reconnecting,
    ) else {
        return false;
    };
    panel.request_shutdown();
    *panel = placeholder;
    let mut group = CloudGroup::new(
        PREVIEW_ISSUE,
        "Synthetic stopped cloud".into(),
        workspace_local,
        "/synthetic".into(),
        [40.0, 560.0],
    );
    group.size = [1140.0, 460.0];
    group.remote = Some(launch);
    group.panels = vec![member];
    app.cloud_prototype.groups.0.push(group);
    let mut stopped = ready.clone();
    stopped.stage = Stage::Stopped;
    stopped.stop_requested = true;
    let runtime = app
        .cloud_prototype
        .production
        .runtimes
        .entry(PREVIEW_ISSUE)
        .or_default();
    runtime.progress.stage(Stage::Validate, Instant::now());
    runtime.progress.stage(Stage::Provision, Instant::now());
    runtime.stage = Some(Stage::Provision);
    runtime.state = Some(ready);
    let (events, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let _ = events.send(Event::Stopped(Box::new(stopped)));
        ctx.request_repaint();
    });
    true
}

fn preview_launch() -> Option<(CloudLaunch, Deployment)> {
    let config = CloudConfig::parse(
        "version: 1\ndefault: stopped\nprofiles:\n  stopped:\n    provider: hetzner\n    image: example.invalid/worker\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n    idle_stop_minutes: 30\n",
    )
    .ok()?;
    let profile = config.profiles.get("stopped")?.clone();
    let state = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "synthetic-stopped", "repository": "/synthetic", "revision": "a".repeat(40),
        // Never bound, so nothing reads billing or acts on a worker through a provider.
        "profile": profile, "stage": "Ready", "operation": {"state": "prepared"},
        "spec": null, "sessions": [], "source_ready": true, "worker": null
    }))
    .ok()?;
    let launch = CloudLaunch {
        deployment_started: true,
        id: "synthetic-stopped".into(),
        revision: "a".repeat(40),
        profile_name: "stopped".into(),
        profile,
        placement: horizon_core::cloud_panel::Placement::default(),
    };
    Some((launch, state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;

    #[test]
    fn the_preview_member_waits_for_its_cloud_and_then_names_resume_worker() {
        let (_temp, mut app) = test_app();
        let ctx = egui::Context::default();
        assert!(seed_stopped(&mut app, Duration::ZERO, &ctx));
        assert!(!seed_stopped(&mut app, Duration::ZERO, &ctx), "one preview cloud only");
        let member = app.cloud_prototype.groups.0[0].panels[0].clone();
        let id = app.board.panel_id_by_local_id(&member).expect("member");
        let text = |app: &HorizonApp| app.board.panel(id).unwrap().terminal().unwrap().last_lines_text(30);
        assert!(text(&app).contains("Horizon is reconnecting the cloud of this panel."));
        // The session restore already ran; the next frames only process the cloud.
        app.cloud_prototype.initialized = true;
        app.cloud_prototype.production.session_id = app.active_session.as_ref().map(|s| s.session_id.clone());
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.cloud_prototype.production.runtimes[&PREVIEW_ISSUE].stage != Some(Stage::Stopped) {
            assert!(Instant::now() < deadline, "the synthetic stop arrives");
            app.prepare_production_clouds(&ctx);
        }
        assert!(text(&app).contains("Choose Resume worker on the cloud card to restore this panel."));
    }
}
