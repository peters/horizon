use super::*;
use horizon_app_testing::catalog::Device;
use horizon_app_testing::contract::Form;
use horizon_app_testing::recipe::{State, Target};
#[cfg(unix)]
use std::fmt::Write as _;
#[cfg(unix)]
use std::sync::Arc;
use std::sync::Barrier;

#[derive(Default)]
struct Fake {
    screenshots: AtomicUsize,
    actions: AtomicUsize,
    builds: Mutex<Vec<Platform>>,
    media_calls: Mutex<Vec<(Uuid, horizon_app_provider::media::Kind)>>,
    uploads: Mutex<Vec<Platform>>,
    sessions: Mutex<BTreeMap<Uuid, usize>>,
    active: AtomicUsize,
    maximum: AtomicUsize,
    released: AtomicUsize,
    first_two: Option<Barrier>,
    fail_create: Option<Box<dyn Fn(Instant) -> Error + Send + Sync>>,
    failed_creates: AtomicUsize,
    create_cleanup_confirmed: bool,
    fail_reset_cleanup: Option<bool>,
    fail_action: Option<Error>,
    fail_close: bool,
}
impl Runtime for Fake {
    fn build(&self, platform: Platform, _control: &Control) -> Result<()> {
        self.builds.lock().unwrap().push(platform);
        Ok(())
    }
    fn upload(&self, platform: Platform, _deadline: Instant) -> Result<Uuid> {
        self.uploads.lock().unwrap().push(platform);
        Ok(Uuid::new_v4())
    }
    fn create(&self, index: usize, _app: Uuid, deadline: Instant) -> Creation {
        if let Some(failure) = &self.fail_create {
            self.failed_creates.fetch_add(1, Ordering::SeqCst);
            return Err(OperationFailure {
                error: failure(deadline),
                cleanup_confirmed: self.create_cleanup_confirmed,
            });
        }
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        let id = Uuid::new_v4();
        self.sessions.lock().unwrap().insert(id, index);
        if index < 2
            && let Some(barrier) = &self.first_two
        {
            barrier.wait();
        }
        Ok(id)
    }
    fn act(&self, session: Uuid, action: &Action) -> std::result::Result<Option<Uuid>, OperationFailure> {
        self.actions.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.fail_action {
            return Err(OperationFailure {
                error,
                cleanup_confirmed: true,
            });
        }
        if matches!(action, Action::Reset {}) {
            if let Some(cleanup_confirmed) = self.fail_reset_cleanup {
                return Err(OperationFailure {
                    error: horizon_app_provider::Error::TunnelStartFailed.into(),
                    cleanup_confirmed,
                });
            }
            let mut sessions = self.sessions.lock().unwrap();
            let index = sessions.remove(&session).unwrap();
            let replacement = Uuid::new_v4();
            sessions.insert(replacement, index);
            return Ok(Some(replacement));
        }
        if self.sessions.lock().unwrap()[&session] == 0 {
            return Err(OperationFailure {
                error: horizon_app_testing::Error::AssertionFailed.into(),
                cleanup_confirmed: true,
            });
        }
        Ok(None)
    }
    fn screenshot(&self, _session: Uuid) -> Result<Vec<u8>> {
        self.screenshots.fetch_add(1, Ordering::SeqCst);
        Ok(b"synthetic-private-png".to_vec())
    }
    fn media(&self, session: Uuid, kind: horizon_app_provider::media::Kind, timeout: Duration) -> Result<Vec<u8>> {
        assert!(!self.sessions.lock().unwrap().contains_key(&session));
        assert!(!timeout.is_zero());
        self.media_calls.lock().unwrap().push((session, kind));
        if matches!(kind, horizon_app_provider::media::Kind::Network) {
            return Err(horizon_app_provider::Error::MediaUnavailable.into());
        }
        Ok(b"validated-redacted-fixture".to_vec())
    }
    fn close(&self, session: Uuid) -> Result<()> {
        self.sessions.lock().unwrap().remove(&session).unwrap();
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.fail_close {
            Err(Error::CleanupUncertain)
        } else {
            Ok(())
        }
    }
    fn release(&self, _app: Uuid) -> Result<()> {
        self.released.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn targets() -> Vec<ResolvedDevice> {
    [
        (Platform::Ios, Form::Phone),
        (Platform::Ios, Form::Tablet),
        (Platform::Android, Form::Phone),
    ]
    .into_iter()
    .enumerate()
    .map(|(matrix_index, (platform, form))| ResolvedDevice {
        matrix_index,
        device: Device {
            platform,
            form,
            model: format!("synthetic-{matrix_index}"),
            os_version: "27".into(),
        },
    })
    .collect()
}
fn recipe() -> Recipe {
    Recipe {
        version: 1,
        id: "smoke".into(),
        platforms: None,
        steps: vec![horizon_app_testing::recipe::Step {
            id: "visible".into(),
            action: Action::Assert {
                target: Target::Identifier("home".into()),
                state: State::Visible,
            },
        }],
    }
}
fn capture(_session: Uuid, _kind: CaptureKind, bytes: &[u8]) -> Result<Evidence> {
    if bytes.is_empty() {
        return Err(Error::Unavailable);
    }
    Ok(Evidence {
        id: Uuid::new_v4(),
        path: Some("/synthetic-private/frame.png".into()),
        bytes: bytes.len(),
        state: EvidenceState::Available,
    })
}

#[test]
fn parallel_matrix_builds_and_uploads_each_platform_once_and_isolates_device_failure() {
    let fake = Fake {
        first_two: Some(Barrier::new(2)),
        ..Fake::default()
    };
    let control = Control::new(Duration::from_secs(10)).unwrap();
    let recipes = [recipe()];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 2,
        screenshots: true,
        video: false,
        logs_on_failure: false,
    };
    let report = plan.execute(&fake, &control, capture, |_| Ok(())).unwrap();
    assert_eq!(*fake.builds.lock().unwrap(), [Platform::Ios, Platform::Android]);
    assert_eq!(*fake.uploads.lock().unwrap(), [Platform::Ios, Platform::Android]);
    assert_eq!(fake.maximum.load(Ordering::SeqCst), 2); // Barrier proves simultaneous allocation, not just a requested limit.
    assert_eq!(report.devices.len(), 3);
    assert!(!report.devices[0].steps[0].passed);
    assert!(
        report.devices[1..]
            .iter()
            .all(|device| device.steps[0].passed && device.steps[0].screenshot.is_some())
    );
    assert!(report.devices.iter().all(|device| device.cleanup_confirmed));
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(fake.released.load(Ordering::SeqCst), 2);
}

#[test]
fn cancellation_closes_every_allocated_lane_and_prevents_later_device_creation() {
    let fake = Fake {
        first_two: Some(Barrier::new(2)),
        ..Fake::default()
    };
    let control = Control::new(Duration::from_secs(10)).unwrap();
    let recipes = [recipe()];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 2,
        screenshots: false,
        video: false,
        logs_on_failure: false,
    };
    let report = plan
        .execute(&fake, &control, capture, |progress| {
            if progress.phase == "step" {
                control.cancel();
            }
            Ok(())
        })
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.devices.len(), 3);
    assert!(report.devices[2].session.is_none());
    assert!(report.devices.iter().all(|device| device.cleanup_confirmed));
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(fake.released.load(Ordering::SeqCst), 2);
}

