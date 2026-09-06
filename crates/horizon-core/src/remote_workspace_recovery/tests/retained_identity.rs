use super::*;

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    let saved = fixture.reload();
    let runtime = saved.workspace().state().runtime.as_ref().expect("runtime");
    let identity = fixture
        .identities
        .prepare_new(runtime.workflow_id, runtime.job_id)
        .expect("key");
    fixture.reserve(identity.public_key());
    fixture
}

#[test]
fn lost_response_reopens_same_worker_and_private_key_without_ensure_or_cleanup() {
    let fixture = fixture();
    let initial = fixture.reload();
    let provider = Provider::new(Some(fixture.status()));
    let recovered = fixture.run(&provider).expect("recover lost response");
    let saved = recovered.allocation().clone();
    let path = recovered.identity().private_key_path().to_path_buf();
    let key_bytes = std::fs::read(&path).expect("key bytes");
    assert_eq!(recovered.observation(), provider.status.as_ref());
    assert_eq!(saved.workflow(), initial.workflow());
    assert_eq!(provider.calls(), [0, 1, 0, 0]);
    assert!(!format!("{recovered:?}").contains(&path.to_string_lossy().into_owned()));
    drop(recovered);
    let reopened = CloudWorkflowStore::open_path(fixture.store.path()).expect("reopen");
    let identities = RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")));
    let recovered = recover_remote_workspace(&reopened, &identities, &provider, OWNER, "workspace").expect("inspect");
    assert_eq!(recovered.allocation(), &saved);
    assert_eq!(recovered.identity().private_key_path(), path);
    drop(recovered);
    assert_eq!(provider.calls(), [0, 1, 1, 0]);
    assert_eq!(std::fs::read(path).expect("retained key"), key_bytes);
    assert!(
        saved.workspace().state().spec.panels.is_empty(),
        "zero panels never own lifetime"
    );
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn missing_and_mismatched_private_keys_never_recreate_identity_or_call_provider() {
    let fixture = fixture();
    let provider = Provider::new(None);
    let saved = fixture.reload();
    let request = saved.worker_request().expect("request");
    let key = fixture
        .identities
        .recover(request.workflow_id, request.job_id, &request.ssh_public_key)
        .expect("key");
    let bytes = std::fs::read(key.private_key_path()).expect("bytes");
    let other = fixture
        .identities
        .prepare_new(
            crate::cloud_run::CloudWorkflowId::new(),
            crate::cloud_run::CloudJobId::new(),
        )
        .expect("other key");
    std::fs::write(
        key.private_key_path(),
        std::fs::read(other.private_key_path()).expect("other bytes"),
    )
    .expect("fault");
    assert_eq!(
        fixture.run(&provider).expect_err("mismatch"),
        RemoteWorkspaceRecoveryError::Identity(RemoteSshIdentityError::Mismatch)
    );
    std::fs::write(key.private_key_path(), bytes).expect("restore fixture");
    std::fs::remove_file(key.private_key_path()).expect("simulate missing fixture key");
    assert_eq!(
        fixture.run(&provider).expect_err("missing"),
        RemoteWorkspaceRecoveryError::Identity(RemoteSshIdentityError::Missing)
    );
    assert!(!key.private_key_path().exists());
    assert_eq!(provider.calls(), [0; 4]);
    assert_eq!(fixture.reload(), saved);
}

#[test]
fn expired_setup_and_absent_worker_do_not_renew_or_allocate() {
    let fixture = fixture();
    let mut workflow = fixture.reload().workflow().workflow().clone();
    workflow.created_at_millis = 1000;
    workflow.updated_at_millis = 1000;
    workflow.retain_until_millis = 2000;
    rusqlite::Connection::open(fixture.store.path())
        .expect("connection")
        .execute(
            "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000, retain_until_millis=2000,
         snapshot=?1 WHERE workflow_id=?2",
            rusqlite::params![
                serde_json::to_vec(&workflow).expect("snapshot"),
                workflow.id.to_string()
            ],
        )
        .expect("expired fixture");
    let expired = fixture.reload();
    let provider = Provider::new(Some(fixture.status()));
    let recovered = fixture.run(&provider).expect("persistent recovery after setup expiry");
    assert_eq!(recovered.allocation().workflow(), expired.workflow());
    let saved = recovered.allocation().clone();
    let absent = Provider::new(None);
    let recovered = fixture.run(&absent).expect("absent is not replacement authority");
    assert!(recovered.observation().is_none());
    assert_eq!(recovered.allocation(), &saved);
    assert_eq!(absent.calls(), [0, 0, 1, 0]);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn slow_provider_cannot_overwrite_newer_intent_and_provider_errors_are_redacted() {
    let fixture = fixture();
    let mut provider = Provider::new(Some(fixture.status()));
    let store = fixture.store.clone();
    provider.during_read = Some(Box::new(move || {
        let current = store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation");
        let mut state = current.workspace().state().clone();
        state.spec.working_directory = "changed".into();
        store
            .replace_remote_workspace(current.workspace(), &state)
            .expect("user edit");
    }));
    assert_eq!(
        fixture.run(&provider).expect_err("stale"),
        RemoteWorkspaceRecoveryError::StateChanged
    );
    assert_eq!(fixture.reload().workspace().state().spec.working_directory, "changed");
    assert!(
        fixture
            .reload()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .is_none()
    );
    provider.during_read = None;
    provider.fail = true;
    let saved = fixture.reload();
    let error = fixture.run(&provider).expect_err("provider failed");
    assert_eq!(error, RemoteWorkspaceRecoveryError::ProviderUnavailable);
    assert!(!format!("{error:?} {error}").contains("synthetic-private-provider-response"));
    assert_eq!(fixture.reload(), saved);
    assert_eq!(provider.calls(), [0, 2, 0, 0]);
}
