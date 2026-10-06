use super::*;
use horizon_app_testing::catalog::Device;
use horizon_app_testing::contract::Form;
use horizon_app_testing::recipe::{State, Target};
use std::fmt::Write as _;
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
    fn create(&self, index: usize, _app: Uuid, deadline: Instant) -> Result<Uuid> {
        if let Some(failure) = &self.fail_create {
            self.failed_creates.fetch_add(1, Ordering::SeqCst);
            return Err(failure(deadline));
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
    fn act(&self, session: Uuid, action: &Action) -> Result<Option<Uuid>> {
        self.actions.fetch_add(1, Ordering::SeqCst);
        if matches!(action, Action::Reset {}) {
            let mut sessions = self.sessions.lock().unwrap();
            let index = sessions.remove(&session).unwrap();
            let replacement = Uuid::new_v4();
            sessions.insert(replacement, index);
            return Ok(Some(replacement));
        }
        if self.sessions.lock().unwrap()[&session] == 0 {
            return Err(horizon_app_testing::Error::AssertionFailed.into());
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
        Ok(())
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
    assert_eq!(allocate(&actor, &control, 0, app.id), Err(Error::Cancelled));
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
            ..Fake::default()
        };
        let error = allocate(&runtime, &control, 0, Uuid::new_v4()).unwrap_err();
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
        allocate(&runtime, &control, 0, Uuid::new_v4()),
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