#[test]
fn progress_failure_after_one_upload_retires_that_owned_upload() {
    let fake = Fake::default();
    let control = Control::new(Duration::from_secs(10)).unwrap();
    let recipes = [recipe()];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 1,
        screenshots: false,
        video: false,
        logs_on_failure: false,
    };
    let progress_count = AtomicUsize::new(0);
    assert!(
        plan.execute(&fake, &control, capture, |_| {
            if progress_count.fetch_add(1, Ordering::SeqCst) == 1 {
                Err(Error::Unavailable)
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert_eq!(fake.released.load(Ordering::SeqCst), 1);
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
}

#[test]
// The actor fixture uses Unix process guardians and filesystem permissions.
#[cfg(unix)]
fn conservative_provider_overlap_waits_then_allocates_both_owned_sessions() {
    use std::sync::mpsc;
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(20)).unwrap();
    let (entered, observed) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    *fixture.fake.transport.after_create.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        resumed.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    std::thread::scope(|scope| {
        let first = scope.spawn(|| actor.create(0, app.id, Duration::from_secs(15)));
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        // The first POST exists remotely but its exact allocation ID is not in the ledger yet.
        let deferred = actor.create(0, app.id, Duration::from_secs(15));
        assert!(matches!(deferred, Err(Error::AdmissionDeferred)), "{deferred:?}");
        assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 1);
        let control = Control::new(Duration::from_secs(10)).unwrap();
        let actor_ref = &actor;
        let second = scope.spawn(move || allocate(actor_ref, &control, 0, app.id));
        // Reconciliation of the first positive reply removes the conservative overlap.
        resume.send(()).unwrap();
        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();
        assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 2);
        assert!(actor.tunnel_status(first.id).unwrap().ready);
        assert!(actor.tunnel_status(second).unwrap().ready);
        actor.close(first.id).unwrap();
        actor.close(second).unwrap();
    });
    actor.release_upload(app.id).unwrap();
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
}

