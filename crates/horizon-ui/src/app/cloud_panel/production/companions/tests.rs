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
        placement: horizon_core::cloud_panel::Placement::default(),
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
            .is_ok()
    );
    assert_eq!(state.retiring.len(), 1);
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
            persistent: true,
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
                .is_ok()
        );
        assert_eq!(app.cloud_prototype.production.companions.retiring.len(), 1);
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
            .is_ok()
    );
    assert_eq!(state.retiring.len(), 1);
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
    let (cancel, sender) = pending_job(entry);
    entry.queue(Action::Clear { alias: "app".into() });
    assert!(cancel.check().is_err());
    state.tick(Path::new("/absent"), &groups, &egui::Context::default());
    let entry = state.entries.get_mut("source").unwrap();
    assert!(entry.job.as_ref().unwrap().cancel.check().is_err());
    sender
        .send(job::Outcome {
            snapshot: Some(Snapshot {
                catalog: horizon_core::cloud_runtime::companions::Catalog {
                    version: 1,
                    source_cloud_id: "source".into(),
                    observed_at: 1,
                    companions: vec![],
                },
                rows: vec![],
                publication_error: None,
                notice: None,
            }),
            error: None,
        })
        .unwrap();
    entry.poll();
    assert!(entry.job.is_none() && entry.snapshot.is_none());
    assert!(entry.clearing.contains("app"));
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
        assert_eq!(entry.job.is_none(), fail);
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
        let entry = state.entries.get_mut("source").unwrap();
        let (_, sender) = pending_job(entry);
        sender
            .send(job::Outcome {
                snapshot: None,
                error: Some("Unavailable".into()),
            })
            .unwrap();
        entry.poll();
        assert!(entry.selecting.is_empty());
    }
}

#[test]
fn idle_companions_schedule_only_the_earliest_unblocked_refresh() {
    let groups = groups();
    let delay = |state: &mut State| {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();
        }
        ctx.run_ui(egui::RawInput::default(), |ui| {
            state.tick(Path::new("/absent"), &groups, ui.ctx());
        })
        .discard_textures()
        .viewport_output[&egui::ViewportId::ROOT]
            .repaint_delay
    };
    let mut state = State::default();
    state.sync(Some("session"), &groups);
    let entry = state.entries.get_mut("source").unwrap();
    entry.due = Instant::now() + REFRESH;
    let mut other = Entry::new(entry.owner.clone());
    other.blocked = true;
    state.entries.insert("blocked".into(), other);
    assert!(delay(&mut state) > REFRESH / 2);
    let other = state.entries.get_mut("blocked").unwrap();
    other.blocked = false;
    other.due = Instant::now() + Duration::from_secs(5);
    let refresh = delay(&mut state);
    assert!(refresh > Duration::from_secs(2) && refresh <= Duration::from_secs(5));
    let (_, _sender) = pending_job(state.entries.get_mut("source").unwrap());
    let polling = delay(&mut state);
    assert!(!polling.is_zero() && polling <= Duration::from_millis(100));
    state.entries.clear();
    assert!(delay(&mut state) > Duration::from_secs(60));
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
    entry.poll();
    assert!(entry.job.is_none());
    assert!(entry.error.is_none());
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

#[test]
fn retired_jobs_fence_session_roundtrips_and_readded_clouds() {
    for switch_session in [false, true] {
        for reply in [false, true] {
            let mut state = State::default();
            let groups = groups();
            state.sync(Some("first"), &groups);
            let (cancel, sender) = pending_job(state.entries.get_mut("source").unwrap());
            if switch_session {
                state.sync(Some("second"), &groups);
            } else {
                state.sync(Some("first"), &CloudGroups::default());
            }
            state.sync(Some("first"), &groups);
            state.tick(Path::new("/absent"), &groups, &egui::Context::default());
            assert!(cancel.check().is_err());
            assert_eq!(state.retiring.len(), 1);
            assert!(state.entries["source"].job.is_none());
            if reply {
                sender
                    .send(job::Outcome {
                        snapshot: None,
                        error: Some("stale".into()),
                    })
                    .unwrap();
            }
            drop(sender);
            state.entries.get_mut("source").unwrap().due = Instant::now() + REFRESH;
            state.tick(Path::new("/absent"), &groups, &egui::Context::default());
            assert!(state.retiring.is_empty());
            assert!(state.entries["source"].error.is_none());
        }
    }
}

