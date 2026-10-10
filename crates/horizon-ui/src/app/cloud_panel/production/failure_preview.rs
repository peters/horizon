//! Two synthetic failed clouds for a debug build, so the layout of a long failure line
//! can be seen without a provider: one with a member panel shows the failure in its
//! Status tab, one without panels shows it in its body. It never runs in a release
//! build, only on an ephemeral session without a cloud, and it asks no provider: no
//! record binds a worker.
use super::Stage;
use crate::app::HorizonApp;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use horizon_core::{CloudWait, Panel, PanelKind, PanelOptions};
use std::time::Instant;

const WITH_PANEL: u32 = 8_800_003;
const WITHOUT_PANELS: u32 = 8_800_004;

const CAUSE: &str = "Error response from daemon: Conflict. The container name \"/horizon-contract-00000000-1111-2222-3333-444444444444\" is already in use by container \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\". You have to remove (or rename) that container to be able to reuse that name.";

pub(super) fn seed(app: &mut HorizonApp) {
    if std::env::var_os("HORIZON_CLOUD_FAILURE_PREVIEW").is_some() {
        seed_failures(app);
    }
}

fn seed_failures(app: &mut HorizonApp) -> bool {
    // A saved session would persist the synthetic clouds. Any cloud already on the board is left alone.
    if app.active_session.as_ref().is_none_or(|session| session.persistent) || !app.cloud_prototype.groups.0.is_empty()
    {
        return false;
    }
    let Some(launch) = preview_launch() else {
        return false;
    };
    let workspace = app.board.create_workspace("Synthetic failed clouds");
    let Some(workspace_local) = app.board.workspace(workspace).map(|item| item.local_id.clone()) else {
        return false;
    };
    let Some(member) = member_panel(app, workspace) else {
        return false;
    };
    for (issue, origin, panels) in [
        (WITH_PANEL, [40.0, 40.0], vec![member]),
        (WITHOUT_PANELS, [40.0, 760.0], Vec::new()),
    ] {
        let mut group = CloudGroup::new(
            issue,
            "Synthetic failed cloud".into(),
            workspace_local.clone(),
            "/synthetic".into(),
            origin,
        );
        group.size = [1140.0, 680.0];
        group.remote = Some(launch.clone());
        group.panels = panels;
        app.cloud_prototype.groups.0.push(group);
        let runtime = app.cloud_prototype.production.runtimes.entry(issue).or_default();
        let now = Instant::now();
        for stage in [Stage::Validate, Stage::Provision, Stage::Readiness] {
            runtime.progress.stage(stage, now);
            runtime.stage = Some(stage);
            runtime.push_log(format!("synthetic {} output", stage.label().to_lowercase()));
        }
        runtime.push_log("starting the synthetic contract container".into());
        runtime.push_log(CAUSE.into());
        runtime.progress.finish(now);
        runtime.error = Some("Readiness check failed; inspect deployment output".into());
        // The cloud with a panel opens on its Status tab, where the failure is read.
        if issue == WITH_PANEL {
            runtime.drawer = Some(super::cards::Tab::Status);
        }
    }
    true
}

/// A member panel that waits for its cloud, so the cloud shows its panels, not its body.
fn member_panel(app: &mut HorizonApp, workspace: horizon_core::WorkspaceId) -> Option<String> {
    // The panel is replaced by its placeholder at once; its command only has to exit.
    let (command, args) = if cfg!(windows) {
        ("cmd.exe", vec!["/C".to_owned(), "exit".to_owned()])
    } else {
        ("/bin/sh", vec!["-c".to_owned(), "exit".to_owned()])
    };
    let options = || PanelOptions {
        name: Some("Agent".into()),
        kind: PanelKind::Shell,
        command: Some(command.into()),
        args: args.clone(),
        ..PanelOptions::default()
    };
    let id = app.board.create_panel(options(), workspace).ok()?;
    let panel = app.board.panel_mut(id)?;
    let member = panel.local_id.clone();
    let placeholder = Panel::cloud_placeholder(
        id,
        workspace,
        PanelOptions {
            local_id: Some(member.clone()),
            ..options()
        },
        CloudWait::Reconnecting,
    )
    .ok()?;
    panel.request_shutdown();
    *panel = placeholder;
    Some(member)
}

fn preview_launch() -> Option<CloudLaunch> {
    let config = CloudConfig::parse(
        "version: 1\ndefault: failed\nprofiles:\n  failed:\n    provider: hetzner\n    image: example.invalid/worker\n    cpu: 2\n    memory_gb: 4\n    gpu: false\n",
    )
    .ok()?;
    Some(CloudLaunch {
        deployment_started: true,
        id: "synthetic-failed".into(),
        revision: "a".repeat(40),
        profile_name: "failed".into(),
        profile: config.profiles.get("failed")?.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;

    #[test]
    fn the_preview_seeds_two_failed_clouds_once() {
        let (_temp, mut app) = test_app();
        assert!(seed_failures(&mut app));
        assert!(!seed_failures(&mut app), "one preview only");
        let groups = &app.cloud_prototype.groups.0;
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].panels.len(), 1);
        assert_eq!(groups[1].panels, Vec::<String>::new());
        for group in groups {
            let runtime = &app.cloud_prototype.production.runtimes[&group.issue];
            assert_eq!(runtime.stage, Some(Stage::Readiness));
            assert!(
                runtime.receiver.is_none() && runtime.state.is_none(),
                "nothing reaches a provider"
            );
            assert_eq!(runtime.logs.back().map(|line| line.text.as_str()), Some(CAUSE));
        }
    }
}