#[test]
// The actor fixture uses Unix process guardians and filesystem permissions.
#[cfg(unix)]
fn cancelled_admission_does_not_send_another_native_post() {
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(10)).unwrap();
    let control = Control::new(Duration::from_secs(5)).unwrap();
    control.cancel();
    assert_eq!(
        allocate(&actor, &control, 0, app.id).map_err(|failure| failure.error),
        Err(Error::Cancelled)
    );
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 0);
    actor.release_upload(app.id).unwrap();
}

#[test]
fn callback_panic_closes_every_known_session_before_releasing_uploads() {
    let runtime = Fake::default();
    let recipe = recipe();
    let plan = Plan {
        targets: targets(),
        recipes: &[recipe],
        parallel: 2,
        screenshots: true,
        video: false,
        logs_on_failure: false,
    };
    let control = Control::new(Duration::from_secs(5)).unwrap();
    let result = plan.execute(
        &runtime,
        &control,
        |_, _, _| panic!("synthetic capture failure"),
        |_| Ok(()),
    );
    assert!(matches!(result, Err(Error::Unavailable)));
    assert!(runtime.sessions.lock().unwrap().is_empty());
    assert_eq!(runtime.active.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.released.load(Ordering::SeqCst), 2);
}

#[test]
fn deferred_admission_obeys_mid_attempt_cancellation_and_the_original_deadline() {
    for cancel in [true, false] {
        let control = std::sync::Arc::new(
            Control::new(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(1)
            })
            .unwrap(),
        );
        let held = control.clone();
        let runtime = Fake {
            fail_create: Some(Box::new(move |deadline| {
                assert_eq!(deadline, held.deadline);
                if cancel {
                    held.cancel();
                } else {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(20));
                }
                Error::AdmissionDeferred
            })),
            create_cleanup_confirmed: true,
            ..Fake::default()
        };
        let failure = allocate(&runtime, &control, 0, Uuid::new_v4()).unwrap_err();
        assert!(failure.cleanup_confirmed);
        let error = failure.error;
        assert_eq!(
            error,
            if cancel {
                Error::Cancelled
            } else {
                horizon_app_runtime::Error::OperationExpired.into()
            }
        );
        assert_eq!(runtime.failed_creates.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.active.load(Ordering::SeqCst), 0);
    }
}
#[test]
fn an_uncertain_native_post_is_never_retried_by_the_matrix_runner() {
    let runtime = Fake {
        fail_create: Some(Box::new(|_| horizon_app_testing::Error::AllocationUncertain.into())),
        ..Fake::default()
    };
    let control = Control::new(Duration::from_secs(5)).unwrap();
    assert_eq!(
        allocate(&runtime, &control, 0, Uuid::new_v4()).map_err(|failure| failure.error),
        Err(horizon_app_testing::Error::AllocationUncertain.into())
    );
    assert_eq!(runtime.failed_creates.load(Ordering::SeqCst), 1);
}

#[test]
fn reset_replacement_is_closed_after_capture_failure_and_report_uses_the_new_handle() {
    let fake = Fake::default();
    let control = Control::new(Duration::from_secs(10)).unwrap();
    let recipes = [Recipe {
        version: 1,
        id: "reset".into(),
        platforms: None,
        steps: vec![horizon_app_testing::recipe::Step {
            id: "fresh".into(),
            action: Action::Reset {},
        }],
    }];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 2,
        screenshots: true,
        video: false,
        logs_on_failure: false,
    };
    let report = plan
        .execute(
            &fake,
            &control,
            |session, _, _| {
                assert!(fake.sessions.lock().unwrap().contains_key(&session));
                Err(Error::Unavailable)
            },
            |_| Ok(()),
        )
        .unwrap();
    assert!(
        report
            .devices
            .iter()
            .all(|device| device.session.is_some() && device.cleanup_confirmed && !device.steps[0].passed)
    );
    assert!(fake.sessions.lock().unwrap().is_empty());
    assert_eq!(fake.active.load(Ordering::SeqCst), 0);
    assert_eq!(fake.released.load(Ordering::SeqCst), 2);
}

