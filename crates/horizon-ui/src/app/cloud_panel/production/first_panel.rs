//! A freshly deployed cloud opens its first panel itself, so the person lands in a
//! working session instead of an empty card.
use super::{Deployment, HorizonApp};
use horizon_core::cloud_panel::CloudGroup;

/// Only a new deployment with nothing attached: a resumed or reconnected cloud, and one
/// with sessions of its own, are left as the person had them.
pub(super) fn state_opens_first_panel(state: &Deployment) -> bool {
    state.sessions.is_empty() && state.timeline.as_ref().is_some_and(|timeline| !timeline.reconnected)
}

impl HorizonApp {
    pub(super) fn start_first_cloud_panels(&mut self, ctx: &egui::Context) {
        // Ready to take panels, with everything it had already attached.
        let due: Vec<u32> = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| {
                group.remote.is_some()
                    && self
                        .cloud_prototype
                        .production
                        .runtimes
                        .get(&group.issue)
                        .is_some_and(|runtime| runtime.first_panel_due && !runtime.attaching())
                    && self.cloud_prototype.production.accepts_panels(group)
            })
            .map(|group| group.issue)
            .collect();
        for issue in due {
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                runtime.first_panel_due = false;
            }
            // Anything already open is the person's; the first panel is only for an empty cloud.
            let first = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .find(|group| group.issue == issue && group.panels.is_empty())
                .map(CloudGroup::first_panel_kind);
            if let Some(kind) = first {
                self.cloud_add_panel(ctx, issue, kind, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::cloud_panel::CloudLaunch;

    fn deployment(sessions: &serde_json::Value, timeline: &serde_json::Value) -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version": 1, "cloud_id": "fixture", "repository": "/synthetic", "revision": "a".repeat(40),
            "profile": {"provider": "runpod", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8},
            "stage": "Ready", "operation": {"state": "bound", "worker_id": "worker"},
            "sessions": sessions, "timeline": timeline,
        }))
        .unwrap()
    }

    fn session() -> serde_json::Value {
        serde_json::json!([{"panel_id": "p1", "agent": "shell", "tmux": "p1", "branch": "", "worktree": "/workspace/checkout"}])
    }

    #[test]
    fn only_a_new_deployment_with_nothing_attached_opens_its_first_panel() {
        let none = serde_json::json!([]);
        let fresh = serde_json::json!({"reconnected": false, "spans": []});
        let resumed = serde_json::json!({"reconnected": true, "spans": []});
        assert!(state_opens_first_panel(&deployment(&none, &fresh)));
        assert!(!state_opens_first_panel(&deployment(&none, &resumed)));
        assert!(!state_opens_first_panel(&deployment(&session(), &fresh)));
        assert!(!state_opens_first_panel(&deployment(&none, &serde_json::Value::Null)));
    }

    fn add_ready_cloud(app: &mut HorizonApp, root: &std::path::Path) {
        let workspace = app.board.create_workspace("Fixture");
        let state = deployment(
            &serde_json::json!([]),
            &serde_json::json!({"reconnected": false, "spans": []}),
        );
        let mut group = CloudGroup::new(
            101,
            "Fixture".into(),
            app.board.workspace(workspace).unwrap().local_id.clone(),
            root.into(),
            [0.0, 0.0],
        );
        group.remote = Some(CloudLaunch {
            id: "fixture".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: state.profile.clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
            deployment_started: true,
        });
        app.cloud_prototype.groups.0.push(group);
        app.cloud_prototype.root = Some(root.into());
        app.cloud_prototype.production.runtimes.insert(
            101,
            super::super::Runtime {
                stage: Some(super::super::Stage::Ready),
                state: Some(state),
                first_panel_due: true,
                ..Default::default()
            },
        );
    }

    #[test]
    fn a_ready_empty_cloud_starts_its_first_panel_once() {
        let (temp, mut app) = test_app();
        add_ready_cloud(&mut app, temp.path());
        let ctx = egui::Context::default();
        app.start_first_cloud_panels(&ctx);
        assert!(
            !app.cloud_prototype.production.runtimes[&101].first_panel_due,
            "the first panel is asked for once"
        );
        // The fixture has no cloud settings, so the start is refused with its reason.
        let reason = app.cloud_prototype.error.take();
        assert!(reason.is_some(), "the cloud was asked to open a panel");
        app.start_first_cloud_panels(&ctx);
        assert!(app.cloud_prototype.error.is_none(), "and never again");
    }

    #[test]
    fn a_cloud_that_is_not_due_or_already_has_panels_starts_nothing() {
        let (temp, mut app) = test_app();
        add_ready_cloud(&mut app, temp.path());
        let ctx = egui::Context::default();
        app.cloud_prototype
            .production
            .runtimes
            .get_mut(&101)
            .unwrap()
            .first_panel_due = false;
        app.start_first_cloud_panels(&ctx);
        assert!(app.cloud_prototype.error.is_none(), "a cloud that is not due");

        app.cloud_prototype
            .production
            .runtimes
            .get_mut(&101)
            .unwrap()
            .first_panel_due = true;
        app.cloud_prototype.groups.0[0].panels.push("open".into());
        app.start_first_cloud_panels(&ctx);
        assert!(
            app.cloud_prototype.error.is_none(),
            "the person's own panel comes first"
        );
        assert!(!app.cloud_prototype.production.runtimes[&101].first_panel_due);
    }

    #[test]
    fn a_cloud_still_attaching_waits_for_its_turn() {
        let (temp, mut app) = test_app();
        add_ready_cloud(&mut app, temp.path());
        app.cloud_prototype
            .production
            .runtimes
            .get_mut(&101)
            .unwrap()
            .needs_attach = true;
        app.start_first_cloud_panels(&egui::Context::default());
        assert!(app.cloud_prototype.production.runtimes[&101].first_panel_due);
        assert!(app.cloud_prototype.error.is_none());
    }
}