#[test]
fn ephemeral_sessions_never_own_persisted_companion_access() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    app.cloud_prototype.root = None;
    app.cloud_prototype.groups = groups();
    app.active_session = Some(crate::app::ActiveSession {
        session_id: "ephemeral".into(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    app.prepare_cloud_companions(&egui::Context::default());
    assert!(!app.cloud_prototype.production.companions.entries.is_empty());
    app.active_session.as_mut().unwrap().persistent = false;
    for _ in 0..2 {
        app.sync_cloud_companion_session(&egui::Context::default());
        let state = &app.cloud_prototype.production.companions;
        assert!(state.session.is_none() && state.entries.is_empty());
        app.prepare_cloud_companions(&egui::Context::default());
        let state = &app.cloud_prototype.production.companions;
        assert!(state.session.is_none() && state.entries.is_empty() && state.inventory.is_empty());
    }
}

#[cfg(unix)] // Companion journals require the Unix directory-durability contract.
#[test]
fn queued_revocation_survives_owner_roundtrips_and_absent_journals() {
    use horizon_core::cloud_runtime::{companions, settings::Settings};
    for transition in ["session", "remove", "workspace"] {
        for persist_selection in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "runpod_key_file":"/absent", "ssh_identity_file":"/absent", "docker_config":"/absent", "cpu_flavors":[], "gpu_types":[]
            })).unwrap();
            std::fs::write(
                root.path().join("settings.json"),
                serde_json::to_vec(&settings).unwrap(),
            )
            .unwrap();
            let mut state = State::default();
            let groups = groups();
            state.sync(Some("first"), &groups);
            let entry = state.entries.get_mut("source").unwrap();
            let owner = entry.owner.clone();
            let (_, sender) = pending_job(entry);
            entry.queue(Action::Clear { alias: "app".into() });
            match transition {
                "session" => state.sync(Some("second"), &groups),
                "remove" => state.sync(Some("first"), &CloudGroups::default()),
                _ => {
                    let mut moved = groups.clone();
                    moved.0[0].workspace = "other".into();
                    state.sync(Some("first"), &moved);
                }
            }
            state.sync(Some("first"), &groups);
            let source = companions::Target {
                scope: owner.scope.clone(),
                cloud_id: "source".into(),
                declaration: companions::Declaration {
                    repository: "example/library".into(),
                    profile: "cpu".into(),
                },
            };
            let target = companions::Target {
                cloud_id: "target".into(),
                declaration: companions::Declaration {
                    repository: "example/consumer".into(),
                    profile: "cpu".into(),
                },
                ..source.clone()
            };
            let snapshot = persist_selection.then(|| {
                companions::refresh(
                    &companions::Request {
                        root: root.path().to_owned(),
                        owner,
                        context: Some(companions::Context {
                            source: source.clone(),
                            declarations: [("app".into(), target.declaration.clone())].into(),
                            inventory: vec![source, target],
                        }),
                        action: Action::Select {
                            alias: "app".into(),
                            target_cloud_id: "target".into(),
                        },
                        settings,
                    },
                    &Cancellation::default(),
                )
                .unwrap()
            });
            sender.send(job::Outcome { snapshot, error: None }).unwrap();
            let (_app_root, mut app) = crate::app::test_support::test_app();
            app.cloud_prototype.root = Some(root.path().to_owned());
            app.cloud_prototype.groups = groups;
            app.cloud_prototype.production.companions = state;
            app.active_session = Some(crate::app::ActiveSession {
                session_id: "first".into(),
                lease: None,
                last_lease_refresh: None,
                persistent: true,
            });
            app.startup_bootstrap_failure = Some(crate::app::StartupBootstrapFailure::WorkerDisconnected);
            let ctx = egui::Context::default();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app.cloud_prototype.production.companions.retiring.is_empty() {
                crate::app::test_support::run_app_frame(&ctx, &mut app);
                assert!(
                    Instant::now() < deadline,
                    "revocation did not finish for {transition}, journal={persist_selection}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            let journal: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.path().join("source/companions.json")).unwrap()).unwrap();
            assert!(journal["grants"].as_object().unwrap().is_empty());
            assert!(
                app.cloud_prototype.production.companions.entries["source"]
                    .job
                    .is_none()
            );
        }
    }
}

