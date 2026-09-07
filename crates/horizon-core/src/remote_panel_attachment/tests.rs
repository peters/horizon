use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, WorkerLifetime, interactive_worker::*},
    remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState},
    terminal::TerminalSpawnOptions,
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
    provider: Provider,
}

impl Fixture {
    fn new() -> Self {
        Self::with_lifetime(WorkerLifetime::Persistent)
    }

    fn with_lifetime(lifetime: WorkerLifetime) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).expect("store");
        let identities = RemoteSshIdentityStore::new(&home);
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0,
                "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                    "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)},
                "panels":[{"panel_local_id":"terminal", "kind":"command",
                    "command":{"program":"printf", "args":["synthetic task"]}}]
            }
        }))
        .expect("state");
        state.spec.target.lifetime = lifetime;
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
        let identity = identities
            .prepare_new(runtime.workflow_id, runtime.job_id)
            .expect("identity");
        let reserved = store
            .reserve_remote_worker_request(&allocation, identity.public_key())
            .expect("request");
        let request = reserved.worker_request().expect("request");
        let observed_lifetime = match lifetime {
            WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
            WorkerLifetime::TimeLimited { seconds } => InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                terminate_after: (time::OffsetDateTime::now_utc() + time::Duration::seconds(i64::from(seconds)))
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("deadline"),
            }),
        };
        let provider = Provider {
            status: Some(InteractiveWorkerStatus {
                worker: InteractiveWorker {
                    identity: InteractiveWorkerIdentity {
                        provider: request.target.provider,
                        workflow_id: request.workflow_id,
                        job_id: request.job_id,
                        resource_id: "synthetic-worker".into(),
                    },
                    target: request.target,
                    ssh_public_key: request.ssh_public_key,
                    lifetime: observed_lifetime,
                },
                lifecycle: InteractiveWorkerLifecycle::Ready,
                ssh: Some(InteractiveWorkerSshEndpoint {
                    host: "127.0.0.1".into(),
                    port: 2222,
                    username: "horizon".into(),
                    host_key: identity.public_key().into(),
                }),
            }),
            calls: Mutex::new(0),
            on_inspect: None,
        };
        store
            .record_remote_worker_recovery(&reserved, provider.status.as_ref())
            .expect("recovery");
        *provider.calls.lock().expect("calls") = 0;
        Self {
            _directory: directory,
            store,
            identities,
            provider,
        }
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn request(allocation: &StoredRemoteAllocation) -> RemotePanelAttachRequest<'_> {
        RemotePanelAttachRequest {
            allocation,
            panel_id: "terminal",
            terminal: terminal_size(),
        }
    }
}

struct Provider {
    status: Option<InteractiveWorkerStatus>,
    calls: Mutex<usize>,
    on_inspect: Option<Box<dyn Fn() + Send + Sync>>,
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::LocalDocker
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no creation")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no deletion")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("retained fixture must use its exact worker")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        *self.calls.lock().expect("calls") += 1;
        if let Some(action) = &self.on_inspect {
            action();
        }
        Ok(self.status.clone())
    }
}

fn terminal_size() -> RemotePanelTerminalSize {
    RemotePanelTerminalSize {
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 16,
        scrollback_limit: 128,
        window_id: 1,
        kitty_keyboard: false,
    }
}

fn local_options(script: &str) -> TerminalSpawnOptions {
    let size = terminal_size();
    TerminalSpawnOptions {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: None,
        rows: size.rows,
        cols: size.cols,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
        scrollback_limit: size.scrollback_limit,
        window_id: size.window_id,
        kitty_keyboard: size.kitty_keyboard,
        replay_bytes: vec![],
        env: std::collections::HashMap::new(),
    }
}

fn invalidate(store: &CloudWorkflowStore) {
    let current = store
        .load_remote_allocation(OWNER, "workspace")
        .expect("load")
        .expect("allocation");
    let mut next = current.workspace().state().clone();
    next.spec.working_directory = "changed".into();
    store
        .replace_remote_workspace(current.workspace(), &next)
        .expect("edit");
}

fn running() -> RemotePanelStatus {
    RemotePanelStatus::Running { pid: 123 }
}

