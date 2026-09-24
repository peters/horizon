use super::*;
use crate::test_egui::DiscardTextures as _;
use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
use std::sync::mpsc::channel;

fn groups() -> CloudGroups {
    let config = CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 32\n").unwrap();
    let mut group = CloudGroup::new(1, "Source".into(), "workspace1".into(), "/synthetic".into(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "source".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
    });
    CloudGroups(vec![group])
}

fn pending_job(entry: &mut Entry) -> (Cancellation, std::sync::mpsc::Sender<job::Outcome>) {
    let (sender, receiver) = channel();
    let cancel = Cancellation::default();
    entry.job = Some(Job {
        receiver,
        cancel: cancel.clone(),
    });
    (cancel, sender)
}

#[test]
fn switching_sessions_cancels_inflight_results_and_queued_consent() {
    let mut state = State::default();
    let groups = groups();
    state.sync(Some("first"), &groups);
    let entry = state.entries.get_mut("source").unwrap();
    entry.queue(Action::Clear { alias: "app".into() });
    let (cancel, sender) = pending_job(entry);
    state.sync(Some("second"), &groups);
    assert!(cancel.check().is_err());
    assert!(
        sender
            .send(job::Outcome {
                snapshot: None,
                error: Some("old session".into())
            })
            .is_err()
    );
    let entry = &state.entries["source"];
    assert!(entry.pending.is_none());
    assert!(entry.clearing.is_empty());
    assert!(entry.error.is_none());
    assert_eq!(entry.owner.scope.session_id, "second");
}

#[test]
fn session_bootstrap_cancels_old_jobs_without_using_the_old_inventory() {
    for blocked in ["runtime", "receiver", "failed"] {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.cloud_prototype.groups = groups();
        let state = &mut app.cloud_prototype.production.companions;
        state.sync(Some("first"), &app.cloud_prototype.groups);
        let entry = state.entries.get_mut("source").unwrap();
        entry.queue(Action::Select {
            alias: "app".into(),
            target_cloud_id: "target".into(),
        });
        let (cancel, sender) = pending_job(entry);
        app.active_session = Some(crate::app::ActiveSession {
            session_id: "second".into(),
            lease: None,
            last_lease_refresh: None,
            persistent: false,
        });
        app.pending_startup_runtime_state = (blocked == "runtime").then(horizon_core::RuntimeState::default);
        let (_bootstrap_sender, bootstrap_receiver) = channel();
        app.startup_receiver = (blocked == "receiver").then_some(bootstrap_receiver);
        app.startup_bootstrap_failure =
            (blocked == "failed").then_some(crate::app::StartupBootstrapFailure::WorkerDisconnected);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            crate::app::test_support::run_app_frame(&ctx, &mut app);
            let state = &app.cloud_prototype.production.companions;
            assert!(state.entries.is_empty() && state.inventory.is_empty());
            assert_eq!(state.session.as_deref(), Some("second"));
        }
        assert!(cancel.check().is_err());
        assert!(
            sender
                .send(job::Outcome {
                    snapshot: None,
                    error: None
                })
                .is_err()
        );
    }
}

#[test]
fn moving_source_drops_presentation_and_rebinds_ownership() {
    let mut state = State::default();
    let mut groups = groups();
    state.sync(Some("session"), &groups);
    let (cancel, _) = pending_job(state.entries.get_mut("source").unwrap());
    groups.0[0].workspace = "second-workspace".into();
    state.sync(Some("session"), &groups);
    assert!(cancel.check().is_err());
    assert_eq!(state.entries["source"].owner.scope.workspace_id, "second-workspace");
    assert!(state.entries["source"].snapshot.is_none());
}

#[test]
fn revision_change_cancels_stale_jobs_and_removed_cloud_cannot_receive_results() {
    let mut state = State::default();
    let mut groups = groups();
    state.sync(Some("session"), &groups);
    let (cancel, _) = pending_job(state.entries.get_mut("source").unwrap());
    groups.0[0].remote.as_mut().unwrap().revision = "b".repeat(40);
    state.sync(Some("session"), &groups);
    assert!(cancel.check().is_err());
    assert!(state.entries["source"].error.is_some());
    let (cancel, sender) = pending_job(state.entries.get_mut("source").unwrap());
    state.sync(Some("session"), &CloudGroups::default());
    assert!(cancel.check().is_err());
    assert!(state.entries.is_empty());
    assert!(
        sender
            .send(job::Outcome {
                snapshot: None,
                error: None
            })
            .is_err()
    );
}

#[test]
fn lost_job_surfaces_failure_without_staying_busy() {
    let mut entry = Entry::new(Owner {
        scope: Scope {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
        },
        cloud_id: "source".into(),
    });
    let (_, sender) = pending_job(&mut entry);
    drop(sender);
    entry.poll();
    assert!(entry.job.is_none());
    assert!(entry.error.as_ref().unwrap().contains("without a result"));
}

#[test]
fn duplicate_cloud_ids_disable_companion_jobs_even_across_workspaces() {
    let mut state = State::default();
    let mut groups = groups();
    let mut other = groups.0[0].clone();
    other.workspace = "other".into();
    groups.0.push(other);
    state.sync(Some("session"), &groups);
    assert!(state.entries["source"].blocked);
    assert!(state.entries["source"].snapshot.is_none());
}

