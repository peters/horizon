mod operations;
mod paint;

use super::super::{InventoryPage, paint as inventory_paint};
use super::result::Status;
use super::*;
use horizon_core::{
    cloud_run::{
        CloudJobId, CloudProvider, CloudWorkflowId, WorkerLifetime, interactive_worker::InteractiveWorkerIdentity,
    },
    remote_workspace::RemoteRuntimePhase,
};

struct Fixture {
    _temp: tempfile::TempDir,
    scope: Scope,
}

impl Fixture {
    fn new(provider: CloudProvider) -> Self {
        let temp = tempfile::tempdir().expect("synthetic fixture");
        let workflow_id = CloudWorkflowId::new();
        let job_id = CloudJobId::new();
        let expected = RemoteEnvironmentSummary {
            workspace_local_id: "synthetic-delete".into(),
            owning_session_id: "00000000-0000-4000-8000-000000000001".into(),
            revision: 10,
            repository: "example/project".into(),
            provider,
            profile: "fixture".into(),
            lifetime: WorkerLifetime::Persistent,
            generation: 1,
            saved_phase: Some(RemoteRuntimePhase::Ready),
            workflow_id: Some(workflow_id),
            job_id: Some(job_id),
            worker_identity: Some(InteractiveWorkerIdentity {
                provider,
                workflow_id,
                job_id,
                resource_id: if provider == CloudProvider::Azure {
                    "/subscriptions/00000000-0000-4000-8000-000000000002/resourceGroups/synthetic-owned-group".into()
                } else {
                    "synthetic-pod".into()
                },
            }),
            checkpoint: None,
            panel_count: 1,
        };
        Self {
            scope: Scope {
                home: HorizonHome::from_root(temp.path().join("home")),
                config: RemoteProviderConfig::default(),
                expected,
            },
            _temp: temp,
        }
    }

    fn request(&self, operation: Operation) -> Request {
        Request {
            scope: self.scope.clone(),
            operation,
        }
    }

    fn action(&self, state: &mut DeleteState, action: Action) -> Option<Request> {
        state.request(action, &self.scope.home, &self.scope.config, &self.scope.expected)
    }

    fn pending(
        &self,
        state: &mut DeleteState,
        operation: Operation,
    ) -> mpsc::SyncSender<Result<ConfiguredEnvironmentDeletion, Error>> {
        let (tx, rx) = mpsc::sync_channel(1);
        state.pending = Some(Pending {
            request: self.request(operation),
            rx,
            discard: false,
        });
        tx
    }

    fn result(&self, delta: u64, phase: RemoteRuntimePhase, verified: bool) -> ConfiguredEnvironmentDeletion {
        let mut saved = self.scope.expected.clone();
        saved.revision += delta;
        saved.saved_phase = Some(phase);
        ConfiguredEnvironmentDeletion {
            saved,
            absence_verified: verified,
        }
    }

    fn view(&self) -> RemoteEnvironments {
        let mut other = self.scope.expected.clone();
        other.workspace_local_id = "other-synthetic".into();
        RemoteEnvironments {
            open: true,
            selected: Some(0),
            page: Some(InventoryPage {
                rows: vec![
                    inventory_paint::InventoryRow::new(self.scope.expected.clone()),
                    inventory_paint::InventoryRow::new(other),
                ],
                next_cursor: None,
            }),
            ..Default::default()
        }
    }
}

fn requested() -> RemoteRuntimePhase {
    RemoteRuntimePhase::DeleteRequested {
        requested_at_millis: 20,
    }
}
fn deleted() -> RemoteRuntimePhase {
    RemoteRuntimePhase::Deleted {
        requested_at_millis: 20,
        observed_at_millis: 30,
    }
}