#[test]
fn run_retains_finalized_video_and_failure_logs_after_exact_session_close() {
    let fake = Fake::default();
    let control = Control::new(Duration::from_secs(10)).unwrap();
    let recipes = [recipe()];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 2,
        screenshots: true,
        video: true,
        logs_on_failure: true,
    };
    let report = plan.execute(&fake, &control, capture, |_| Ok(())).unwrap();
    assert_eq!(fake.media_calls.lock().unwrap().len(), 7);
    assert_eq!(report.devices[0].media.len(), 5);
    assert!(
        report.devices[0].media[..4]
            .iter()
            .all(|item| item.evidence.is_some() && item.error.is_none())
    );
    assert!(
        report.devices[0].media[4]
            .error
            .as_ref()
            .unwrap()
            .starts_with("app_media_unavailable:")
    );
    assert!(
        report.devices[1..]
            .iter()
            .all(|device| device.media.len() == 1 && device.media[0].evidence.is_some())
    );
    assert!(report.devices.iter().all(|device| device.cleanup_confirmed));
}

#[test]
fn reset_retains_recordings_for_both_exact_allocations_after_closure() {
    let fake = Fake::default();
    let control = Control::new(Duration::from_secs(10)).unwrap();
    let recipes = [Recipe {
        version: 1,
        id: "reset".into(),
        platforms: None,
        steps: vec![horizon_app_testing::recipe::Step {
            id: "fresh".into(),
            action: Action::Reset {},
        }],
    }];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 2,
        screenshots: false,
        video: true,
        logs_on_failure: false,
    };
    let report = plan.execute(&fake, &control, capture, |_| Ok(())).unwrap();
    for device in report.devices {
        assert_eq!(device.allocations.len(), 2);
        assert_eq!(device.media.len(), 2);
        assert_ne!(device.allocations[0], device.allocations[1]);
        for (id, media) in device.allocations.iter().zip(&device.media) {
            assert_eq!(*id, media.session);
            assert!(media.evidence.is_some());
        }
        assert_eq!(device.session, device.allocations.last().copied());
    }
    assert_eq!(fake.media_calls.lock().unwrap().len(), 6);
}

