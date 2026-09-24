use super::*;
use horizon_cloud_protocol::companion::{Request as WorkerRequest, Response};
use transport::{Transport, Worker};

struct Fixture {
    root: tempfile::TempDir,
    owner: Owner,
    context: Context,
    transport: Fake,
}

struct Fake {
    workers: BTreeMap<String, Worker>,
    calls: Vec<(String, String)>,
    fail_authorize: bool,
    cancel_on_call: Option<Cancellation>,
    journal: PathBuf,
}

impl Transport for Fake {
    fn worker(&mut self, cloud: &str) -> Result<Option<Worker>> {
        Ok(self.workers.get(cloud).cloned())
    }
    fn publish(&mut self, _: &str, catalog: &Catalog) -> Result<()> {
        catalog.validate().map_err(Error::Invalid)
    }
    fn call(&mut self, cloud: &str, request: &WorkerRequest) -> Result<Response> {
        let saved = std::fs::read_to_string(&self.journal).unwrap();
        assert!(
            saved.contains(request.grant()),
            "grant intent must be durable before SSH"
        );
        self.calls.push((
            cloud.into(),
            serde_json::to_value(request).unwrap()["operation"]
                .as_str()
                .unwrap()
                .into(),
        ));
        if let Some(cancel) = self.cancel_on_call.take() {
            cancel.cancel();
            return Err(horizon_cloud::CloudError::Cancelled.into());
        }
        match request {
            WorkerRequest::Identity { .. } => {
                assert!(saved.contains("worker-source") && saved.contains("worker-target"));
                Ok(Response::Identity {
                    public_key: "synthetic-key".into(),
                })
            }
            WorkerRequest::Authorize { grant, .. } => {
                if std::mem::take(&mut self.fail_authorize) {
                    return Err(Error::Invalid("reply lost after authorization"));
                }
                Ok(Response::Authorized {
                    host_key: "synthetic-host-key".into(),
                    worktree: format!("/workspace/companions/worktrees/{grant}"),
                })
            }
            WorkerRequest::Connect { grant, alias, .. } => Ok(Response::Connected {
                ssh_alias: format!("companion-{alias}"),
                worktree: format!("/workspace/companions/worktrees/{grant}"),
            }),
            WorkerRequest::Disconnect { .. } => Ok(Response::Disconnected),
            WorkerRequest::Revoke { .. } => Ok(Response::Revoked),
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let scope = Scope {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
        };
        let source = Target {
            scope: scope.clone(),
            cloud_id: "source".into(),
            declaration: Declaration {
                repository: "example/lib".into(),
                profile: "cpu".into(),
            },
        };
        let target = Target {
            scope: scope.clone(),
            cloud_id: "target".into(),
            declaration: Declaration {
                repository: "example/app".into(),
                profile: "cpu".into(),
            },
        };
        let owner = Owner {
            scope,
            cloud_id: source.cloud_id.clone(),
        };
        let context = Context {
            source: source.clone(),
            declarations: [("app".into(), target.declaration.clone())].into(),
            inventory: vec![source, target],
        };
        let workers = ["source", "target"]
            .into_iter()
            .map(|id| {
                (
                    id.into(),
                    Worker {
                        id: format!("worker-{id}"),
                        revision: "a".repeat(40),
                        address: Some("127.0.0.1:2222".parse().unwrap()),
                        status: Status::Ready,
                    },
                )
            })
            .collect();
        let transport = Fake {
            workers,
            calls: Vec::new(),
            fail_authorize: false,
            cancel_on_call: None,
            journal: root.path().join("source/companions.json"),
        };
        Self {
            root,
            owner,
            context,
            transport,
        }
    }
    fn run(&mut self, action: &Action) -> Snapshot {
        let store = journal::Store::open(self.root.path(), &self.owner).unwrap();
        execute(&store, &self.owner, Some(&self.context), action, &mut self.transport).unwrap()
    }
    fn select(&mut self) -> Snapshot {
        self.run(&Action::Select {
            alias: "app".into(),
            target_cloud_id: "target".into(),
        })
    }
    fn saved(&self) -> journal::State {
        journal::Store::open(self.root.path(), &self.owner)
            .unwrap()
            .load()
            .unwrap()
    }
}

#[test]
fn declarations_and_stopped_selection_never_issue_worker_commands() {
    let mut fixture = Fixture::new();
    assert_eq!(
        fixture.run(&Action::Refresh).rows[0].companion.status,
        Status::Unselected
    );
    fixture.transport.workers.get_mut("target").unwrap().status = Status::Stopped;
    let snapshot = fixture.select();
    assert!(snapshot.rows[0].companion.selected);
    assert_eq!(snapshot.rows[0].companion.status, Status::Stopped);
    assert!(fixture.transport.calls.is_empty());
    let grant = fixture.saved().grants.remove("app").unwrap();
    assert_eq!(grant.source_worker.as_deref(), Some("worker-source"));
    assert_eq!(grant.target_worker.as_deref(), Some("worker-target"));
    assert_eq!(grant.revision, Some("a".repeat(40)));
    assert!(grant.source_disconnected && grant.target_revoked);
    assert_eq!(fixture.run(&Action::Refresh).rows[0].companion.status, Status::Stopped);
}

#[test]
fn durable_grant_retries_reuse_the_identity_after_controller_restart() {
    let mut fixture = Fixture::new();
    let first = fixture.select();
    assert_eq!(first.rows[0].companion.status, Status::Ready);
    assert_eq!(
        fixture.transport.calls,
        vec![
            ("source".into(), "identity".into()),
            ("target".into(), "authorize".into()),
            ("source".into(), "connect".into())
        ]
    );
    let grant = first.rows[0].companion.access.clone();
    assert_eq!(fixture.run(&Action::Refresh).rows[0].companion.access, grant);
    fixture.context.inventory[1].declaration.repository = "EXAMPLE/App".into();
    assert_eq!(fixture.run(&Action::Refresh).rows[0].companion.status, Status::Ready);
}

#[test]
fn uncertain_authorization_is_revoked_and_offline_cleanup_remains_pending() {
    let mut fixture = Fixture::new();
    fixture.transport.fail_authorize = true;
    assert_eq!(fixture.select().rows[0].companion.status, Status::Unreachable);
    fixture.transport.workers.get_mut("target").unwrap().status = Status::Stopped;
    let cleared = fixture.run(&Action::Clear { alias: "app".into() });
    assert!(!cleared.rows[0].companion.selected);
    assert_eq!(cleared.rows[0].companion.status, Status::RevocationPending);
    let grant = fixture.saved().grants.remove("app").unwrap();
    assert!(grant.source_disconnected && !grant.target_revoked);
    assert!(
        fixture
            .transport
            .calls
            .iter()
            .any(|call| call == &("source".into(), "disconnect".into()))
    );
    fixture.transport.workers.get_mut("target").unwrap().status = Status::Ready;
    assert_eq!(
        fixture.run(&Action::Refresh).rows[0].companion.status,
        Status::Unselected
    );
    assert!(fixture.saved().grants.is_empty());
    assert_eq!(
        fixture.transport.calls.last().unwrap(),
        &("target".into(), "revoke".into())
    );
}

#[test]
fn transient_controller_failure_keeps_prior_access_available_for_worker_inspection() {
    let mut fixture = Fixture::new();
    let first = fixture.select();
    fixture.transport.fail_authorize = true;
    let failed = fixture.run(&Action::Refresh);
    assert_eq!(failed.rows[0].companion.status, Status::Unreachable);
    assert_eq!(failed.rows[0].companion.access, first.rows[0].companion.access);
    fixture.transport.workers.get_mut("target").unwrap().status = Status::Stopped;
    let cleared = fixture.run(&Action::Clear { alias: "app".into() });
    assert!(cleared.rows[0].companion.access.is_none());
    assert!(fixture.saved().grants["app"].access.is_none());
}

#[test]
fn changed_yaml_and_replaced_workers_cannot_redirect_a_selected_grant() {
    let mut fixture = Fixture::new();
    fixture.select();
    fixture.transport.calls.clear();
    fixture.context.declarations.get_mut("app").unwrap().repository = "example/other".into();
    let changed = fixture.run(&Action::Refresh);
    assert_eq!(changed.rows[0].companion.status, Status::Changed);
    assert!(changed.rows[0].companion.access.is_none());
    assert_eq!(
        fixture
            .transport
            .calls
            .iter()
            .map(|(_, operation)| operation.as_str())
            .collect::<Vec<_>>(),
        ["disconnect", "revoke"]
    );

    let mut fixture = Fixture::new();
    fixture.select();
    fixture.transport.calls.clear();
    fixture.transport.workers.get_mut("target").unwrap().id = "replacement-worker".into();
    let snapshot = fixture.run(&Action::Refresh);
    assert_eq!(snapshot.rows[0].companion.status, Status::RevocationPending);
    assert_eq!(snapshot.catalog.companions[0].status, Status::RevocationPending);
    assert!(snapshot.catalog.companions[0].access.is_none());
    assert_eq!(fixture.transport.calls, [("source".into(), "disconnect".into())]);
}

#[test]
fn moving_source_to_another_workspace_cleans_old_grants_before_accepting_new_ones() {
    let mut fixture = Fixture::new();
    fixture.select();
    fixture.transport.calls.clear();
    fixture.owner.scope.workspace_id = "new-workspace".into();
    fixture.context.source.scope = fixture.owner.scope.clone();
    let snapshot = fixture.select();
    assert!(snapshot.notice.is_some());
    assert!(fixture.saved().grants.is_empty());
    assert_eq!(fixture.saved().owner, fixture.owner);
    assert!(
        fixture
            .transport
            .calls
            .iter()
            .all(|(_, operation)| operation == "disconnect" || operation == "revoke")
    );
}

#[test]
fn invalid_scope_ambiguity_and_competing_controllers_fail_closed() {
    let mut fixture = Fixture::new();
    let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
    assert!(journal::Store::open(fixture.root.path(), &fixture.owner).is_err());
    let mut state = store.load().unwrap();
    fixture.context.inventory.push(fixture.context.inventory[1].clone());
    let select = Action::Select {
        alias: "app".into(),
        target_cloud_id: "target".into(),
    };
    assert!(apply_action(&mut state, Some(&fixture.context), &select).is_err());
    fixture.context.inventory.pop();
    fixture.context.inventory[1].scope.workspace_id = "other-workspace".into();
    assert!(apply_action(&mut state, Some(&fixture.context), &select).is_err());
    assert!(state.grants.is_empty());
    let mut owner = fixture.owner.clone();
    owner.cloud_id = "../escape".into();
    assert!(journal::Store::open(fixture.root.path(), &owner).is_err());
}

#[test]
fn contradictory_access_and_duplicate_grants_fail_before_ssh_or_journal_mutation() {
    for corruption in ["source", "target", "duplicate", "source_pin", "target_pin", "revision"] {
        let mut fixture = Fixture::new();
        fixture.transport.fail_authorize = corruption.ends_with("_pin") || corruption == "revision";
        fixture.select();
        fixture.transport.calls.clear();
        let store = journal::Store::open(fixture.root.path(), &fixture.owner).unwrap();
        let mut state = store.load().unwrap();
        let grant = state.grants.get_mut("app").unwrap();
        match corruption {
            "source" => grant.source_disconnected = true,
            "target" => grant.target_revoked = true,
            "source_pin" => grant.source_worker = None,
            "target_pin" => grant.target_worker = None,
            "revision" => grant.revision = None,
            _ => {
                let mut duplicate = grant.clone();
                duplicate.selection =
                    Selection::new(&fixture.context.source, "utility", &fixture.context.inventory[1]).unwrap();
                duplicate.access.as_mut().unwrap().ssh_alias = "companion-utility".into();
                state.grants.insert("utility".into(), duplicate);
            }
        }
        store.save(&state).unwrap();
        let before = std::fs::read(&fixture.transport.journal).unwrap();
        assert!(
            execute(
                &store,
                &fixture.owner,
                Some(&fixture.context),
                &Action::Clear { alias: "app".into() },
                &mut fixture.transport,
            )
            .is_err()
        );
        assert!(fixture.transport.calls.is_empty());
        assert_eq!(std::fs::read(&fixture.transport.journal).unwrap(), before);
    }
}

#[test]
fn stopped_selection_fences_replacements_and_clears_without_remote_cleanup() {
    for change in ["source", "target", "revision"] {
        let mut fixture = Fixture::new();
        fixture.transport.workers.get_mut("target").unwrap().status = Status::Stopped;
        fixture.select();
        let worker = fixture
            .transport
            .workers
            .get_mut(if change == "revision" { "target" } else { change })
            .unwrap();
        if change == "revision" {
            worker.revision = "b".repeat(40);
        } else {
            worker.id = "replacement".into();
        }
        fixture.transport.workers.get_mut("target").unwrap().status = Status::Ready;
        assert_eq!(fixture.run(&Action::Refresh).rows[0].companion.status, Status::Changed);
        assert!(fixture.transport.calls.is_empty());
    }
    let mut fixture = Fixture::new();
    fixture.transport.workers.get_mut("target").unwrap().status = Status::Stopped;
    fixture.select();
    assert_eq!(
        fixture.run(&Action::Clear { alias: "app".into() }).rows[0]
            .companion
            .status,
        Status::Unselected
    );
    assert!(fixture.saved().grants.is_empty());
    assert!(fixture.transport.calls.is_empty());
}

#[test]
fn observed_target_is_pinned_even_when_the_source_is_missing() {
    let mut fixture = Fixture::new();
    let source = fixture.transport.workers.remove("source").unwrap();
    assert_eq!(fixture.select().rows[0].companion.status, Status::Unavailable);
    assert_eq!(
        fixture.saved().grants["app"].target_worker.as_deref(),
        Some("worker-target")
    );
    fixture.transport.workers.insert("source".into(), source);
    fixture.transport.workers.get_mut("target").unwrap().id = "replacement".into();
    assert_eq!(fixture.run(&Action::Refresh).rows[0].companion.status, Status::Changed);
    assert!(fixture.transport.calls.is_empty());
}

#[test]
fn cancellation_is_observable_before_state_creation_and_after_remote_work() {
    let mut fixture = Fixture::new();
    let request = Request {
        root: fixture.root.path().into(), owner: fixture.owner.clone(), context: Some(fixture.context.clone()),
        action: Action::Select { alias: "app".into(), target_cloud_id: "target".into() },
        settings: serde_json::from_value(serde_json::json!({
            "runpod_key_file":"/absent", "ssh_identity_file":"/absent", "docker_config":"/absent", "cpu_flavors":[], "gpu_types":[]
        })).unwrap(),
    };
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        refresh(&request, &cancel),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
    assert_eq!(std::fs::read_dir(fixture.root.path()).unwrap().count(), 0);
    let cancel = Cancellation::default();
    fixture.transport.cancel_on_call = Some(cancel.clone());
    assert!(matches!(
        refresh_with_transport(&request, &cancel, &mut fixture.transport),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
    let grant = fixture.saved().grants.remove("app").unwrap();
    assert!(grant.selected && !grant.source_disconnected && !grant.target_revoked);
    assert_eq!(grant.target_worker.as_deref(), Some("worker-target"));
}
