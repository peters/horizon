use super::*;
use crate::{
    HorizonHome, PanelKind,
    cloud_run::{CloudProvider, WorkerLifetime, interactive_worker::*},
    remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState},
    remote_workspace_recovery::recover_remote_workspace,
};
use std::{
    cell::Cell,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const RUNNING: &str = r#"{"state":"running","panel":"shell","pid":12,"exit_status":null}"#;

struct Provider {
    status: InteractiveWorkerStatus,
    calls: AtomicUsize,
}
impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::LocalDocker
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        Ok(Some(self.status.clone()))
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(self.status.clone()))
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
    allocation: StoredRemoteAllocation,
    provider: Provider,
}
impl Fixture {
    fn new(branch: Option<&str>, lifetime: WorkerLifetime) -> Self {
        Self::with_pin(branch, lifetime, true)
    }

    fn with_pin(branch: Option<&str>, lifetime: WorkerLifetime, pinned: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open(&home).unwrap();
        let identities = RemoteSshIdentityStore::new(&home);
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1,"spec":{"workspace_local_id":"workspace","working_directory":"nested","generation":0,
              "panels":[{"panel_local_id":"shell","kind":"shell","command":{"program":"/bin/sh","args":["-c","printf synthetic"]}}],
              "repository":{"repository":"fixture/repository","commit":"a".repeat(40),"branch":branch},
              "target":{"provider":"local_docker","profile":"development","image":format!("fixture/worker@sha256:{}","b".repeat(64)),
                "disk_gib":20,"lifetime":"persistent"}}})).unwrap();
        state.spec.target.lifetime = lifetime;
        let workspace = store.create_remote_workspace(OWNER, &state).unwrap();
        let allocation = store.allocate_remote_runtime(&workspace, i64::MAX).unwrap();
        let runtime = allocation.workspace().state().runtime.as_ref().unwrap();
        let identity = identities.prepare_new(runtime.workflow_id, runtime.job_id).unwrap();
        let allocation = store
            .reserve_remote_worker_request(&allocation, identity.public_key())
            .unwrap();
        let request = allocation.worker_request().unwrap();
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: request.target.provider,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: "synthetic-worker".into(),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: match lifetime {
                    WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
                    WorkerLifetime::TimeLimited { seconds } => {
                        InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                            terminate_after: (OffsetDateTime::now_utc() + time::Duration::seconds(i64::from(seconds)))
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap(),
                        })
                    }
                },
            },
            lifecycle: if pinned {
                InteractiveWorkerLifecycle::Ready
            } else {
                InteractiveWorkerLifecycle::Provisioning
            },
            ssh: pinned.then(|| InteractiveWorkerSshEndpoint {
                host: "127.0.0.1".into(),
                port: 2222,
                username: "horizon".into(),
                host_key: identity.public_key().into(),
            }),
        };
        let provider = Provider {
            status,
            calls: AtomicUsize::new(0),
        };
        let recovered = recover_remote_workspace(&store, &identities, &provider, OWNER, "workspace").unwrap();
        Self {
            _directory: directory,
            store,
            identities,
            allocation: recovered.allocation().clone(),
            provider,
        }
    }
    fn ready() -> Self {
        Self::new(Some("work/one"), WorkerLifetime::Persistent)
    }
    fn edit(&mut self, change: impl FnOnce(&mut RemoteWorkspaceState)) {
        let mut state = self.allocation.workspace().state().clone();
        change(&mut state);
        self.store
            .replace_remote_workspace(self.allocation.workspace(), &state)
            .unwrap();
        self.allocation = self.store.load_remote_allocation(OWNER, "workspace").unwrap().unwrap();
    }
    fn start(
        &self,
        execute: impl FnOnce(
            &CloudWorkflowStore,
            &RecoveredRemoteWorkspace,
            &str,
            &[u8],
        ) -> Result<RemotePanelStatus, RemoteGitTaskStartError>,
    ) -> Result<RemotePanelStatus, RemoteGitTaskStartError> {
        start_with(
            &self.store,
            &self.identities,
            &self.provider,
            &self.allocation,
            "shell",
            execute,
        )
    }
}