#[test]
fn screenshot_steps_capture_once_and_report_capture_failure() {
    for fail_capture in [false, true] {
        let fake = Fake::default();
        let recipes = [Recipe {
            version: 1,
            id: "screenshot".into(),
            platforms: None,
            steps: vec![horizon_app_testing::recipe::Step {
                id: "frame".into(),
                action: Action::Screenshot {},
            }],
        }];
        let plan = Plan {
            targets: targets(),
            recipes: &recipes,
            parallel: 2,
            screenshots: true,
            video: false,
            logs_on_failure: false,
        };
        let control = Control::new(Duration::from_secs(5)).unwrap();
        let report = plan
            .execute(
                &fake,
                &control,
                |session, kind, bytes| {
                    if fail_capture {
                        Err(Error::Unavailable)
                    } else {
                        capture(session, kind, bytes)
                    }
                },
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(fake.actions.load(Ordering::SeqCst), 0);
        assert_eq!(fake.screenshots.load(Ordering::SeqCst), 3);
        assert!(
            report
                .devices
                .iter()
                .all(|device| device.cleanup_confirmed && device.steps[0].passed != fail_capture)
        );
    }
}

#[test]
// The actor fixture uses Unix process guardians and filesystem permissions.
#[cfg(unix)]
fn long_reset_run_retains_every_video_and_failure_log_through_finalization() {
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let mut recipe = String::from("```yaml\ndevice-recipe:\n  version: 1\n  id: long-reset\n  steps:\n");
    for index in 0..35 {
        writeln!(recipe, "    - id: reset-{index}\n      action: reset").unwrap();
    }
    recipe.push_str("    - id: late-failure\n      action: assert\n      target: {by: identifier, value: menu.open}\n      state: hidden\n```\n");
    std::fs::write(fixture.root.path().join("recipe.md"), recipe).unwrap();
    let report = run(
        &actor,
        &Control::new(Duration::from_secs(120)).unwrap(),
        capture,
        |_| Ok(()),
    )
    .unwrap();
    let device = &report.devices[0];
    assert_eq!(device.allocations.len(), 36);
    assert_eq!(device.steps.len(), 36);
    assert!(device.steps[..35].iter().all(|step| step.passed));
    assert!(!device.steps[35].passed);
    for allocation in &device.allocations {
        let media: Vec<_> = device.media.iter().filter(|item| item.session == *allocation).collect();
        assert_eq!(media.len(), 5);
        assert!(
            media.iter().all(|item| item.evidence.is_some() && item.error.is_none()),
            "{:?}",
            media.iter().map(|item| &item.error).collect::<Vec<_>>()
        );
    }
    assert!(device.cleanup_confirmed);
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn evidence_preflight_counts_platforms_explicit_captures_and_reset_media() {
    use horizon_app_testing::contract::{Evidence as Policy, MatrixEntry};
    let matrix: Vec<_> = targets()
        .iter()
        .map(|target| MatrixEntry {
            platform: target.device.platform,
            form: target.device.form,
            os: "latest".into(),
            device: None,
        })
        .collect();
    let mut recipe = recipe();
    recipe.platforms = Some(vec![Platform::Ios]);
    recipe.steps = (0..512)
        .map(|index| horizon_app_testing::recipe::Step {
            id: format!("capture-{index}"),
            action: Action::Screenshot {},
        })
        .collect();
    let mut policy = Policy {
        screenshots: false,
        video: false,
        logs_on_failure: false,
    };
    assert!(validate_evidence(&[recipe.clone()], &matrix, &policy).is_ok());
    policy.video = true;
    assert!(matches!(
        validate_evidence(&[recipe.clone()], &matrix, &policy),
        Err(Error::EvidenceFull)
    ));
    policy.video = false;
    recipe.steps.iter_mut().for_each(|step| step.action = Action::Reset {});
    assert!(validate_evidence(&[recipe.clone()], &matrix, &policy).is_ok());
    policy.logs_on_failure = true;
    assert!(matches!(
        validate_evidence(&[recipe], &matrix, &policy),
        Err(Error::EvidenceFull)
    ));
}

#[test]
#[cfg(unix)]
fn oversized_evidence_run_is_rejected_before_any_resource_operation() {
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    let mut recipe = String::from("```yaml\ndevice-recipe:\n  version: 1\n  id: too-much-evidence\n  steps:\n");
    for index in 0..200 {
        writeln!(recipe, "    - id: reset-{index}\n      action: reset").unwrap();
    }
    recipe.push_str("```\n");
    std::fs::write(fixture.root.path().join("recipe.md"), recipe).unwrap();
    let result = run(&actor, &Control::new(Duration::from_secs(5)).unwrap(), capture, |_| {
        panic!("must not dispatch")
    });
    assert!(matches!(result, Err(Error::EvidenceFull)));
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .workspace
            .journal()
            .completed(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
}

#[test]
#[cfg(unix)]
fn cleaned_setup_refusal_keeps_the_original_error_and_reports_confirmed_cleanup() {
    let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
    fixture.fake.refuse_driver.store(true, Ordering::SeqCst);
    let report = run(&actor, &Control::new(Duration::from_secs(30)).unwrap(), capture, |_| {
        Ok(())
    })
    .unwrap();
    let device = &report.devices[0];
    assert!(
        device.error.as_deref().unwrap().contains("device_unverified"),
        "{:?} builds {:?}",
        device.error,
        report.builds.iter().map(|build| &build.error).collect::<Vec<_>>()
    );
    assert!(device.cleanup_confirmed);
    assert!(device.allocations.is_empty());
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 1);
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn creation_cleanup_outcomes_are_reported_without_replaying_failed_creation() {
    for confirmed in [false, true] {
        let runtime = Fake {
            fail_create: Some(Box::new(|_| Error::CleanupUncertain)),
            create_cleanup_confirmed: confirmed,
            ..Fake::default()
        };
        let recipes = [recipe()];
        let plan = Plan {
            targets: targets(),
            recipes: &recipes,
            parallel: 2,
            screenshots: false,
            video: false,
            logs_on_failure: false,
        };
        let report = plan
            .execute(
                &runtime,
                &Control::new(Duration::from_secs(10)).unwrap(),
                capture,
                |_| Ok(()),
            )
            .unwrap();
        assert!(
            report
                .devices
                .iter()
                .all(|device| device.cleanup_confirmed == confirmed && device.error.is_some())
        );
        assert_eq!(runtime.failed_creates.load(Ordering::SeqCst), 3);
    }
}

#[test]
fn uncertain_deferred_creation_is_not_retried_or_reported_as_clean() {
    let runtime = Fake {
        fail_create: Some(Box::new(|_| Error::AdmissionDeferred)),
        ..Fake::default()
    };
    let control = Control::new(Duration::from_secs(5)).unwrap();
    let failure = allocate(&runtime, &control, 0, Uuid::new_v4()).unwrap_err();
    assert_eq!(failure.error, Error::AdmissionDeferred);
    assert!(!failure.cleanup_confirmed);
    assert_eq!(runtime.failed_creates.load(Ordering::SeqCst), 1);
}

#[test]
fn reset_cleanup_outcomes_preserve_errors_without_replaying_replacements() {
    for confirmed in [false, true] {
        let runtime = Fake {
            fail_reset_cleanup: Some(confirmed),
            ..Fake::default()
        };
        let mut reset = recipe();
        reset.steps.truncate(1);
        reset.steps[0].action = Action::Reset {};
        let recipes = [reset];
        let plan = Plan {
            targets: targets(),
            recipes: &recipes,
            parallel: 2,
            screenshots: false,
            video: false,
            logs_on_failure: false,
        };
        let report = plan
            .execute(
                &runtime,
                &Control::new(Duration::from_secs(10)).unwrap(),
                capture,
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(runtime.actions.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.active.load(Ordering::SeqCst), 0);
        assert!(report.devices.iter().all(|device| {
            device.cleanup_confirmed == confirmed
                && device.allocations.len() == 1
                && device.steps.len() == 1
                && !device.steps[0].passed
                && device.steps[0]
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("tunnel_start_failed")
        }));
    }
}

#[test]
#[cfg(unix)]
fn reset_report_uses_actual_replacement_cleanup_acknowledgements() {
    for confirmed in [false, true] {
        let (fixture, actor) = crate::actor::tests::actor("http://localhost:{tunnel.port.backend}");
        std::fs::write(fixture.root.path().join("recipe.md"), "```yaml\ndevice-recipe:\n  version: 1\n  id: reset-failure\n  steps:\n    - id: reset\n      action: reset\n```\n").unwrap();
        let report = run(
            &actor,
            &Control::new(Duration::from_secs(30)).unwrap(),
            capture,
            |progress| {
                if progress.phase == "session_created" {
                    fixture.fake.reject_verification.store(true, Ordering::SeqCst);
                    let transport = Arc::clone(&fixture.fake.transport);
                    *fixture.fake.transport.after_create.lock().unwrap() = Some(Box::new(move || {
                        transport.lost_delete.store(!confirmed, Ordering::SeqCst);
                    }));
                }
                Ok(())
            },
        )
        .unwrap();
        let device = &report.devices[0];
        assert_eq!(device.cleanup_confirmed, confirmed);
        assert_eq!(device.allocations.len(), 1);
        assert_eq!(device.steps.len(), 1);
        assert!(!device.steps[0].passed);
        assert!(device.steps[0].error.as_deref().unwrap().contains(if confirmed {
            "app_device_unverified"
        } else {
            "app_resource_cleanup_uncertain"
        }));
        assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 2);
        fixture.fake.transport.lost_delete.store(false, Ordering::SeqCst);
        actor.shutdown().unwrap();
        assert!(
            fixture
                .workspace
                .journal()
                .pending(fixture.workspace.owner())
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
#[cfg(unix)]
fn fixed_service_matrix_serializes_and_completes_every_declared_device() {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let (fixture, actor) = crate::actor::tests::actor_with_fixed_backend(listener.local_addr().unwrap().port());
    let report = run(&actor, &Control::new(Duration::from_secs(30)).unwrap(), capture, |_| {
        Ok(())
    })
    .unwrap();
    assert_eq!(report.parallel, 1);
    assert_eq!(report.devices.len(), 2);
    assert!(report.builds.iter().all(|build| build.error.is_none()));
    assert!(report.devices.iter().all(|device| device.error.is_none()
        && device.cleanup_confirmed
        && device.steps.iter().all(|step| step.passed)));
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 2);
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reset_publishes_the_owned_replacement_before_capture_and_callback_failure_closes_it() {
    for fail_progress in [false, true] {
        let fake = Fake::default();
        let control = Control::new(Duration::from_secs(10)).unwrap();
        let recipes = [Recipe {
            version: 1,
            id: "reset-view".into(),
            platforms: None,
            steps: vec![horizon_app_testing::recipe::Step {
                id: "fresh".into(),
                action: Action::Reset {},
            }],
        }];
        let plan = Plan {
            targets: targets().into_iter().take(1).collect(),
            recipes: &recipes,
            parallel: 1,
            screenshots: true,
            video: false,
            logs_on_failure: false,
        };
        let published = Mutex::new(Vec::new());
        let report = plan
            .execute(
                &fake,
                &control,
                |session, _, _| {
                    assert_eq!(published.lock().unwrap().last(), Some(&session));
                    capture(session, CaptureKind::Screenshot, b"synthetic-private-png")
                },
                |event| {
                    if event.phase == "session_created" {
                        let session = event.session.unwrap();
                        assert!(fake.sessions.lock().unwrap().contains_key(&session));
                        let mut events = published.lock().unwrap();
                        events.push(session);
                        if fail_progress && events.len() == 2 {
                            return Err(Error::Unavailable);
                        }
                    }
                    Ok(())
                },
            )
            .unwrap();
        let published = published.into_inner().unwrap();
        assert_eq!(published.len(), 2);
        assert_ne!(published[0], published[1]);
        let device = &report.devices[0];
        assert_eq!(device.allocations, published);
        assert_eq!(device.session, published.last().copied());
        assert_eq!(device.steps[0].passed, !fail_progress);
        assert!(device.cleanup_confirmed);
        assert!(fake.sessions.lock().unwrap().is_empty());
        assert_eq!(fake.active.load(Ordering::SeqCst), 0);
        assert_eq!(fake.screenshots.load(Ordering::SeqCst), usize::from(!fail_progress));
    }
}

#[test]
fn recipe_completion_follows_steps_with_lane_and_reset_session_before_cleanup() {
    let fake = Fake::default();
    let events = Mutex::new(Vec::new());
    let mut reset = recipe();
    reset.id = "reset".into();
    reset.steps[0].action = Action::Reset {};
    let recipes = [reset, recipe()];
    let plan = Plan {
        targets: targets(),
        recipes: &recipes,
        parallel: 2,
        screenshots: false,
        video: false,
        logs_on_failure: false,
    };
    let report = plan
        .execute(
            &fake,
            &Control::new(Duration::from_secs(10)).unwrap(),
            capture,
            |event| {
                if matches!(event.phase, "recipe_passed" | "recipe_failed") {
                    assert!(fake.sessions.lock().unwrap().contains_key(&event.session.unwrap()));
                }
                events.lock().unwrap().push(event);
                Ok(())
            },
        )
        .unwrap();
    let events = events.into_inner().unwrap();
    for device in &report.devices {
        let events: Vec<_> = events
            .iter()
            .filter(|e| e.matrix_index == Some(device.matrix_index))
            .collect();
        assert_eq!(
            events.iter().map(|e| e.phase).collect::<Vec<_>>(),
            vec![
                "session_created",
                "step",
                "session_created",
                "recipe_passed",
                "step",
                if device.matrix_index == 0 {
                    "recipe_failed"
                } else {
                    "recipe_passed"
                },
                "lane_complete"
            ]
        );
        assert_eq!(events[2].recipe.as_deref(), Some("reset"));
        assert_eq!(events[2].step.as_deref(), Some("visible"));
        assert_ne!(events[0].session, events[2].session);
        assert_eq!(events.last().unwrap().session, device.session);
        assert_eq!(events[events.len() - 2].recipe.as_deref(), Some("smoke"));
        assert!(events.last().unwrap().step.is_none());
        assert!(events.iter().all(|e| e.run == report.id));
    }
}

#[test]
fn exhausted_evidence_blocks_every_later_step_without_losing_the_live_driver() {
    let fake = Fake::default();
    let mut first = recipe();
    first.id = "first".into();
    first.steps[0].action = Action::Home {};
    first.steps.push(horizon_app_testing::recipe::Step {
        id: "never-run".into(),
        action: Action::Home {},
    });
    let recipes = [first, recipe()];
    let plan = Plan {
        targets: targets().into_iter().skip(1).take(1).collect(),
        recipes: &recipes,
        parallel: 1,
        screenshots: true,
        video: true,
        logs_on_failure: true,
    };
    let events = Mutex::new(Vec::new());
    let failure = Error::HostUnavailable(crate::HostFailure::EvidenceBytes);
    let report = plan
        .execute(
            &fake,
            &Control::new(Duration::from_secs(10)).unwrap(),
            |session, _, _| {
                assert!(fake.sessions.lock().unwrap().contains_key(&session));
                Err(failure)
            },
            |event| {
                events.lock().unwrap().push(event);
                Ok(())
            },
        )
        .unwrap();
    let device = &report.devices[0];
    assert_eq!(fake.actions.load(Ordering::SeqCst), 1);
    assert_eq!(fake.screenshots.load(Ordering::SeqCst), 1);
    assert_eq!(device.error.as_deref(), Some(failure.to_string().as_str()));
    assert!(device.blocked && device.cleanup_confirmed);
    assert!(!device.steps[0].blocked && !device.steps[0].passed);
    assert_eq!(device.steps[0].error.as_deref(), device.error.as_deref());
    assert!(
        device.steps[1..]
            .iter()
            .all(|step| step.blocked && !step.passed && step.error.is_none() && step.screenshot.is_none())
    );
    assert!(fake.sessions.lock().unwrap().is_empty());
    assert!(fake.media_calls.lock().unwrap().is_empty());
    assert_eq!(device.media.len(), 5);
    assert!(
        device
            .media
            .iter()
            .all(|media| media.error.as_deref() == device.error.as_deref())
    );
    let events = events.into_inner().unwrap();
    let failures: Vec<_> = events.iter().filter(|event| event.phase == "lane_blocked").collect();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].error.as_deref(), device.error.as_deref());
    assert!(serde_json::to_string(failures[0]).unwrap().contains("1 GiB"));
    assert_eq!(events.iter().filter(|event| event.phase == "step").count(), 1);
    assert_eq!(events.iter().filter(|event| event.phase == "recipe_blocked").count(), 1);
}

#[test]
fn forced_host_loss_blocks_later_recipes_and_retains_the_initial_cause() {
    let failure = Error::Native(horizon_app_testing::Error::SessionClosed);
    let fake = Fake {
        fail_action: Some(failure),
        fail_close: true,
        ..Fake::default()
    };
    let mut first = recipe();
    first.steps[0].action = Action::Home {};
    let recipes = [first, recipe()];
    let plan = Plan {
        targets: targets().into_iter().skip(1).take(1).collect(),
        recipes: &recipes,
        parallel: 1,
        screenshots: false,
        video: true,
        logs_on_failure: true,
    };
    let report = plan
        .execute(&fake, &Control::new(Duration::from_secs(10)).unwrap(), capture, |_| {
            Ok(())
        })
        .unwrap();
    assert_eq!(fake.actions.load(Ordering::SeqCst), 1);
    assert!(report.devices[0].steps[1].blocked);
    assert_eq!(report.devices[0].error.as_deref(), Some(failure.to_string().as_str()));
    assert!(!report.devices[0].cleanup_confirmed);
    assert_eq!(fake.media_calls.lock().unwrap().len(), 5);
    assert_eq!(report.devices[0].media.len(), 5);
}

#[test]
fn failed_progress_sink_does_not_omit_blocked_recipes_from_the_terminal_report() {
    let fake = Fake::default();
    let recipes = [recipe(), recipe(), recipe()];
    let plan = Plan {
        targets: targets().into_iter().skip(1).take(1).collect(),
        recipes: &recipes,
        parallel: 1,
        screenshots: true,
        video: false,
        logs_on_failure: false,
    };
    let report = plan
        .execute(
            &fake,
            &Control::new(Duration::from_secs(10)).unwrap(),
            capture,
            |event| {
                if matches!(event.phase, "build" | "session_created") {
                    Ok(())
                } else {
                    Err(Error::Unavailable)
                }
            },
        )
        .unwrap();
    assert_eq!(fake.actions.load(Ordering::SeqCst), 0);
    assert_eq!(report.devices[0].steps.len(), 3);
    assert!(!report.devices[0].steps[0].blocked);
    assert!(report.devices[0].steps[1..].iter().all(|step| step.blocked));
    assert!(report.devices[0].cleanup_confirmed);
}
