use super::*;
use horizon_app_provider::api::Quota;
use horizon_app_testing::contract::{Form, Platform};
use horizon_browser::{WebDriverHttpError, remote::*};
use horizon_core::remote_browser_credential::{
    CredentialLocator, CredentialStores, RemoteCredentialStore, SessionCredentialStore,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
pub(crate) struct Transport {
    pub(crate) active: Mutex<BTreeMap<String, Value>>,
    pub(crate) creates: AtomicUsize,
    pub(crate) deletes: AtomicUsize,
    allocation_timeouts: Mutex<Vec<(Duration, Instant)>>,
    pub(crate) after_create: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    mutations: AtomicUsize,
    pub(crate) lost_create: AtomicBool,
    pub(crate) lost_delete: AtomicBool,
    panic_native_delete: AtomicBool,
    delay_create: AtomicBool,
    pause: Mutex<Option<String>>,
    entered: AtomicBool,
    resume: AtomicBool,
}
impl ClassicTransport for Transport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        timeout: Duration,
    ) -> std::result::Result<Value, WebDriverHttpError> {
        if method == "POST" && path != "/session" {
            self.mutations.fetch_add(1, Ordering::SeqCst);
        }
        if method == "POST" && path == "/session" {
            self.allocation_timeouts.lock().unwrap().push((timeout, Instant::now()));
            let index = self.creates.fetch_add(1, Ordering::SeqCst) + 1;
            let id = format!("synthetic_session_{index:016}");
            self.active.lock().unwrap().insert(id.clone(), body.cloned().unwrap());
            if self.delay_create.load(Ordering::SeqCst) {
                std::thread::sleep(timeout + Duration::from_millis(50));
            }
            if self.lost_create.load(Ordering::SeqCst) {
                return Err(WebDriverHttpError::Transport("synthetic lost reply".into()));
            }
            let hook = self.after_create.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
            return Ok(json!({"value":{"sessionId":id}}));
        }
        let paused = self.pause.lock().unwrap().as_ref().is_some_and(|id| path.contains(id));
        if paused && (path.ends_with("/execute/sync") || method == "DELETE") {
            self.entered.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !self.resume.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if method == "DELETE" {
            assert!(
                !self.panic_native_delete.load(Ordering::SeqCst),
                "injected native DELETE panic"
            );
            self.deletes.fetch_add(1, Ordering::SeqCst);
            if self.lost_delete.load(Ordering::SeqCst) {
                return Err(WebDriverHttpError::Transport("synthetic lost reply".into()));
            }
            self.active.lock().unwrap().remove(path.trim_start_matches("/session/"));
        }
        if path.ends_with("/source") {
            return Ok(
                json!({"value":"<AppiumAUT><XCUIElementTypeApplication name='app'><XCUIElementTypeButton name='menu.open' label='Menu' visible='true' enabled='true'/></XCUIElementTypeApplication></AppiumAUT>"}),
            );
        }
        if path.ends_with("/elements") {
            if body.and_then(|value| value.get("using")).and_then(Value::as_str) == Some("accessibility id") {
                return Ok(json!({"value":[{"element-6066-11e4-a52e-4f735466cecf":"menu-element"}]}));
            }
            return Ok(
                json!({"value":[{"element-6066-11e4-a52e-4f735466cecf":"application-element"},{"element-6066-11e4-a52e-4f735466cecf":"menu-element"}]}),
            );
        }
        if path.ends_with("/displayed") || path.ends_with("/enabled") {
            return Ok(json!({"value":true}));
        }
        if path.ends_with("/screenshot") {
            use base64::Engine as _;
            let mut bytes = Vec::new();
            {
                let mut writer = png::Encoder::new(&mut bytes, 1, 1).write_header().unwrap();
                writer.write_image_data(&[0]).unwrap();
            }
            return Ok(json!({"value":base64::engine::general_purpose::STANDARD.encode(bytes)}));
        }
        Ok(json!({"value":null}))
    }
}
pub(crate) struct Fake {
    pub(crate) transport: Arc<Transport>,
    reject_verification: AtomicBool,
    pub(crate) delay_capacity: AtomicBool,
    before_driver_return: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    media_calls: AtomicUsize,
    uploads: AtomicUsize,
    upload_deletes: AtomicUsize,
    lost_upload: AtomicBool,
    after_upload: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    before_capacity: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    panic_delete: AtomicBool,
    lost_upload_delete_once: AtomicBool,
}
impl Backend for Fake {
    fn media(&self, reference: &str, kind: horizon_app_provider::media::Kind, _timeout: Duration) -> Result<Vec<u8>> {
        self.media_calls.fetch_add(1, Ordering::SeqCst);
        assert!(reference.starts_with("synthetic_session_"));
        Ok(if matches!(kind, horizon_app_provider::media::Kind::Video) {
            b"\0\0\0\x0cftypisom".to_vec()
        } else {
            b"redacted fixture stack frame\n".to_vec()
        })
    }
    fn capacity(&self, timeout: Duration) -> Result<Capacity> {
        let hook = self.before_capacity.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        if self.delay_capacity.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(80).min(timeout));
            if timeout <= Duration::from_millis(80) {
                return Err(horizon_app_runtime::Error::CapacityUnavailable.into());
            }
        }
        let running = self
            .transport
            .active
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        Capacity::observed(
            Quota {
                parallel_sessions_max_allowed: 2,
                team_parallel_sessions_max_allowed: 2,
                parallel_sessions_running: u32::try_from(running.len()).unwrap(),
                queued_sessions: 0,
            },
            running,
        )
        .map_err(Error::from)
    }
    fn driver(&self) -> Result<Arc<dyn ClassicTransport>> {
        if let Some(hook) = self.before_driver_return.lock().unwrap().take() {
            hook();
        }
        Ok(self.transport.clone())
    }
    fn verify(&self, id: &str, device: &Device, app: &UploadedApp, operation: Uuid, _timeout: Duration) -> Result<()> {
        if self.reject_verification.load(Ordering::SeqCst) {
            return Err(horizon_app_provider::Error::DeviceUnverified.into());
        }
        let active = self.transport.active.lock().unwrap();
        let caps = &active.get(id).unwrap()["capabilities"]["alwaysMatch"];
        assert_eq!(caps["appium:deviceName"], device.model);
        assert_eq!(
            caps["bstack:options"]["buildName"],
            format!("horizon-native-{operation}")
        );
        app.use_for_driver(|token| assert_eq!(caps["appium:app"], token));
        Ok(())
    }
    fn upload(&self, _: &mut Artifact, _: Uuid, timeout: Duration) -> Result<UploadedApp> {
        assert!(!timeout.is_zero());
        let index = self.uploads.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(hook) = self.after_upload.lock().unwrap().take() {
            hook();
        }
        if self.lost_upload.load(Ordering::SeqCst) {
            return Err(horizon_app_provider::Error::ProviderFailed.into());
        }
        Ok(UploadedApp::from_owned_reference(&format!("bs://{index:040x}"))?)
    }
    fn confirmed_closed(&self, reference: &str) -> Result<bool> {
        Ok(!self.transport.active.lock().unwrap().contains_key(reference))
    }
    fn delete(&self, _: &UploadedApp) -> Result<()> {
        assert!(
            !self.panic_delete.load(Ordering::SeqCst),
            "injected provider DELETE panic"
        );
        self.upload_deletes.fetch_add(1, Ordering::SeqCst);
        if self.lost_upload_delete_once.swap(false, Ordering::SeqCst) {
            return Err(Error::CleanupUncertain);
        }
        Ok(())
    }
}
pub(crate) fn account() -> Account {
    let bindings = ["user", "key"]
        .map(|reference| {
            (
                CredentialReference::from(reference),
                CredentialBinding {
                    store: CredentialStoreKind::Session,
                    slot: None,
                },
            )
        })
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let profile = RemoteProviderProfile {
        adapter: RemoteAdapterKind::Browserstack,
        endpoint: ControlEndpoint::parse("https://hub-cloud.browserstack.com/wd/hub").unwrap(),
        authentication: RemoteAuthentication::Basic {
            username_ref: CredentialReference::from("user"),
            password_ref: CredentialReference::from("key"),
        },
        credential_bindings: bindings,
        limits: RemoteSessionLimits::default(),
    };
    let mut store = SessionCredentialStore::new();
    for (reference, value) in [("user", "synthetic-user"), ("key", "synthetic-key")] {
        let reference = CredentialReference::from(reference);
        store
            .put(
                &CredentialLocator::new(&profile.endpoint, &reference, &profile.credential_bindings[&reference]),
                value.as_bytes(),
            )
            .unwrap();
    }
    Account::capture(
        &profile,
        &CredentialStores {
            session: &store,
            os_keychain: None,
            environment: None,
        },
    )
    .unwrap()
}

