//! A freshly deployed cloud opens its first panel itself, so the person lands in a
//! working session instead of an empty card. It opens when the person is in the cloud's
//! workspace, so it never takes the view or the focus from other work.
use super::{Deployment, HorizonApp};

/// Only a new deployment with nothing attached: a resumed or reconnected cloud, and one
/// with sessions of its own, are left as the person had them.
fn state_opens_first_panel(state: &Deployment) -> bool {
    state.sessions.is_empty() && state.timeline.as_ref().is_some_and(|timeline| !timeline.reconnected)
}

/// Whether the first panel of a cloud in `workspace` opens now: only while the person is
/// in that workspace, since the panel takes the view and the focus.
fn opens_first_panel_now(workspace: &str, active: Option<&str>) -> bool {
    active == Some(workspace)
}

impl super::Runtime {
    /// A cloud becomes Ready. Only its first Ready can ask for the first panel: resizes and
    /// rebuilds reach Ready again later, and a session-less cloud is not a new deployment then.
    pub(super) fn note_ready_for_first_panel(&mut self, state: &Deployment) {
        if !std::mem::replace(&mut self.first_panel_considered, true) {
            self.first_panel_due = state_opens_first_panel(state);
        }
    }
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
        let active = self
            .board
            .active_workspace
            .and_then(|id| self.board.workspace(id))
            .map(|workspace| workspace.local_id.clone());
        for issue in due {
            let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == issue) else {
                continue;
            };
            // A person who went to another workspace while the cloud deployed keeps their
            // view and focus: the first panel waits until they come to the cloud's workspace.
            if !opens_first_panel_now(&group.workspace, active.as_deref()) {
                continue;
            }
            // Anything already open is the person's; the first panel is only for an empty cloud.
            let first = group.panels.is_empty().then(|| group.first_panel_kind());
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                runtime.first_panel_due = false;
            }
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
    use horizon_core::cloud_panel::{CloudGroup, CloudLaunch};

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
        // The person is in the cloud's workspace, as after New cloud.
        app.board.active_workspace = Some(workspace);
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
    fn only_the_first_ready_of_a_runtime_asks_for_a_panel() {
        let fresh = serde_json::json!({"reconnected": false, "spans": []});
        let state = deployment(&serde_json::json!([]), &fresh);
        let mut runtime = super::super::Runtime::default();
        runtime.note_ready_for_first_panel(&state);
        assert!(runtime.first_panel_due, "a new deployment asks");
        runtime.first_panel_due = false;
        // A resize or rebuild reaches Ready again; the cloud has no sessions, but it is not new.
        runtime.note_ready_for_first_panel(&state);
        assert!(!runtime.first_panel_due, "a later Ready never asks again");

        let mut resumed = super::super::Runtime::default();
        resumed.note_ready_for_first_panel(&deployment(
            &serde_json::json!([]),
            &serde_json::json!({"reconnected": true, "spans": []}),
        ));
        assert!(!resumed.first_panel_due);
        resumed.note_ready_for_first_panel(&state);
        assert!(
            !resumed.first_panel_due,
            "and a reconnect cannot be turned into a deployment later"
        );
    }

    #[test]
    fn the_first_panel_waits_while_the_person_is_in_another_workspace() {
        let (temp, mut app) = test_app();
        add_ready_cloud(&mut app, temp.path());
        let ctx = egui::Context::default();
        let other = app.board.create_workspace("Elsewhere");
        app.board.active_workspace = Some(other);
        app.start_first_cloud_panels(&ctx);
        assert!(
            app.cloud_prototype.production.runtimes[&101].first_panel_due,
            "it waits for the person"
        );
        assert!(app.cloud_prototype.error.is_none(), "and nothing was asked yet");
        assert!(opens_first_panel_now("cloud", Some("cloud")));
        assert!(!opens_first_panel_now("cloud", Some("local")));
        assert!(!opens_first_panel_now("cloud", None));
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