#[test]
fn eligibility_is_phase_and_identity_bound_without_any_ssh_state() {
    for provider in [CloudProvider::RunPod, CloudProvider::Azure, CloudProvider::LocalDocker] {
        let mut fixture = Fixture::new(provider);
        for phase in [
            RemoteRuntimePhase::Ready,
            RemoteRuntimePhase::Reconciling,
            RemoteRuntimePhase::Failed,
            RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
        ] {
            fixture.scope.expected.saved_phase = Some(phase);
            assert_eq!(
                supported(&fixture.scope.expected, Operation::Delete),
                provider != CloudProvider::LocalDocker
            );
            assert!(!supported(&fixture.scope.expected, Operation::Check));
        }
        fixture.scope.expected.saved_phase = Some(requested());
        assert!(!supported(&fixture.scope.expected, Operation::Delete));
        for operation in [Operation::Check, Operation::Retry] {
            assert_eq!(
                supported(&fixture.scope.expected, operation),
                provider != CloudProvider::LocalDocker
            );
        }
        for phase in [
            None,
            Some(deleted()),
            Some(RemoteRuntimePhase::Provisioning),
            Some(RemoteRuntimePhase::Starting { requested_at_millis: 1 }),
            Some(RemoteRuntimePhase::Stopping { requested_at_millis: 1 }),
        ] {
            fixture.scope.expected.saved_phase = phase;
            for operation in [Operation::Delete, Operation::Check, Operation::Retry] {
                assert!(!supported(&fixture.scope.expected, operation));
            }
        }
    }
    for fault in 0..4 {
        let mut fixture = Fixture::new(CloudProvider::RunPod);
        match fault {
            0 => fixture.scope.expected.worker_identity = None,
            1 => fixture.scope.expected.lifetime = WorkerLifetime::TimeLimited { seconds: 60 },
            2 => fixture.scope.expected.workflow_id = None,
            _ => {
                fixture
                    .scope
                    .expected
                    .worker_identity
                    .as_mut()
                    .expect("identity")
                    .provider = CloudProvider::Azure;
            }
        }
        assert!(!supported(&fixture.scope.expected, Operation::Delete));
    }
}

#[test]
fn actual_callback_never_initializes_missing_storage() {
    let fixture = Fixture::new(CloudProvider::RunPod);
    for operation in [Operation::Delete, Operation::Check, Operation::Retry] {
        assert!(matches!(execute(&fixture.scope, operation), Err(Error::Storage)));
        assert!(!fixture.scope.home.root().exists());
    }
}

fn snapshot(path: &std::path::Path) -> (Vec<u8>, i64, String) {
    let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read-only snapshot");
    (
        std::fs::read(path).expect("bytes"),
        db.pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version"),
        db.pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal"),
    )
}

#[test]
fn actual_callbacks_refuse_legacy_and_drift_without_migration_or_journal_changes() {
    for sql in [
        "DROP TABLE remote_provider_bindings; PRAGMA user_version = 6;",
        "DROP INDEX remote_workspaces_session;",
        "CREATE TRIGGER unexpected_delete AFTER UPDATE ON cloud_workflows BEGIN DELETE FROM remote_workspaces; END;",
        "",
    ] {
        let fixture = Fixture::new(CloudProvider::Azure);
        let store = CloudWorkflowStore::open(&fixture.scope.home).expect("synthetic store");
        let db = rusqlite::Connection::open(store.path()).expect("fixture writer");
        db.execute_batch(sql).expect("fixture schema");
        db.pragma_update(None, "journal_mode", "DELETE")
            .expect("commit fixture");
        drop(db);
        let before = snapshot(store.path());
        for operation in [Operation::Delete, Operation::Check, Operation::Retry] {
            let result = execute(&fixture.scope, operation);
            if sql.is_empty() {
                assert!(matches!(result, Err(Error::Core(_))));
            } else {
                assert!(matches!(result, Err(Error::Storage)), "{result:?}");
            }
            assert_eq!(snapshot(store.path()), before);
        }
    }
}

#[cfg(unix)]
#[test]
fn actual_callbacks_refuse_insecure_store_without_changing_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(CloudProvider::Azure);
    let store = CloudWorkflowStore::open(&fixture.scope.home).expect("fixture store");
    std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).expect("insecure fixture");
    let before = std::fs::read(store.path()).expect("before");
    for operation in [Operation::Delete, Operation::Check, Operation::Retry] {
        assert!(matches!(execute(&fixture.scope, operation), Err(Error::Storage)));
    }
    assert_eq!(std::fs::read(store.path()).expect("after"), before);
    assert_eq!(
        std::fs::metadata(store.path()).expect("metadata").permissions().mode() & 0o777,
        0o644
    );
}