pub(crate) struct Fixture {
    pub(crate) workspace: Arc<Workspace>,
    pub(crate) fake: Arc<Fake>,
    pub(crate) root: tempfile::TempDir,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let account = account();
    let journal = Arc::new(horizon_app_runtime::journal::Journal::open(&path.join("private"), &account).unwrap());
    let workspace = Arc::new(Workspace::open(journal, Uuid::new_v4(), &path).unwrap());
    let fake = Arc::new(Fake {
        transport: Arc::new(Transport::default()),
        reject_verification: AtomicBool::new(false),
        delay_capacity: AtomicBool::new(false),
        before_driver_return: Mutex::new(None),
        media_calls: AtomicUsize::new(0),
        uploads: AtomicUsize::new(0),
        upload_deletes: AtomicUsize::new(0),
        lost_upload: AtomicBool::new(false),
        after_upload: Mutex::new(None),
        before_capacity: Mutex::new(None),
        panic_delete: AtomicBool::new(false),
        lost_upload_delete_once: AtomicBool::new(false),
    });
    Fixture { workspace, fake, root }
}
use crate::local::Configuration;
use horizon_app_testing::contract::{Backend as DeclaredBackend, Port};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
pub(crate) fn actor(base_url: &str) -> (Fixture, Actor) {
    let fixture = fixture();
    let root = fixture.root.path().canonicalize().unwrap();
    let script = r"#!/usr/bin/python3
import json,os,pathlib,socket,sys
spec=json.loads(sys.stdin.readline())
path=pathlib.Path(spec['state'])/'process.json'
receipt={'operation':spec['operation'],'guardian_pid':os.getpid(),'complete':False}
path.write_text(json.dumps(receipt));path.chmod(0o600)
tunnel='key' in spec
print(json.dumps('armed' if tunnel else {'phase':'armed'}),flush=True)
sock=None
if sys.stdin.readline()=='start\n':
    if not tunnel and spec['kind']=='build':
        print(json.dumps({'phase':'started'}),flush=True)
        receipt['complete']=True;path.write_text(json.dumps(receipt))
        print(json.dumps({'phase':'complete','success':True}),flush=True)
        sys.exit(0)
    if tunnel: print(json.dumps('ready'),flush=True)
    else:
        sock=socket.socket();sock.bind(('127.0.0.1',0));sock.listen()
        ports=pathlib.Path(spec['root'])/'backend-ports'
        with ports.open('a') as out:out.write(str(sock.getsockname()[1])+'\n')
        ports.chmod(0o600)
        print(json.dumps({'phase':'started'}),flush=True)
        print(json.dumps({'phase':'ready','port':sock.getsockname()[1]}),flush=True)
    sys.stdin.read()
if sock:sock.close()
receipt['complete']=True
path.write_text(json.dumps(receipt))
print(json.dumps('complete' if tunnel else {'phase':'complete','success':True}),flush=True)
";
    let worker = root.join("synthetic-guardian.py");
    std::fs::write(&worker, script).unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    let state = root.join("local");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = Arc::clone(&fixture.workspace);
    let local = Arc::new(
        Local::new(
            Arc::clone(&workspace),
            &account(),
            &root,
            Configuration {
                process_worker: worker.clone(),
                tunnel_worker: worker.clone(),
                tunnel_binary: worker,
                tunnel_sha256: Sha256::digest(script.as_bytes())
                    .iter()
                    .fold(String::new(), |mut text, byte| {
                        let _ = write!(text, "{byte:02x}");
                        text
                    }),
                state,
            },
        )
        .unwrap(),
    );
    let mut contract = Contract::from_agents("```yaml\nremote-device-testing:\n  version: 1\n  provider: browserstack\n  max_parallel: 2\n  apps:\n    ios:\n      build: [build]\n      artifact: App.ipa\n      bundle_id: com.example.app\n  matrix: [{platform: ios, form: phone}]\n  recipes: [recipe.md]\n```").unwrap();
    contract.tunnel.ports.insert(
        "backend".into(),
        Port::Managed(DeclaredBackend {
            start: vec!["synthetic".into()],
            timeout_seconds: 2,
        }),
    );
    contract.launch_arguments.insert("BASE_URL".into(), base_url.into());
    std::fs::write(root.join("App.ipa"), b"PK\x03\x04synthetic immutable app").unwrap();
    std::fs::write(root.join("recipe.md"), "```yaml\ndevice-recipe:\n  version: 1\n  id: smoke\n  steps:\n    - id: home\n      action: assert\n      target: {by: identifier, value: menu.open}\n      state: visible\n```\n").unwrap();
    let matrix = vec![ResolvedDevice {
        matrix_index: 0,
        device: Device {
            platform: Platform::Ios,
            form: Form::Phone,
            model: "iPhone synthetic".into(),
            os_version: "27.0".into(),
        },
    }];
    let actor = Actor::from_backend(workspace, fixture.fake.clone(), local, contract, matrix);
    (fixture, actor)
}

