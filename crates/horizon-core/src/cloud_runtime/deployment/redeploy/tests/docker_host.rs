use super::*;
use crate::cloud_runtime::{
    Cancellation, deployment::deletion::terminate, lifecycle::can_remove, tailnet::Selection, timeline::Timeline,
};
use horizon_cloud::docker_host::{self, Engine, Journal, Records};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

// Only this fixture executable runs; no real Docker engine or remote host is contacted.
const DOCKER: &str = r"#!/usr/bin/python3
import json,pathlib,sys
root=pathlib.Path(__file__).parent
path=root/'engine.json'; state=json.loads(path.read_text())
args=sys.argv[1:]; i=next(i for i,a in enumerate(args) if a in ('info','container','volume')); args=args[i:]
state.setdefault('calls',[]).append(' '.join(args))
def save():path.write_text(json.dumps(state))
def done(value=''):save();print(value);sys.exit(0)
if args[0]=='info':
 done('FIXTURE-ENGINE' if args[-1]=='{{.ID}}' else json.dumps(dict(OSType='linux',NCPU=1,MemTotal=1073741824,ServerVersion='29.0.1',Architecture='x86_64')))
if args[:2]==['container','ls']:
 if 'label=io.horizon.cloud=deleted-cloud' in args:
  state['lookups']=state.get('lookups',0)+1
  if state.get('appear_on_lookup')==state['lookups']:state['present']=True
 if state.pop('appear_after_first_lookup',False):state['present']=True;done('')
 done('a'*64 if state.get('present') else '')
if args[:2]==['container','inspect']:
 if state.pop('block_save',False):
  record=root/'deleted-cloud'/'deployment.json';record.rename(record.with_suffix('.before'));record.mkdir()
 done(json.dumps([dict(Id='a'*64,Name='/horizon-cloud-deleted-cloud',Config=dict(Labels={'io.horizon.cloud':'deleted-cloud'},Image='fixture',Env=[]),HostConfig=dict(NanoCpus=4000000000,Memory=8589934592,PortBindings={'22/tcp':[dict(HostIp='127.0.0.1',HostPort='31000')]}),Mounts=[dict(Type='volume',Name='horizon-cloud-deleted-cloud-workspace',Destination='/workspace')],State=dict(Running=True,StartedAt='2026-10-14T00:00:00Z'))]))
if args[:2]==['container','rm']:
 assert json.loads((root/'deleted-cloud'/'deployment.json').read_text())['operation']['state']!='prepared'
 state['present']=False
 if state.pop('lose_remove_reply',False):save();sys.exit(1)
 done()
if args[:2]==['volume','ls']:done('horizon-cloud-deleted-cloud-workspace' if state.get('volume') else '')
if args[:2]==['volume','rm']:state['volume']=False;done()
raise AssertionError('unexpected fixture command')
";
struct NoWrites;
impl Records for NoWrites {
    fn journal(&mut self, _: &Journal) -> std::result::Result<(), horizon_cloud::CloudError> {
        panic!("capacity failure must not save a resource intent")
    }
    fn operation(&mut self, _: &CreateState) -> std::result::Result<(), horizon_cloud::CloudError> {
        panic!("capacity failure must not request a worker")
    }
    fn mutation(&mut self) -> std::result::Result<(), horizon_cloud::CloudError> {
        panic!("capacity failure must not mutate a worker")
    }
}
fn fixture() -> (tempfile::TempDir, PathBuf, Settings) {
    let root = tempfile::tempdir().unwrap();
    let cloud = root.path().join("deleted-cloud");
    let script = root.path().join("fixture-docker");
    std::fs::write(&script, DOCKER).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(root.path().join("engine.json"), b"{}").unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file":root.path().join("unused-key"),"ssh_identity_file":root.path().join("unused-identity"),
        "docker_config":root.path().join("unused-docker"),"cpu_flavors":[],"gpu_types":[],
        "docker_hosts":[{"name":"fixture","access":"this-pc","docker":script}]
    }))
    .unwrap();
    let mut state = deleted(&json!({"state":"prepared"}));
    state.profile.provider = "docker-host".into();
    state.stage = Stage::Provision;
    state.source_ready = false;
    state.ready_history = ReadyHistory::Unobserved;
    state.ready_after_seconds = None;
    state.stop_requested = false;
    state.browserstack_targets.clear();
    state.sessions.clear();
    let spec = state.spec.as_mut().unwrap();
    spec.profile = state.profile.clone();
    spec.public_key = PUBLIC_KEY.into();
    spec.data_centers = vec!["fixture".into()];
    let store = Store::lock(&cloud).unwrap();
    store.save(&state).unwrap();
    let cancel = Cancellation::default();
    let engine = Engine {
        host: &settings.docker_hosts[0],
        local_config: cloud.join("docker-config"),
        cancel: &cancel,
        engine_id: Mutex::new(None),
    };
    let mut operation = state.operation.clone();
    let mut journal = Journal::default();
    assert!(matches!(
        docker_host::provision(
            &engine,
            &settings.docker_hosts[0],
            state.spec.as_ref().unwrap(),
            &mut operation,
            &mut journal,
            &mut NoWrites
        ),
        Err(horizon_cloud::CloudError::Capacity(_))
    ));
    assert_eq!(operation, CreateState::Prepared);
    assert_eq!(journal, Journal::default());
    assert_eq!(store.load().unwrap().unwrap().operation, CreateState::Prepared);
    Selection {
        tailnet: Some("work".into()),
    }
    .commit(&cloud)
    .unwrap();
    drop(store);
    (root, cloud, settings)
}
fn engine_state(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("engine.json")).unwrap()).unwrap()
}
fn change_engine(root: &Path, mutate: impl FnOnce(&mut Value)) {
    let mut state = engine_state(root);
    mutate(&mut state);
    std::fs::write(root.join("engine.json"), serde_json::to_vec(&state).unwrap()).unwrap();
}
fn delete(cloud: &Path, settings: &Settings) -> crate::cloud_runtime::Result<()> {
    terminate(cloud, settings, &Cancellation::default(), &|_| {})
}