#[test]
fn exact_saved_shell_and_git_binding_are_sent_without_persisting_new_state() {
    let fixture = Fixture::ready();
    let expected = fixture.allocation.clone();
    let result = fixture
        .start(|_, recovered, panel, bytes| {
            assert_eq!(recovered.allocation(), &expected);
            assert_eq!(panel, "shell");
            let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(value["operation"], "start-git");
            assert_eq!(value["directory"], "nested");
            assert_eq!(value["argv"], serde_json::json!(["/bin/sh", "-c", "printf synthetic"]));
            let git: crate::repository_git::GitPreparation =
                serde_json::from_value(value["repository"].clone()).unwrap();
            assert_eq!(git.source, expected.workspace().state().spec.repository);
            assert_eq!(git.work_branch, "work/one");
            assert_eq!(
                git.runtime_id.to_string(),
                expected.worker_request().unwrap().job_id.to_string()
            );
            Ok(RemotePanelStatus::Running { pid: 12 })
        })
        .unwrap();
    assert_eq!(result, RemotePanelStatus::Running { pid: 12 });
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .store
            .load_remote_allocation(OWNER, "workspace")
            .unwrap()
            .unwrap(),
        expected
    );
}

#[test]
fn unsupported_saved_intents_are_rejected_before_provider_or_ssh() {
    for fault in ["kind", "command", "handoff", "args"] {
        let mut fixture = Fixture::ready();
        fixture.edit(|state| {
            let task = &mut state.spec.panels[0];
            match fault {
                "kind" => task.kind = PanelKind::Command,
                "command" => task.command = None,
                "handoff" => task.task_handoff = Some("synthetic context".into()),
                // Persisted argv permits 64KiB of arguments; the worker also
                // counts the executable, so this valid saved intent is too large.
                _ => task.command.as_mut().unwrap().args = vec!["a".repeat(65536)],
            }
        });
        assert_eq!(
            fixture.start(|_, _, _, _| panic!("no SSH")),
            Err(RemoteGitTaskStartError::InvalidIntent),
            "{fault}"
        );
        assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
    }
    for branch in [None, Some("HEAD")] {
        let fixture = Fixture::new(branch, WorkerLifetime::Persistent);
        assert_eq!(
            fixture.start(|_, _, _, _| panic!("no SSH")),
            Err(RemoteGitTaskStartError::InvalidIntent)
        );
        assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
    }
    let fixture = Fixture::ready();
    assert_eq!(
        start_with(
            &fixture.store,
            &fixture.identities,
            &fixture.provider,
            &fixture.allocation,
            "unknown-panel",
            |_, _, _, _| panic!("no SSH")
        ),
        Err(RemoteGitTaskStartError::InvalidIntent)
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn changed_snapshot_and_pending_management_refuse_before_provider() {
    for stop in [false, true] {
        let mut fixture = Fixture::ready();
        let stale = fixture.allocation.clone();
        if stop {
            fixture
                .store
                .record_remote_stop_phase(
                    &fixture.allocation,
                    RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
                )
                .unwrap();
        } else {
            fixture.edit(|state| state.spec.working_directory = ".".into());
        }
        assert!(
            start_with(
                &fixture.store,
                &fixture.identities,
                &fixture.provider,
                &stale,
                "shell",
                |_, _, _, _| panic!("no SSH")
            )
            .is_err()
        );
        assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn changed_valid_host_key_or_missing_worker_never_sends_start() {
    let mut fixture = Fixture::ready();
    let another = Fixture::ready();
    fixture.provider.status.ssh.as_mut().unwrap().host_key =
        another.provider.status.ssh.as_ref().unwrap().host_key.clone();
    assert!(fixture.start(|_, _, _, _| panic!("no SSH")).is_err());
    let fixture = Fixture::with_pin(Some("work/one"), WorkerLifetime::Persistent, false);
    assert_eq!(
        fixture.start(|_, _, _, _| panic!("no SSH")),
        Err(RemoteGitTaskStartError::MissingRetainedWorker)
    );
    assert_eq!(fixture.provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn post_send_intent_drift_is_unknown_even_with_a_valid_task_response() {
    let fixture = Fixture::ready();
    assert_eq!(
        fixture.start(|store, recovered, _, _| {
            store
                .record_remote_stop_phase(
                    recovered.allocation(),
                    RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
                )
                .unwrap();
            Ok(RemotePanelStatus::Running { pid: 12 })
        }),
        Err(RemoteGitTaskStartError::OutcomeUnknown)
    );
}

#[test]
fn short_lease_is_enforced_without_shared_clock_skew_grace() {
    let fixture = Fixture::new(Some("work/one"), WorkerLifetime::TimeLimited { seconds: 1 });
    let result = fixture.start(|_, _, _, _| {
        std::thread::sleep(Duration::from_millis(1100));
        Ok(RemotePanelStatus::Running { pid: 12 })
    });
    assert_eq!(result, Err(RemoteGitTaskStartError::OutcomeUnknown));
    assert!(
        fixture
            .start(|_, _, _, _| panic!("expired request must not send"))
            .is_err()
    );
}

fn child(script: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.env_clear().arg("-c").arg(script);
    command
}

#[test]
fn transport_accepts_only_exact_complete_successful_matching_status() {
    for response in [
        RUNNING,
        r#"{"state":"exited","panel":"shell","pid":12,"exit_status":42}"#,
        r#"{"state":"unavailable","panel":"shell"}"#,
    ] {
        let script = format!("/bin/cat >/dev/null; printf '%s' '{response}'");
        assert!(exchange(child(&script), b"request", "shell", None, OffsetDateTime::now_utc).is_ok());
    }
    for response in [
        "{}",
        r#"{"state":"running","panel":"other","pid":12,"exit_status":null}"#,
        r#"{"state":"running","panel":"shell","pid":12}"#,
        r#"{"state":"running","panel":"shell","pid":12,"exit_status":null,"private":"sentinel"}"#,
    ] {
        let script = format!("/bin/cat >/dev/null; printf '%s' '{response}'");
        assert_eq!(
            exchange(child(&script), b"request", "shell", None, OffsetDateTime::now_utc),
            Err(RemoteGitTaskStartError::OutcomeUnknown)
        );
    }
    for script in [
        format!("/bin/cat >/dev/null; printf '%s' '{RUNNING}'; exit 1"),
        format!("exec 0<&-; printf '%s' '{RUNNING}'"),
        "/bin/cat >/dev/null".into(),
    ] {
        assert_eq!(
            exchange(
                child(&script),
                &vec![b'x'; 256 * 1024],
                "shell",
                None,
                OffsetDateTime::now_utc
            ),
            Err(RemoteGitTaskStartError::OutcomeUnknown)
        );
    }
}

#[test]
fn absolute_deadline_covers_spawn_and_checks_before_first_stdin_write() {
    let now = OffsetDateTime::now_utc();
    assert_eq!(timeout(None, now), Ok(Duration::from_secs(15)));
    assert_eq!(
        timeout(Some(now + time::Duration::seconds(2)), now),
        Ok(Duration::from_secs(2))
    );
    assert_eq!(timeout(Some(now), now), Err(RemoteGitTaskStartError::ExpiredWorker));
    let temp = tempfile::tempdir().unwrap();
    let received = temp.path().join("received");
    let mut command = child("/bin/cat > \"$1\"");
    command.arg("fixture").arg(&received);
    let calls = Cell::new(0);
    assert_eq!(
        exchange(
            command,
            b"request must not arrive",
            "shell",
            Some(now + time::Duration::seconds(1)),
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    now
                } else {
                    now + time::Duration::seconds(2)
                }
            }
        ),
        Err(RemoteGitTaskStartError::OutcomeUnknown)
    );
    assert!(!received.exists() || std::fs::read(received).unwrap().is_empty());
}