#[test]
fn unchanged_upload_has_independent_caller_handles_and_keeps_original_deadline() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let first = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let original = actor.uploaded(first.id).unwrap().lock().unwrap().deadline;
    let second = actor.upload(Platform::Ios, Duration::from_mins(30)).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(fixture.fake.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(actor.uploaded(second.id).unwrap().lock().unwrap().deadline, original);
    actor.release_upload(first.id).unwrap();
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 0);
    let session = actor.create(0, second.id, Duration::from_secs(20)).unwrap();
    actor.release_upload(second.id).unwrap();
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 0);
    actor.close(session.id).unwrap();
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 1);
}

#[test]
fn lost_native_delete_blocks_actions_and_reset_but_keeps_other_lane_live() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let first = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    let second = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    fixture.fake.transport.lost_delete.store(true, Ordering::SeqCst);
    assert_eq!(actor.close(first.id), Err(Error::CleanupUncertain));
    let attempts = fixture.fake.transport.creates.load(Ordering::SeqCst);
    assert!(actor.act(first.id, &Action::Home {}).is_err());
    assert!(actor.reset(first.id).is_err());
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), attempts);
    assert!(!actor.snapshot(second.id).unwrap().nodes.is_empty());
    assert!(matches!(
        actor.create(0, app.id, Duration::from_secs(20)),
        Err(Error::AdmissionDeferred)
    ));
    fixture.fake.transport.lost_delete.store(false, Ordering::SeqCst);
    actor.close(first.id).unwrap();
    actor.close(second.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn concurrent_reset_replaces_old_handle_once_and_does_not_renew_deadline() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let old = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    let until = actor.lane(old.id).unwrap().lock().unwrap().deadline;
    actor.release_upload(app.id).unwrap();
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| actor.reset(old.id));
        let b = scope.spawn(|| actor.reset(old.id));
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 2);
    let fresh = results.into_iter().find_map(Result::ok).unwrap();
    assert_ne!(fresh.id, old.id);
    assert_eq!(actor.lane(fresh.id).unwrap().lock().unwrap().deadline, until);
    assert!(actor.snapshot(old.id).is_err());
    actor.close(fresh.id).unwrap();
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 1);
}