#[cfg(unix)]
fn selected_request(root: &Path, owner: Owner) -> horizon_core::cloud_runtime::companions::Request {
    use horizon_core::cloud_runtime::{companions, settings::Settings};
    let source = companions::Target {
        scope: owner.scope.clone(),
        cloud_id: owner.cloud_id.clone(),
        declaration: companions::Declaration {
            repository: "example/library".into(),
            profile: "cpu".into(),
        },
    };
    let target = companions::Target {
        cloud_id: "target".into(),
        declaration: companions::Declaration {
            repository: "example/consumer".into(),
            profile: "cpu".into(),
        },
        ..source.clone()
    };
    companions::Request {
        root: root.to_owned(), owner,
        context: Some(companions::Context {
            source: source.clone(), declarations: [("app".into(), target.declaration.clone())].into(), inventory: vec![source, target],
        }),
        action: Action::Select { alias: "app".into(), target_cloud_id: "target".into() },
        settings: serde_json::from_value::<Settings>(serde_json::json!({
            "runpod_key_file":"/absent", "ssh_identity_file":"/absent", "docker_config":"/absent", "cpu_flavors":[], "gpu_types":[]
        })).unwrap(),
    }
}

#[cfg(unix)] // Subprocesses exercise paths that terminate the application process.
#[test]
fn shutdown_persists_queued_unchecks_before_both_exit_paths() {
    use horizon_core::cloud_runtime::companions;
    if let Some(root) = std::env::var_os("HORIZON_M0_SHUTDOWN_ROOT") {
        let root = std::path::PathBuf::from(root);
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.cloud_prototype.root = Some(root.clone());
        let state = &mut app.cloud_prototype.production.companions;
        state.sync(Some("first"), &groups());
        let entry = state.entries.get_mut("source").unwrap();
        let request = selected_request(&root, entry.owner.clone());
        companions::refresh(&request, &Cancellation::default()).unwrap();
        let (_, sender) = pending_job(entry);
        entry.queue(Action::Clear { alias: "app".into() });
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let snapshot = companions::refresh(&request, &Cancellation::default()).unwrap();
            sender
                .send(job::Outcome {
                    snapshot: Some(snapshot),
                    error: None,
                })
                .unwrap();
        });
        if std::env::var_os("HORIZON_M0_SHUTDOWN_FALLBACK").is_some() {
            eframe::App::on_exit(&mut app);
        } else {
            app.begin_shutdown();
            let ctx = egui::Context::default();
            loop {
                crate::app::test_support::run_app_frame(&ctx, &mut app);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        panic!("shutdown returned without exiting");
    }
    for fallback in [false, true] {
        let root = tempfile::tempdir().unwrap();
        // Saving the uncheck must not depend on provider settings being readable.
        std::fs::write(root.path().join("settings.json"), "invalid settings").unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "app::cloud_panel::production::companions::tests::shutdown_persists_queued_unchecks_before_both_exit_paths"])
            .env("HORIZON_M0_SHUTDOWN_ROOT", root.path());
        if fallback {
            command.env("HORIZON_M0_SHUTDOWN_FALLBACK", "1");
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("shutdown did not finish, fallback={fallback}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success());
        let journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("source/companions.json")).unwrap()).unwrap();
        assert_eq!(journal["grants"]["app"]["selected"], false);
    }
}

#[cfg(unix)]
#[test]
fn shutdown_retries_local_lock_failure_without_remote_cleanup() {
    use horizon_core::cloud_runtime::companions;
    let root = tempfile::tempdir().unwrap();
    let mut state = State::default();
    state.sync(Some("first"), &groups());
    let entry = state.entries.get_mut("source").unwrap();
    let request = selected_request(root.path(), entry.owner.clone());
    companions::refresh(&request, &Cancellation::default()).unwrap();
    let path = root.path().join("source/companions.json");
    let mut journal: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let grant = &mut journal["grants"]["app"];
    grant["source_worker"] = "original-source".into();
    grant["target_worker"] = "original-target".into();
    grant["revision"] = "a".repeat(40).into();
    grant["source_disconnected"] = false.into();
    grant["target_revoked"] = false.into();
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    let other_owner = Owner {
        scope: Scope {
            session_id: "other".into(),
            ..request.owner.scope.clone()
        },
        ..request.owner.clone()
    };
    companions::persist_deselections(root.path(), &other_owner, &["app".into()]).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap(),
        journal
    );
    entry.queue(Action::Clear { alias: "app".into() });
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("source/companions.lock"))
        .unwrap();
    lock.lock().unwrap();
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.retiring.first().is_none_or(|entry| entry.error.is_none()) {
        assert!(!state.finish_shutdown(Some(root.path()), &ctx));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(state.retiring[0].clearing.contains("app"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap(),
        journal
    );
    lock.unlock().unwrap();
    state.retiring[0].due = Instant::now();
    while !state.finish_shutdown(Some(root.path()), &ctx) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    journal["grants"]["app"]["selected"] = false.into();
    let actual: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        actual, journal,
        "offline revocation pins must survive for later cleanup"
    );
    companions::persist_deselections(root.path(), &request.owner, &["app".into()]).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap(),
        actual
    );
}
