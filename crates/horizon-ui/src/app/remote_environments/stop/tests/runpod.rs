use super::*;

fn retained() -> RemoteEnvironmentSummary {
    let mut expected = check_summary();
    expected.saved_phase = Some(RemoteRuntimePhase::Ready);
    expected
}

#[test]
fn first_runpod_stop_and_saved_intent_check_are_mutually_exclusive() {
    let mut expected = retained();
    assert_eq!(supported(&expected), cfg!(target_os = "linux"));
    assert!(!check_supported(&expected));
    for phase in [
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
    ] {
        expected.saved_phase = Some(phase);
        assert!(!supported(&expected));
        assert_eq!(check_supported(&expected), cfg!(target_os = "linux"));
    }
    for fault in 0..3 {
        let mut expected = retained();
        match fault {
            0 => expected.worker_identity = None,
            1 => expected.lifetime = WorkerLifetime::TimeLimited { seconds: 900 },
            _ => expected.worker_identity.as_mut().expect("worker").provider = CloudProvider::LocalDocker,
        }
        assert!(!supported(&expected));
    }
}

#[test]
fn runpod_start_eligibility_requires_exact_persistent_identity_and_saved_stop() {
    let mut expected = retained();
    for (phase, allowed) in [
        (RemoteRuntimePhase::Ready, false),
        (RemoteRuntimePhase::Reconciling, false),
        (RemoteRuntimePhase::Stopping { requested_at_millis: 1 }, false),
        (
            RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
            true,
        ),
        (RemoteRuntimePhase::Starting { requested_at_millis: 3 }, true),
        (RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 }, false),
        (
            RemoteRuntimePhase::Deleted {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
            false,
        ),
    ] {
        expected.saved_phase = Some(phase);
        assert_eq!(
            start_supported(&expected),
            allowed && cfg!(target_os = "linux"),
            "{phase:?}"
        );
    }
    expected.saved_phase = Some(RemoteRuntimePhase::Starting { requested_at_millis: 3 });
    for fault in 0..4 {
        let mut altered = expected.clone();
        match fault {
            0 => altered.worker_identity = None,
            1 => altered.worker_identity.as_mut().expect("identity").provider = CloudProvider::Azure,
            2 => altered.lifetime = WorkerLifetime::TimeLimited { seconds: 900 },
            _ => altered.saved_phase = None,
        }
        assert!(!start_supported(&altered));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;

    fn visible_text(shapes: &[egui::epaint::ClippedShape]) -> String {
        fn append(shape: &egui::Shape, text: &mut String) {
            match shape {
                egui::Shape::Text(shape) => {
                    text.push_str(shape.galley.text());
                    text.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        append(shape, text);
                    }
                }
                _ => {}
            }
        }
        let mut text = String::new();
        for shape in shapes {
            append(&shape.shape, &mut text);
        }
        text
    }

    fn stopped() -> RemoteEnvironmentSummary {
        let mut expected = retained();
        expected.saved_phase = Some(RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        });
        expected
    }

    fn started(expected: &RemoteEnvironmentSummary) -> ConfiguredStart {
        let mut saved = expected.clone();
        saved.revision += if matches!(expected.saved_phase, Some(RemoteRuntimePhase::Starting { .. })) {
            1
        } else {
            2
        };
        saved.saved_phase = Some(RemoteRuntimePhase::Reconciling);
        ConfiguredStart {
            saved,
            lifecycle: horizon_core::cloud_run::interactive_worker::InteractiveWorkerLifecycle::Provisioning,
            already_running: false,
        }
    }

    #[test]
    fn runpod_start_confirmation_discloses_billing_without_azure_price_or_implicit_authority() {
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("unused"));
        let expected = stopped();
        let ctx = Context::default();
        let mut state = StopState::default();
        for size in [[1200.0, 900.0], [800.0, 600.0]] {
            state.prepare_start(&expected, &config(), &ctx);
            let mut input = raw_input(size, None);
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
            let mut action = InventoryAction::None;
            let output = ctx.run_ui(input, |ui| show(ui, &state, &expected, true, &mut action));
            let text = visible_text(&output.shapes);
            let _ = output.discard_textures();
            for required in [
                "Start this environment?",
                "Compute billing resumes",
                "does not quote a price",
                "spending cap",
                "separately billable",
                "exact saved HPS",
                "public pin",
                "no private SSH key",
                "RUNPOD_API_KEY",
                "five minutes",
                "no task resumes",
                "exiting Horizon",
            ] {
                assert!(text.contains(required), "missing {required}: {text}");
            }
            assert!(!text.contains("Azure CLI") && !text.contains("currency units per hour"));
            assert!(matches!(action, InventoryAction::None));
            assert_eq!(
                ctx.data(|d| d.get_temp::<bool>(egui::Id::new("start-request-enabled-test"))),
                Some(true)
            );
            assert_eq!(
                ctx.data(|d| d.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
                Some(false)
            );
            state.cancel_confirmation();
            assert!(!state.start(&home, &config(), &expected, &ctx));
        }
        for config_drift in [false, true] {
            state.prepare_start(&expected, &config(), &ctx);
            let mut changed = expected.clone();
            if !config_drift {
                changed.revision += 1;
            }
            let configuration = if config_drift {
                RemoteProviderConfig::default()
            } else {
                config()
            };
            assert!(!state.start(&home, &configuration, &changed, &ctx));
        }
        assert!(!home.root().exists());
    }

    #[test]
    fn runpod_start_results_are_exact_and_do_not_claim_azure_login_or_task_replay() {
        let mut expected = stopped();
        for retry in [false, true] {
            if retry {
                expected.saved_phase = Some(RemoteRuntimePhase::Starting { requested_at_millis: 3 });
            }
            let valid = started(&expected);
            let notice = StopNotice::started(expected.clone(), Ok(valid.clone()));
            assert!(notice.succeeded && notice.started);
            assert!(notice.message.contains("Nothing resumed a task"));
            assert!(notice.message.contains("not attested"));
            for fault in 0..7 {
                let mut result = valid.clone();
                match fault {
                    0 => result.saved.revision += 1,
                    1 => result.saved.provider = CloudProvider::Azure,
                    2 => result.saved.generation += 1,
                    3 => result.saved.panel_count += 1,
                    4 => result.saved.worker_identity.as_mut().expect("worker").resource_id = "foreign-worker".into(),
                    5 => result.saved.saved_phase = Some(RemoteRuntimePhase::Ready),
                    _ => result.saved.profile = "foreign-profile".into(),
                }
                assert!(!StopNotice::started(expected.clone(), Ok(result)).succeeded);
            }
        }
        let error = StopError::RunPodStart(ConfiguredRunPodStartError::Start(
            horizon_core::remote_workspace::start::RemoteWorkspaceStartError::ProviderUnavailable,
        ));
        let state = StopState {
            notice: Some(StopNotice::started(expected.clone(), Err(error))),
            ..Default::default()
        };
        let ctx = Context::default();
        let mut action = InventoryAction::None;
        let output = ctx.run_ui(raw_input([1200.0, 900.0], None), |ui| {
            show(ui, &state, &expected, true, &mut action);
        });
        let text = visible_text(&output.shapes);
        let _ = output.discard_textures();
        assert!(text.contains("Compute may already be billing"));
        assert!(!text.contains("Azure CLI") && !text.contains("Sign in"));
        let wrong = StopNotice::finish(expected.clone(), Operation::Start, Ok(StopResult::Stopped(expected)));
        assert!(!wrong.succeeded && wrong.started);
    }

    #[test]
    fn pending_start_is_single_flight_and_closed_or_changed_views_discard_without_replay() {
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("unused"));
        let expected = stopped();
        let ctx = Context::default();
        for action in [
            InventoryAction::Close,
            InventoryAction::Select(1),
            InventoryAction::None,
        ] {
            let mut state = view(&expected);
            let (tx, rx) = mpsc::sync_channel(1);
            state.stop.pending = Some(PendingStop {
                rx,
                expected: expected.clone(),
                operation: Operation::Start,
                discard: false,
            });
            assert!(state.stop.pending_label().contains("RunPod Start"));
            for _ in 0..3 {
                state.stop.prepare_start(&expected, &config(), &ctx);
                assert!(state.stop.confirmation.is_none());
                assert!(!state.stop.start(&home, &config(), &expected, &ctx));
                assert!(!state.stop.check(&home, &config(), &expected, &ctx));
                assert!(!state.stop.drain_result());
            }
            if matches!(action, InventoryAction::None) {
                state.invalidate_provider_state();
            } else {
                state.apply(action, &home, &ctx);
            }
            tx.send(Ok(StopResult::Started(started(&expected))))
                .expect("late result");
            assert!(state.stop.drain_result());
            assert!(state.stop.notice.is_none() && !state.stop.is_pending());
        }
        assert!(!home.root().exists());
        assert!(matches!(
            execute_start(&home, &config(), &expected),
            Err(StopError::StorageUnavailable)
        ));
        assert!(!home.root().exists());
        let store = CloudWorkflowStore::open(&home).expect("fixture store");
        let before = snapshot(store.path());
        assert!(matches!(
            execute_start(&home, &config(), &expected),
            Err(StopError::RunPodStart(ConfiguredRunPodStartError::Configuration(_)))
        ));
        assert_eq!(snapshot(store.path()), before);
    }

    #[test]
    fn confirmation_discloses_cloud_limits_and_cancel_enter_or_drift_do_not_submit() {
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("unused"));
        let expected = retained();
        let ctx = Context::default();
        let mut state = StopState::default();
        for size in [[1200.0, 900.0], [960.0, 720.0]] {
            state.prepare(&expected, &config(), &ctx);
            let mut input = raw_input(size, None);
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
            let mut action = InventoryAction::None;
            let output = ctx.run_ui(input, |ui| show(ui, &state, &expected, true, &mut action));
            let text = visible_text(&output.shapes);
            let _ = output.discard_textures();
            for required in [
                "Unsaved process memory is lost",
                "exact saved HPS",
                "no private SSH key",
                "not filesystem durability",
                "storage may still be billed",
                "never resend",
                "exiting Horizon",
            ] {
                assert!(text.contains(required), "missing {required}: {text}");
            }
            assert!(!text.contains("Retains this local container"));
            assert!(matches!(action, InventoryAction::None));
            assert_eq!(
                ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
                Some(true)
            );
            assert_eq!(
                ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-check-enabled-test"))),
                Some(false)
            );
            state.cancel_confirmation();
            assert!(!state.start(&home, &config(), &expected, &ctx));
        }
        for config_drift in [false, true] {
            state.prepare(&expected, &config(), &ctx);
            let mut selected = expected.clone();
            if !config_drift {
                selected.revision += 1;
            }
            let config = if config_drift {
                RemoteProviderConfig::default()
            } else {
                config()
            };
            assert!(!state.start(&home, &config, &selected, &ctx));
        }
        assert!(!state.is_pending());
        assert!(!home.root().exists());
    }

    #[test]
    fn runpod_stop_requires_exact_completion_and_rejects_drift_or_wrong_callback_kind() {
        let expected = retained();
        assert!(StopNotice::new(expected.clone(), Ok(completed(&expected))).succeeded);
        let mut changes = vec![completed(&expected); 8];
        changes[0].revision += 1;
        changes[1].workflow_id = Some(CloudWorkflowId::new());
        changes[2].repository = "foreign/repository".into();
        changes[3].panel_count += 1;
        changes[4].saved_phase = Some(RemoteRuntimePhase::Stopped {
            requested_at_millis: -1,
            observed_at_millis: 2,
        });
        changes[5].saved_phase = Some(RemoteRuntimePhase::Stopped {
            requested_at_millis: 3,
            observed_at_millis: 2,
        });
        changes[6].generation += 1;
        changes[7].lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
        for changed in changes {
            assert!(!StopNotice::new(expected.clone(), Ok(changed)).succeeded);
        }
        let mut overflow = expected.clone();
        overflow.revision = u64::MAX;
        assert!(!valid_runpod_stop_result(&overflow, &completed(&expected)));
        let wrong_kind = StopNotice::finish(
            expected.clone(),
            Operation::Stop,
            Ok(StopResult::Checked(ConfiguredStopConfirmation {
                saved: completed(&expected),
                observation: InteractiveWorkerStopObservation::RetainedStopped,
            })),
        );
        assert!(!wrong_kind.succeeded);
    }

    #[test]
    fn pending_runpod_stop_blocks_other_operations_and_closed_or_changed_view_discards_late_result() {
        let fixture = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(fixture.path().join("unused"));
        let expected = retained();
        let ctx = Context::default();
        for action in [
            InventoryAction::Close,
            InventoryAction::Select(1),
            InventoryAction::None,
        ] {
            let mut state = view(&expected);
            let sender = pending(&mut state.stop, &expected);
            assert!(state.stop.pending_label().contains("Exiting Horizon may interrupt"));
            assert!(state.stop.pending_label().contains("never resend"));
            for _ in 0..20 {
                state.stop.prepare(&expected, &config(), &ctx);
                assert!(!state.stop.start(&home, &config(), &expected, &ctx));
                assert!(!state.stop.check(&home, &config(), &check_summary(), &ctx));
                state.start_observation(&home, &config(), &ctx);
                assert!(!state.observation.is_pending());
                assert!(!state.stop.drain_result());
            }
            if matches!(action, InventoryAction::None) {
                state.invalidate_provider_state();
            } else {
                state.apply(action, &home, &ctx);
            }
            sender
                .send(Ok(StopResult::Stopped(completed(&expected))))
                .expect("late reply");
            assert!(state.stop.drain_result());
            assert!(state.stop.notice.is_none());
            assert!(!state.stop.is_pending());
        }
        assert!(!home.root().exists());
    }

    fn snapshot(path: &std::path::Path) -> (Vec<u8>, i64, String) {
        let reader =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("reader");
        (
            std::fs::read(path).expect("bytes"),
            reader
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .expect("version"),
            reader
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .expect("journal"),
        )
    }

    #[test]
    fn actual_runpod_stop_callback_never_creates_repairs_or_migrates_the_store() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().expect("fixture");
        let missing = HorizonHome::from_root(fixture.path().join("missing"));
        assert!(matches!(
            execute(&missing, &config(), &retained()),
            Err(StopError::StorageUnavailable)
        ));
        assert!(!missing.root().exists());
        for (name, sql) in [
            ("legacy", "DROP TABLE remote_provider_bindings; PRAGMA user_version=6;"),
            ("index", "DROP INDEX remote_workspaces_session;"),
            (
                "trigger",
                "CREATE TRIGGER unexpected_stop_write AFTER UPDATE ON cloud_workflows BEGIN DELETE FROM remote_workspaces; END;",
            ),
            ("mode", ""),
            ("current", ""),
        ] {
            let home = HorizonHome::from_root(fixture.path().join(name));
            let store = CloudWorkflowStore::open(&home).expect("owned fixture");
            let connection = rusqlite::Connection::open(store.path()).expect("fixture writer");
            connection.execute_batch(sql).expect("schema fixture");
            connection
                .pragma_update(None, "journal_mode", "DELETE")
                .expect("commit fixture");
            drop(connection);
            if name == "mode" {
                std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644))
                    .expect("insecure fixture");
            }
            let before = snapshot(store.path());
            let result = execute(&home, &config(), &retained());
            assert_eq!(snapshot(store.path()), before, "{name}: {result:?}");
            if name == "current" {
                assert!(matches!(
                    result,
                    Err(StopError::RunPod(ConfiguredRunPodStopError::Configuration(_)))
                ));
            } else {
                assert!(
                    matches!(result, Err(StopError::StorageUnavailable)),
                    "{name}: {result:?}"
                );
            }
            if name == "mode" {
                assert_eq!(
                    std::fs::metadata(store.path()).expect("metadata").permissions().mode() & 0o777,
                    0o644
                );
            }
        }
    }
}