#[test]
fn native_lane_deadline_cannot_extend_upload_and_expired_admission_never_posts() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let upload_deadline = actor.uploaded(app.id).unwrap().lock().unwrap().deadline;
    let lane = actor.create(0, app.id, Duration::from_mins(30)).unwrap();
    assert_eq!(actor.lane(lane.id).unwrap().lock().unwrap().deadline, upload_deadline);
    assert!(
        actor
            .create_until(0, app.id, Instant::now().checked_sub(Duration::from_secs(1)).unwrap())
            .is_err()
    );
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 1);
    actor.close(lane.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn lost_upload_reply_is_not_replayed_even_if_marking_uncertain_fails() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let ledger = std::fs::read_dir(fixture.root.path().join("private"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("journal.json").is_file())
        .unwrap()
        .join("journal.json");
    fixture.fake.lost_upload.store(true, Ordering::SeqCst);
    let saved = ledger.with_extension("saved");
    let original = ledger.clone();
    let backup = saved.clone();
    *fixture.fake.after_upload.lock().unwrap() = Some(Box::new(move || {
        std::fs::rename(original, backup).unwrap();
    }));
    // Remove persistence after dispatch, before uncertainty can be recorded.
    assert!(actor.upload(Platform::Ios, Duration::from_secs(30)).is_err());
    assert!(actor.upload(Platform::Ios, Duration::from_secs(30)).is_err());
    std::fs::rename(&saved, &ledger).unwrap();
    assert_eq!(
        actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap_err(),
        Error::CleanupUncertain
    );
    assert_eq!(fixture.fake.uploads.load(Ordering::SeqCst), 1);
}

#[test]
fn expired_lane_cleanup_releases_its_exact_resources_without_subsecond_setup_assumptions() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let session = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    // Shorten the retained test lane only after all setup/receipt writes are complete.
    actor.lane(session.id).unwrap().lock().unwrap().deadline =
        Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    actor.expire().unwrap();
    let by = Instant::now() + Duration::from_secs(2);
    while fixture.fake.transport.deletes.load(Ordering::SeqCst) == 0 && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fixture.fake.transport.deletes.load(Ordering::SeqCst), 1);
    assert!(actor.snapshot(session.id).is_err());
    actor.close(session.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn durable_uncertainty_blocks_actions_and_upload_reuse_without_new_posts() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let session = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    fixture
        .workspace
        .journal()
        .uncertain(fixture.workspace.owner(), session.id)
        .unwrap();
    assert!(actor.snapshot(session.id).is_err());
    let upload = actor.uploaded(app.id).unwrap().lock().unwrap().id;
    fixture
        .workspace
        .journal()
        .uncertain(fixture.workspace.owner(), upload)
        .unwrap();
    assert!(actor.upload(Platform::Ios, Duration::from_secs(30)).is_err());
    assert!(actor.create(0, app.id, Duration::from_secs(20)).is_err());
    assert_eq!(fixture.fake.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 1);
    actor.close(session.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn acknowledged_upload_delete_with_failed_receipt_cannot_be_uploaded_again() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let ledger = std::fs::read_dir(fixture.root.path().join("private"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("journal.json").is_file())
        .unwrap()
        .join("journal.json");
    let backup = ledger.with_extension("saved");
    std::fs::rename(&ledger, &backup).unwrap();
    assert_eq!(actor.release_upload(app.id), Err(Error::CleanupUncertain));
    std::fs::rename(backup, ledger).unwrap();
    assert_eq!(
        actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap_err(),
        Error::CleanupUncertain
    );
    actor.release_upload(app.id).unwrap();
    assert_eq!(fixture.fake.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 1);
}

#[test]
fn remote_quota_wait_does_not_block_an_existing_lane_or_its_cleanup() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let first = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    let (entered, wait) = std::sync::mpsc::channel();
    let (resume, blocked) = std::sync::mpsc::channel();
    *fixture.fake.before_capacity.lock().unwrap() = Some(Box::new(move || {
        entered.send(()).unwrap();
        blocked.recv_timeout(Duration::from_secs(3)).unwrap();
    }));
    std::thread::scope(|scope| {
        let preparing = scope.spawn(|| actor.create(0, app.id, Duration::from_secs(20)));
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!actor.snapshot(first.id).unwrap().nodes.is_empty());
        actor.close(first.id).unwrap();
        resume.send(()).unwrap();
        actor.close(preparing.join().unwrap().unwrap().id).unwrap();
    });
    actor.release_upload(app.id).unwrap();
}