#[test]
fn fresh_running_and_exited_attempts_preserve_snapshots_and_never_promote_readiness() {
    for (status, phase) in [
        running(),
        RemotePanelStatus::Exited {
            pid: 123,
            exit_status: Some(7),
        },
    ]
    .into_iter()
    .flat_map(|status| {
        [RemoteRuntimePhase::Reconciling, RemoteRuntimePhase::Ready].map(|phase| (status.clone(), phase))
    }) {
        let fixture = Fixture::new();
        let original = fixture.current();
        let mut state = original.workspace().state().clone();
        state.runtime.as_mut().expect("runtime").phase = phase;
        fixture
            .store
            .replace_remote_workspace(original.workspace(), &state)
            .expect("phase");
        let before = fixture.current();
        let attempt = attach_with(
            &fixture.store,
            &fixture.identities,
            &fixture.provider,
            Fixture::request(&before),
            |_, recovered, panel| {
                assert_eq!(recovered.allocation(), &before);
                assert_eq!(panel, "terminal");
                Ok(status.clone())
            },
            |_, _, _, _| Ok(Terminal::spawn(local_options("exit 0")).expect("local fixture")),
        )
        .expect("attempt");
        assert_eq!(attempt.allocation(), &before);
        assert_eq!(attempt.panel_id(), "terminal");
        assert_eq!(attempt.observed_status(), &status);
        assert!(!format!("{attempt:?}").contains("synthetic"));
        assert_eq!(fixture.current(), before);
        let mut terminal = attempt.into_terminal(&fixture.store).expect("current handoff");
        assert!(terminal.shutdown_with_timeout(Duration::from_secs(2)));
        assert_eq!(fixture.current(), before);
        assert_eq!(*fixture.provider.calls.lock().expect("calls"), 1);
    }
}

#[test]
fn stale_missing_panel_timed_and_pending_management_never_inspect_or_spawn() {
    for case in 0..4 {
        let fixture = if case == 2 {
            Fixture::with_lifetime(WorkerLifetime::TimeLimited { seconds: 900 })
        } else {
            Fixture::new()
        };
        let original = fixture.current();
        if case == 0 {
            invalidate(&fixture.store);
        }
        if case == 3 {
            fixture
                .store
                .record_remote_stop_phase(&original, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
                .expect("stop intent");
        }
        let expected = if case == 0 { original } else { fixture.current() };
        let mut request = Fixture::request(&expected);
        if case == 1 {
            request.panel_id = "absent;start";
        }
        assert!(
            attach_with(
                &fixture.store,
                &fixture.identities,
                &fixture.provider,
                request,
                |_, _, _| panic!("no query"),
                |_, _, _, _| panic!("no spawn")
            )
            .is_err()
        );
        assert_eq!(*fixture.provider.calls.lock().expect("calls"), 0);
    }
}

#[test]
fn missing_key_absent_worker_and_unsupported_intent_never_spawn_or_replace() {
    let mut fixture = Fixture::new();
    let before = fixture.current();
    fixture.provider.status = None;
    let result = attach_with(
        &fixture.store,
        &fixture.identities,
        &fixture.provider,
        Fixture::request(&before),
        |_, recovered, _| {
            assert!(recovered.observation().is_none());
            Ok(RemotePanelStatus::Unavailable)
        },
        |_, _, _, _| panic!("no spawn"),
    );
    assert_eq!(
        result.expect_err("missing task"),
        RemotePanelAttachError::TaskUnavailable
    );
    assert_eq!(fixture.current(), before);
    assert_eq!(
        attach_with(
            &fixture.store,
            &fixture.identities,
            &fixture.provider,
            Fixture::request(&before),
            |_, _, _| Err(RemotePanelStatusError::UnsupportedIntent),
            |_, _, _, _| panic!("no spawn")
        )
        .expect_err("intent"),
        RemotePanelAttachError::Inspection(RemotePanelStatusError::UnsupportedIntent)
    );
    let request = before.worker_request().expect("request");
    let identity = fixture
        .identities
        .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
        .expect("key");
    std::fs::remove_file(identity.private_key_path()).expect("simulate loss of fixture key");
    assert!(
        attach_with(
            &fixture.store,
            &fixture.identities,
            &fixture.provider,
            Fixture::request(&before),
            |_, _, _| panic!("no query"),
            |_, _, _, _| panic!("no spawn")
        )
        .is_err()
    );
    assert!(!identity.private_key_path().exists());
    assert_eq!(fixture.current(), before);
}

#[test]
fn public_admission_rejects_absent_nonready_and_foreign_allocations_without_ssh() {
    for lifecycle in [
        None,
        Some(InteractiveWorkerLifecycle::Stopped),
        Some(InteractiveWorkerLifecycle::Unknown),
    ] {
        let mut fixture = Fixture::new();
        let before = fixture.current();
        if let Some(lifecycle) = lifecycle {
            fixture.provider.status.as_mut().expect("worker").lifecycle = lifecycle;
        } else {
            fixture.provider.status = None;
        }
        assert_eq!(
            attach_remote_panel(
                &fixture.store,
                &fixture.identities,
                &fixture.provider,
                Fixture::request(&before)
            )
            .expect_err("not ready"),
            RemotePanelAttachError::Inspection(RemotePanelStatusError::WorkerUnavailable)
        );
        assert_eq!(fixture.current(), before);
    }
    let fixture = Fixture::new();
    let foreign = Fixture::new();
    assert_eq!(
        attach_remote_panel(
            &fixture.store,
            &fixture.identities,
            &fixture.provider,
            Fixture::request(&foreign.current())
        )
        .expect_err("foreign"),
        RemotePanelAttachError::StateChanged
    );
    assert_eq!(*fixture.provider.calls.lock().expect("calls"), 0);
}

#[test]
fn provider_and_query_drift_cannot_authorize_a_later_local_launch() {
    for provider_drift in [true, false] {
        let mut fixture = Fixture::new();
        let before = fixture.current();
        if provider_drift {
            let store = fixture.store.clone();
            fixture.provider.on_inspect = Some(Box::new(move || invalidate(&store)));
        }
        assert!(
            attach_with(
                &fixture.store,
                &fixture.identities,
                &fixture.provider,
                Fixture::request(&before),
                |store, _, _| {
                    assert!(!provider_drift);
                    invalidate(store);
                    Ok(running())
                },
                |_, _, _, _| panic!("no spawn")
            )
            .is_err()
        );
        assert_eq!(
            fixture.current().workspace().revision(),
            before.workspace().revision() + 1
        );
    }
}

#[test]
fn drift_after_local_start_or_before_consumption_discards_the_attempt() {
    for drift_at_spawn in [true, false] {
        let fixture = Fixture::new();
        let before = fixture.current();
        let result = attach_with(
            &fixture.store,
            &fixture.identities,
            &fixture.provider,
            Fixture::request(&before),
            |_, _, _| Ok(running()),
            |store, _, _, _| {
                let terminal = Terminal::spawn(local_options("exit 0")).expect("local fixture");
                if drift_at_spawn {
                    invalidate(store);
                }
                Ok(terminal)
            },
        );
        if drift_at_spawn {
            assert_eq!(result.expect_err("late start"), RemotePanelAttachError::StateChanged);
        } else {
            let attempt = result.expect("attempt");
            invalidate(&fixture.store);
            assert!(matches!(
                attempt.into_terminal(&fixture.store),
                Err(RemotePanelAttachError::StateChanged)
            ));
        }
    }
}

fn await_removed(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!path.exists(), "owned trust must be released after local teardown");
}