#[test]
fn failed_initial_docker_capacity_remains_never_provisioned_through_delete_retry_and_redeploy() {
    for volume in [false, true] {
        let (root, cloud, settings) = fixture();
        if volume {
            change_engine(root.path(), |state| state["volume"] = json!(true));
            std::fs::write(
                cloud.join("docker-host.json"),
                serde_json::to_vec(&Journal {
                    volume: true,
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap();
        }
        for _ in 0..2 {
            delete(&cloud, &settings).unwrap();
            delete(&cloud, &settings).unwrap();
            let store = Store::lock(&cloud).unwrap();
            let mut state = store.load().unwrap().unwrap();
            assert_eq!(state.stage, Stage::Deleted);
            assert_eq!(state.operation, CreateState::Prepared);
            assert_eq!(state.ready_history, ReadyHistory::Unobserved);
            assert!(state.tailnet_enrollment.is_empty());
            assert!(can_remove(&store, &state).unwrap());
            assert_eq!(std::fs::read(cloud.join("docker-host.json")).unwrap(), b"{}");
            reopen(&store, &mut state, PUBLIC_KEY).unwrap();
            assert_eq!(state.stage, Stage::Validate);
            assert_eq!(state.operation, CreateState::Prepared);
        }
        assert!(
            !engine_state(root.path())
                .get("volume")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
    }
}
#[test]
fn docker_prepared_deletion_keeps_legacy_and_requested_or_port_history_manual() {
    for history in ["ready", "stop", "resume", "requested", "terminated", "port"] {
        let (_root, cloud, settings) = fixture();
        {
            let store = Store::lock(&cloud).unwrap();
            let mut state = store.load().unwrap().unwrap();
            match history {
                "ready" => state.ready_history = ReadyHistory::Observed,
                "stop" => state.stop_requested = true,
                "resume" => {
                    state.timeline = Some(Timeline {
                        resume_requested: Some(0),
                        ..Default::default()
                    });
                }
                "requested" => state.operation = CreateState::Requested,
                "terminated" => {
                    state.operation = CreateState::Terminated {
                        worker_id: "horizon-cloud-deleted-cloud".into(),
                    }
                }
                "port" => std::fs::write(
                    cloud.join("docker-host.json"),
                    serde_json::to_vec(&Journal {
                        port: Some(31000),
                        ..Default::default()
                    })
                    .unwrap(),
                )
                .unwrap(),
                _ => unreachable!(),
            }
            store.save(&state).unwrap();
        }
        assert!(
            delete(&cloud, &settings)
                .unwrap_err()
                .to_string()
                .contains("not recorded"),
            "{history}"
        );
        let store = Store::lock(&cloud).unwrap();
        let mut state = store.load().unwrap().unwrap();
        let before = std::fs::read(cloud.join("deployment.json")).unwrap();
        assert!(
            can_remove(&store, &state)
                .unwrap_err()
                .to_string()
                .contains("not recorded"),
            "{history}"
        );
        assert!(
            reopen(&store, &mut state, PUBLIC_KEY)
                .unwrap_err()
                .to_string()
                .contains("not recorded"),
            "{history}"
        );
        assert_eq!(std::fs::read(cloud.join("deployment.json")).unwrap(), before);
    }
}
#[test]
fn unexpected_early_or_later_container_is_durably_requested_before_remove_and_retry() {
    for late in [false, true] {
        let (root, cloud, settings) = fixture();
        change_engine(root.path(), |state| {
            state["present"] = json!(!late);
            state["appear_after_first_lookup"] = json!(late);
            state["lose_remove_reply"] = json!(true);
        });
        assert!(delete(&cloud, &settings).is_err());
        {
            let store = Store::lock(&cloud).unwrap();
            let state = store.load().unwrap().unwrap();
            assert_eq!(state.operation, CreateState::Requested);
            assert!(!engine_state(root.path())["present"].as_bool().unwrap());
        }
        assert!(
            delete(&cloud, &settings)
                .unwrap_err()
                .to_string()
                .contains("not recorded")
        );
        let store = Store::lock(&cloud).unwrap();
        let mut state = store.load().unwrap().unwrap();
        assert!(matches!(state.operation, CreateState::Terminated { .. }));
        assert!(can_remove(&store, &state).is_err());
        assert!(reopen(&store, &mut state, PUBLIC_KEY).is_err());
    }
}
#[test]
fn failed_requested_save_cannot_remove_a_container_observed_by_any_lookup() {
    for lookup in [1, 2, 3] {
        let (root, cloud, settings) = fixture();
        change_engine(root.path(), |state| {
            state["appear_on_lookup"] = json!(lookup);
            state["block_save"] = json!(true);
        });
        assert!(delete(&cloud, &settings).is_err());
        let engine = engine_state(root.path());
        assert_eq!(engine["present"], json!(true));
        assert!(
            engine["calls"]
                .as_array()
                .unwrap()
                .iter()
                .all(|call| !call.as_str().unwrap().starts_with("container rm"))
        );
        let before: Value = serde_json::from_slice(&std::fs::read(cloud.join("deployment.before")).unwrap()).unwrap();
        assert_eq!(before["operation"]["state"], json!("prepared"));
        std::fs::remove_dir(cloud.join("deployment.json")).unwrap();
        std::fs::rename(cloud.join("deployment.before"), cloud.join("deployment.json")).unwrap();
        assert!(
            delete(&cloud, &settings)
                .unwrap_err()
                .to_string()
                .contains("not recorded")
        );
        assert!(!engine_state(root.path())["present"].as_bool().unwrap());
    }
}

#[test]
fn final_verification_observation_remains_requested_after_container_disappears_before_retry() {
    let (root, cloud, settings) = fixture();
    change_engine(root.path(), |state| state["appear_on_lookup"] = json!(3));
    let error = delete(&cloud, &settings).unwrap_err();
    assert!(error.to_string().contains("did not remove"), "{error}");
    let engine = engine_state(root.path());
    assert_eq!(engine["lookups"], json!(3));
    assert_eq!(engine["present"], json!(true));
    assert!(
        engine["calls"]
            .as_array()
            .unwrap()
            .iter()
            .all(|call| !call.as_str().unwrap().starts_with("container rm"))
    );
    {
        let store = Store::lock(&cloud).unwrap();
        assert_eq!(store.load().unwrap().unwrap().operation, CreateState::Requested);
    }
    change_engine(root.path(), |state| state["present"] = json!(false));
    assert!(
        delete(&cloud, &settings)
            .unwrap_err()
            .to_string()
            .contains("not recorded")
    );
    let store = Store::lock(&cloud).unwrap();
    let mut state = store.load().unwrap().unwrap();
    assert!(matches!(state.operation, CreateState::Terminated { .. }));
    assert!(can_remove(&store, &state).is_err());
    assert!(reopen(&store, &mut state, PUBLIC_KEY).is_err());
}