#[test]
fn slow_expired_delete_does_not_delay_a_later_lane_deadline() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let first = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    let second = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    *fixture.fake.transport.pause.lock().unwrap() = Some("synthetic_session_0000000000000001".into());
    actor.lane(first.id).unwrap().lock().unwrap().deadline =
        Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    actor.lane(second.id).unwrap().lock().unwrap().deadline = Instant::now() + Duration::from_millis(60);
    actor.expire().unwrap();
    let by = Instant::now() + Duration::from_secs(2);
    while !fixture.fake.transport.entered.load(Ordering::Acquire) && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(fixture.fake.transport.entered.load(Ordering::Acquire));
    while fixture.fake.transport.deletes.load(Ordering::SeqCst) == 0 && Instant::now() < by {
        actor.expire().unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fixture.fake.transport.deletes.load(Ordering::SeqCst),
        1,
        "second lane must close before blocked first DELETE returns"
    );
    fixture.fake.transport.resume.store(true, Ordering::Release);
    actor.close(first.id).unwrap();
    actor.close(second.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn parallel_controller_lanes_own_distinct_live_backend_ports_and_drop_closes_both() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let (first, second) = std::thread::scope(|scope| {
        let a = scope.spawn(|| actor.create(0, app.id, Duration::from_secs(20)).unwrap());
        let b = scope.spawn(|| actor.create(0, app.id, Duration::from_secs(20)).unwrap());
        (a.join().unwrap(), b.join().unwrap())
    });
    let ports: Vec<u16> = std::fs::read_to_string(fixture.root.path().join("backend-ports"))
        .unwrap()
        .lines()
        .map(|line| line.parse().unwrap())
        .collect();
    assert_eq!(ports.len(), 2);
    assert_ne!(ports[0], ports[1]);
    assert!(
        ports
            .iter()
            .all(|port| std::net::TcpStream::connect(("127.0.0.1", *port)).is_ok())
    );
    assert!(actor.create(0, app.id, Duration::from_secs(20)).is_err());
    assert_ne!(first.id, second.id);
    drop(actor);
    assert!(
        ports
            .iter()
            .all(|port| std::net::TcpStream::connect(("127.0.0.1", *port)).is_err())
    );
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
fn poisoned_upload_cleanup_retains_ownership_and_never_replays_upload() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    fixture.fake.panic_delete.store(true, Ordering::SeqCst);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| actor.release_upload(app.id)));
    assert!(panic.is_err());
    fixture.fake.panic_delete.store(false, Ordering::SeqCst);
    assert!(actor.upload(Platform::Ios, Duration::from_secs(30)).is_err());
    assert_eq!(fixture.fake.uploads.load(Ordering::SeqCst), 1);
    assert_eq!(actor.uploads.lock().unwrap().len(), 1);
}