#[test]
fn private_trust_survives_immediate_drop_timeout_and_async_join_until_owned_teardown() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for mode in 0..3 {
        let directory = tempfile::tempdir().expect("fixture");
        let trust = tempfile::NamedTempFile::new_in(directory.path()).expect("trust");
        let path = trust.path().to_path_buf();
        let release = directory.path().join("release");
        let mut options =
            local_options("i=0; while [ ! -f \"$1\" ] && [ \"$i\" -lt 200 ]; do i=$((i+1)); sleep 0.01; done");
        options
            .args
            .extend(["fixture".into(), release.to_string_lossy().into_owned()]);
        let mut terminal = Terminal::spawn_with_ssh_trust(options, Arc::new(trust)).expect("guarded local fixture");
        let completed = Arc::new(AtomicUsize::new(0));
        if mode == 1 {
            assert!(!terminal.wait_for_shutdown(Duration::ZERO));
        }
        if mode == 2 {
            assert!(terminal.begin_async_join(&completed));
        }
        drop(terminal);
        if mode != 0 {
            assert!(path.exists());
            assert_eq!(completed.load(Ordering::Relaxed), 0);
            std::fs::write(&release, b"release").expect("release owned child");
        }
        await_removed(&path);
        if mode == 2 {
            let deadline = Instant::now() + Duration::from_secs(2);
            while completed.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
                std::thread::yield_now();
            }
            assert_eq!(completed.load(Ordering::Relaxed), 1);
        }
    }
}

#[test]
fn failed_startup_and_concurrent_connections_release_only_their_own_trust() {
    let directory = tempfile::tempdir().expect("fixture");
    let first = tempfile::NamedTempFile::new_in(directory.path()).expect("first");
    let second = tempfile::NamedTempFile::new_in(directory.path()).expect("second");
    let first_path = first.path().to_path_buf();
    let second_path = second.path().to_path_buf();
    let second_terminal =
        Terminal::spawn_with_ssh_trust(local_options("read -r fixture"), Arc::new(second)).expect("second");
    let mut options = local_options("exit 0");
    options.program = directory.path().join("nonexistent").to_string_lossy().into_owned();
    assert!(Terminal::spawn_with_ssh_trust(options, Arc::new(first)).is_err());
    await_removed(&first_path);
    assert!(second_path.exists());
    drop(second_terminal);
    await_removed(&second_path);
}