#[test]
fn unchecking_cancels_refresh_and_remains_queued_until_persisted() {
    let mut state = State::default();
    let groups = groups();
    state.sync(Some("session"), &groups);
    let entry = state.entries.get_mut("source").unwrap();
    let (cancel, _) = pending_job(entry);
    entry.queue(Action::Clear { alias: "app".into() });
    assert!(cancel.check().is_err());
    assert!(entry.job.is_none());
    let (_, sender) = pending_job(entry);
    sender
        .send(job::Outcome {
            snapshot: None,
            error: Some("Busy".into()),
        })
        .unwrap();
    entry.poll();
    assert!(entry.clearing.contains("app"));
}

#[test]
fn first_selection_remains_cancellable_while_connecting_and_after_an_uncertain_failure() {
    use horizon_core::cloud_runtime::companions::{Companion, Row, Status};
    for fail in [false, true] {
        let mut state = State::default();
        state.sync(Some("session"), &groups());
        let entry = state.entries.get_mut("source").unwrap();
        entry.queue(Action::Select {
            alias: "app".into(),
            target_cloud_id: "target".into(),
        });
        assert!(matches!(entry.pending.take(), Some(Action::Select { .. })));
        let (cancel, sender) = pending_job(entry);
        if fail {
            sender
                .send(job::Outcome {
                    snapshot: None,
                    error: Some("Connection lost".into()),
                })
                .unwrap();
            entry.poll();
        }
        let row = Row {
            companion: Companion {
                alias: "app".into(),
                repository: "example/app".into(),
                profile: "cpu".into(),
                selected: false,
                status: Status::Unselected,
                target_cloud_id: None,
                access: None,
            },
            candidates: vec![],
            error: None,
        };
        let ctx = egui::Context::default();
        let target = egui::pos2(8.0, 8.0);
        let mut action = None;
        for pressed in [None, None, Some(true), Some(false)] {
            let events = pressed.map_or_else(Vec::new, |pressed| {
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            });
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        action = view::render_row(
                            ui,
                            &row,
                            &mut String::new(),
                            true,
                            true,
                            entry.selecting.contains("app"),
                        );
                    },
                )
                .discard_textures();
        }
        assert!(matches!(&action, Some(Action::Clear { alias }) if alias == "app"));
        entry.queue(action.unwrap());
        assert!(cancel.check().is_err());
        assert!(entry.job.is_none());
        assert!(entry.selecting.is_empty());
        assert!(entry.clearing.contains("app"));
    }
}

#[test]
fn unrelated_inventory_changes_preserve_clear_but_discard_stale_select() {
    for clear in [true, false] {
        let mut state = State::default();
        let mut groups = groups();
        state.sync(Some("session"), &groups);
        let entry = state.entries.get_mut("source").unwrap();
        entry.queue(if clear {
            Action::Clear { alias: "app".into() }
        } else {
            Action::Select {
                alias: "app".into(),
                target_cloud_id: "target".into(),
            }
        });
        let mut other = groups.0[0].clone();
        other.remote.as_mut().unwrap().id = "unrelated".into();
        groups.0.push(other);
        state.sync(Some("session"), &groups);
        assert_eq!(state.entries["source"].clearing.contains("app"), clear);
        assert!(state.entries["source"].pending.is_none());
    }
}

#[test]
fn rapid_unchecks_preserve_every_alias_until_individually_acknowledged() {
    use horizon_core::cloud_runtime::companions::{Catalog, Companion, Row, Status};
    let mut state = State::default();
    let groups = groups();
    state.sync(Some("session"), &groups);
    let entry = state.entries.get_mut("source").unwrap();
    entry.queue(Action::Clear { alias: "app".into() });
    let (cancel, _) = pending_job(entry);
    entry.queue(Action::Clear {
        alias: "utility".into(),
    });
    assert!(cancel.check().is_err());
    assert_eq!(
        entry.clearing.iter().map(String::as_str).collect::<Vec<_>>(),
        ["app", "utility"]
    );
    let (_, sender) = pending_job(entry);
    let companions: Vec<_> = [("app", false), ("utility", true)]
        .into_iter()
        .map(|(alias, selected)| Companion {
            alias: alias.into(),
            repository: format!("example/{alias}"),
            profile: "cpu".into(),
            selected,
            status: Status::Stopped,
            target_cloud_id: Some(alias.into()),
            access: None,
        })
        .collect();
    sender
        .send(job::Outcome {
            snapshot: Some(Snapshot {
                catalog: Catalog {
                    version: 1,
                    source_cloud_id: "source".into(),
                    observed_at: 1,
                    companions: companions.clone(),
                },
                rows: companions
                    .into_iter()
                    .map(|companion| Row {
                        companion,
                        candidates: vec![],
                        error: None,
                    })
                    .collect(),
                publication_error: None,
                notice: None,
            }),
            error: None,
        })
        .unwrap();
    entry.poll();
    assert!(!entry.clearing.contains("app"));
    assert!(entry.clearing.contains("utility"));
}