#[test]
fn upload_only_shutdown_retries_exact_failed_delete_at_original_deadline() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let until = Instant::now() + Duration::from_millis(150);
    actor.uploaded(app.id).unwrap().lock().unwrap().deadline = until;
    fixture.fake.lost_upload_delete_once.store(true, Ordering::SeqCst);
    drop(actor);
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 1);
    let by = Instant::now() + Duration::from_secs(2);
    while !fixture
        .workspace
        .journal()
        .pending(fixture.workspace.owner())
        .unwrap()
        .is_empty()
        && Instant::now() < by
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 2);
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
fn poisoned_native_delete_shutdown_keeps_backend_until_its_original_deadline() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let session = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    let until = Instant::now() + Duration::from_millis(150);
    actor.lane(session.id).unwrap().lock().unwrap().deadline = until;
    actor.uploaded(app.id).unwrap().lock().unwrap().deadline = until;
    let port: u16 = std::fs::read_to_string(fixture.root.path().join("backend-ports"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    fixture.fake.transport.panic_native_delete.store(true, Ordering::SeqCst);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| actor.close(session.id))).is_err());
    fixture
        .fake
        .transport
        .panic_native_delete
        .store(false, Ordering::SeqCst);
    drop(actor);
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "unconfirmed native cleanup must not stop backend early"
    );
    let by = Instant::now() + Duration::from_secs(2);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() && Instant::now() < by {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    assert!(
        !fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty(),
        "unconfirmed native allocation remains durably held"
    );
}

