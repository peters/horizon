use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, interactive_worker::*},
    remote_workspace::RemoteWorkspaceState,
};

struct NeverProvider;

impl InteractiveWorkerProvider for NeverProvider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        panic!("no provider access")
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no reconcile")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no inspect")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

#[test]
fn unsupported_clients_return_before_store_provider_identity_or_transport_io() {
    let directory = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(directory.path().join("home"));
    let store = CloudWorkflowStore::open(&home).expect("store");
    let identities = RemoteSshIdentityStore::new(&home);
    let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
        "version":1,"spec":{
            "workspace_local_id":"workspace","working_directory":".","generation":0,"panels":[],
            "target":{"provider":"local_docker","profile":"development","disk_gib":20,
                "image":format!("example/worker@sha256:{}", "a".repeat(64)),"lifetime":"persistent"},
            "repository":{"repository":"example/project","commit":"b".repeat(40)}
        }
    }))
    .expect("state");
    let dormant = store
        .create_remote_workspace("00000000-0000-4000-8000-000000000001", &state)
        .expect("workspace");
    let allocation = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
    let retained = store.path().with_extension("retained");
    std::fs::rename(store.path(), &retained).expect("retain fixture database");
    assert_eq!(
        install_remote_github_credential(
            &store,
            &identities,
            &NeverProvider,
            &allocation,
            &RepositoryPat::new("synthetic_PAT").expect("token")
        ),
        Err(RemoteCredentialDeliveryError::UnsupportedPlatform)
    );
    assert!(!store.path().exists());
    assert!(!directory.path().join("home/remote-ssh-identities").exists());
}