#[test]
fn app_launch_routes_through_the_exact_started_tunnel_alias() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let session = actor.create(0, app.id, Duration::from_secs(20)).unwrap();
    let tunnel = actor.tunnel_status(session.id).unwrap();
    let active = fixture.fake.transport.active.lock().unwrap();
    let caps = &active.values().next().unwrap()["capabilities"]["alwaysMatch"]["bstack:options"];
    assert_eq!(
        caps["localIdentifier"],
        horizon_app_provider::tunnel::local_identifier(tunnel.id)
    );
    drop(active);
    actor.close(session.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn lost_native_delete_reply_requires_positive_provider_termination_before_releasing_lane() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let artifact = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let session = actor.create(0, artifact.id, Duration::from_secs(30)).unwrap();
    fixture.fake.transport.lost_delete.store(true, Ordering::SeqCst);
    assert!(actor.close(session.id).is_err());
    assert_eq!(
        fixture
            .workspace
            .journal()
            .status(fixture.workspace.owner(), session.id)
            .unwrap()
            .phase,
        Phase::Uncertain
    );
    // The fake provider positively publishes termination, independent of the failed DELETE response.
    fixture.fake.transport.active.lock().unwrap().clear();
    actor.close(session.id).unwrap();
    assert_eq!(
        fixture
            .workspace
            .journal()
            .status(fixture.workspace.owner(), session.id)
            .unwrap()
            .phase,
        Phase::Complete
    );
    actor.release_upload(artifact.id).unwrap();
}

#[test]
fn contended_dispatch_gate_cannot_send_a_post_after_the_original_deadline() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let lock_path = fixture.root.path().join("private/browserstack/journal.lock");
    let held = Arc::new(Mutex::new(None));
    let worker = Arc::clone(&held);
    *fixture.fake.before_driver_return.lock().unwrap() = Some(Box::new(move || {
        let (send, receive) = std::sync::mpsc::channel();
        *worker.lock().unwrap() = Some(std::thread::spawn(move || {
            let file = std::fs::File::open(lock_path).unwrap();
            file.lock().unwrap();
            send.send(()).unwrap();
            std::thread::sleep(Duration::from_secs(11));
            file.unlock().unwrap();
        }));
        receive.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    assert!(actor.create(0, app.id, Duration::from_secs(10)).is_err());
    assert_eq!(fixture.fake.transport.creates.load(Ordering::SeqCst), 0);
    held.lock().unwrap().take().unwrap().join().unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn in_budget_dispatch_contention_reduces_the_provider_request_timeout() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(30)).unwrap();
    let lock_path = fixture.root.path().join("private/browserstack/journal.lock");
    let held = Arc::new(Mutex::new(None));
    let worker = Arc::clone(&held);
    *fixture.fake.before_driver_return.lock().unwrap() = Some(Box::new(move || {
        let (send, receive) = std::sync::mpsc::channel();
        *worker.lock().unwrap() = Some(std::thread::spawn(move || {
            let file = std::fs::File::open(lock_path).unwrap();
            file.lock().unwrap();
            send.send(()).unwrap();
            std::thread::sleep(Duration::from_secs(1));
            file.unlock().unwrap();
        }));
        receive.recv_timeout(Duration::from_secs(1)).unwrap();
    }));
    *fixture.fake.transport.after_create.lock().unwrap() = Some(Box::new(|| {
        std::thread::sleep(Duration::from_millis(400));
    }));
    let deadline = Instant::now() + Duration::from_secs(10);
    let session = actor.create_until(0, app.id, deadline).unwrap();
    let (budget, dispatched) = fixture.fake.transport.allocation_timeouts.lock().unwrap()[0];
    assert!(
        budget <= Duration::from_secs(9),
        "the held gate must consume at least one second"
    );
    assert!(
        budget <= deadline.saturating_duration_since(dispatched) + Duration::from_millis(250),
        "provider budget must include the time spent waiting on the durable gate"
    );
    held.lock().unwrap().take().unwrap().join().unwrap();
    actor.close(session.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn media_admission_covers_blocked_exports_and_callback_errors_release_it() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let app = actor.upload(Platform::Ios, Duration::from_secs(60)).unwrap();
    let session = actor.create(0, app.id, Duration::from_secs(60)).unwrap();
    let entered = std::sync::Barrier::new(3);
    let release = std::sync::Barrier::new(3);
    let kind = horizon_app_provider::media::Kind::Video;
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..2 {
            workers.push(scope.spawn(|| {
                actor.export_media(session.id, kind, Duration::from_secs(5), |_| {
                    entered.wait();
                    release.wait();
                    Err::<(), _>(Error::Unavailable)
                })
            }));
        }
        entered.wait();
        assert_eq!(fixture.fake.media_calls.load(Ordering::SeqCst), 2);
        assert_eq!(actor.media(session.id, kind), Err(Error::MediaBusy));
        assert_eq!(fixture.fake.media_calls.load(Ordering::SeqCst), 2);
        release.wait();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), Err(Error::Unavailable));
        }
    });
    actor.media(session.id, kind).unwrap();
    assert_eq!(fixture.fake.media_calls.load(Ordering::SeqCst), 3);
    actor.close(session.id).unwrap();
    actor.release_upload(app.id).unwrap();
}

#[test]
fn explicit_host_shutdown_closes_native_resources_while_other_arc_owners_remain() {
    let (fixture, actor) = actor("http://localhost:{tunnel.port.backend}");
    let actor = Arc::new(actor);
    let retained_capture = Arc::clone(&actor);
    let app = actor.upload(Platform::Ios, Duration::from_secs(60)).unwrap();
    let session = actor.create(0, app.id, Duration::from_secs(60)).unwrap();
    actor.shutdown().unwrap();
    assert_eq!(fixture.fake.transport.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.fake.upload_deletes.load(Ordering::SeqCst), 1);
    assert!(
        fixture
            .workspace
            .journal()
            .pending(fixture.workspace.owner())
            .unwrap()
            .is_empty()
    );
    assert!(retained_capture.screenshot(session.id).is_err());
    assert!(matches!(
        actor.upload(Platform::Ios, Duration::from_secs(60)),
        Err(Error::Cancelled)
    ));
    actor.shutdown().unwrap();
    assert_eq!(fixture.fake.transport.deletes.load(Ordering::SeqCst), 1);
}
