use super::*;
use crate::app::test_support::test_app;
use horizon_core::cloud_runtime::{
    registry::{Action, Scope, State as RegistryState, Status, Validation},
    settings::Settings,
};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

struct HistoryFixture {
    app: HorizonApp,
    ctx: egui::Context,
    settings: Settings,
    repository: String,
    generation: String,
    account: String,
    provider_path: PathBuf,
    host_path: PathBuf,
    host: Validation,
    provider: Validation,
    root: tempfile::TempDir,
}

impl HistoryFixture {
    fn new() -> Self {
        use sha2::{Digest as _, Sha256};
        use std::os::unix::fs::PermissionsExt;
        let (root, mut app) = test_app();
        let draft = bound_registry_draft(&root.path().join("cloud"));
        let settings = draft.settings.clone();
        let config = settings.registries.as_ref().unwrap();
        let binding = &config.bindings[0];
        let hosts = config.root.join("hosts");
        std::fs::create_dir_all(&hosts).unwrap();
        for path in [&config.root, &hosts] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut host = validation();
        host.checked_at = 10;
        host.image = "registry.example/team/worker@sha256:host".into();
        let mut provider = validation();
        provider.checked_at = 20;
        provider.image = "registry.example/team/worker@sha256:provider".into();
        let account = Sha256::digest(b"synthetic-compute")
            .iter()
            .fold(String::new(), |mut text, byte| {
                use std::fmt::Write as _;
                write!(&mut text, "{byte:02x}").unwrap();
                text
            });
        let repository = binding.repository.clone();
        let generation = binding.generation.clone();
        let provider_path = config.root.join(format!("{generation}.json"));
        let host_path = hosts.join(format!("{generation}.json"));
        app.cloud_prototype.production.setup = State {
            draft: Some(Box::new(draft)),
            ..State::default()
        };
        let fixture = Self {
            app,
            ctx: egui::Context::default(),
            settings,
            repository,
            generation,
            account,
            provider_path,
            host_path,
            host,
            provider,
            root,
        };
        fixture.save_host();
        fixture.save_provider(&Self::active());
        fixture
    }

    fn active() -> RegistryState {
        serde_json::from_value(serde_json::json!({"Bound":{"id":"synthetic-binding","name":"synthetic"}})).unwrap()
    }

    fn write_record(&self, path: &Path, generation: &str, account: &str, state: &RegistryState, check: &Validation) {
        let value = serde_json::json!({"repository":self.repository,"generation":generation,
            "account":account,"fingerprint":null,"state":state,"validation":check});
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn save_host(&self) {
        self.write_record(
            &self.host_path,
            &self.generation,
            "direct-host",
            &RegistryState::Prepared,
            &self.host,
        );
    }

    fn save_provider(&self, state: &RegistryState) {
        self.write_record(
            &self.provider_path,
            &self.generation,
            &self.account,
            state,
            &self.provider,
        );
    }

    fn state(&self) -> &State {
        &self.app.cloud_prototype.production.setup
    }

    fn act(&mut self, action: Action) {
        let saved = self.state().draft.as_ref().unwrap().registries[0]
            .original
            .as_ref()
            .unwrap();
        let request = registry::Request::for_binding(saved, action);
        self.app.manage_cloud_registry(&self.ctx, request);
        wait(&mut self.app, &self.ctx);
    }

    fn provider_status(&mut self) {
        self.act(Action::Status {
            repository: self.repository.clone(),
            generation: self.generation.clone(),
        });
    }

    fn host_status(&mut self) {
        self.act(Action::PullStatus {
            repository: self.repository.clone(),
            generation: self.generation.clone(),
        });
    }

    fn record_fresh_provider(&mut self) {
        let status = Status {
            scope: Scope::Provider,
            repository: self.repository.clone(),
            generation: self.generation.clone(),
            state: Self::active(),
            validation: Some(self.provider.clone()),
            configured_pull_expiry: None,
        };
        self.app
            .cloud_prototype
            .production
            .setup
            .record_registry(registry_outcome(
                &status,
                &Action::Verify {
                    image: self.provider.image.clone(),
                },
            ));
    }

    fn lock_host(&self) -> File {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.host_path.with_extension("lock"))
            .unwrap();
        file.lock().unwrap();
        file
    }

    fn assert_unknown_history(&self) {
        assert!(self.state().error.is_none());
        assert!(!self.state().verified.contains_key(&self.repository));
        assert!(
            self.state()
                .registry_status
                .as_deref()
                .unwrap()
                .contains("Host pull history is unavailable")
        );
    }
}

#[test]
fn cached_host_history_never_opens_provider_history() {
    let mut f = HistoryFixture::new();
    assert_eq!(
        cached_pull_validation(&f.settings, &f.repository, &f.generation)
            .unwrap()
            .unwrap()
            .image,
        f.host.image
    );
    assert!(!f.provider_path.with_extension("lock").exists());
    f.save_provider(&RegistryState::Prepared);
    f.provider_status();
    assert_eq!(f.state().verified[&f.repository].image, f.host.image);
    assert!(
        f.state()
            .registry_status
            .as_deref()
            .unwrap()
            .starts_with("Provider registry access: Not prepared")
    );
}

#[test]
fn provider_status_preserves_its_active_proof_when_host_history_is_empty() {
    let mut f = HistoryFixture::new();
    std::fs::remove_file(&f.host_path).unwrap();
    f.record_fresh_provider();
    f.provider_status();
    assert!(f.state().error.is_none());
    assert_eq!(f.state().verified[&f.repository].image, f.provider.image);
    assert!(
        f.state()
            .registry_status
            .as_deref()
            .unwrap()
            .starts_with("Provider registry access: Active")
    );
    assert!(!f.host_path.exists(), "provider history does not become a host record");
}

#[test]
fn locked_or_corrupt_host_history_never_uses_a_provider_success() {
    let mut f = HistoryFixture::new();
    std::fs::remove_file(&f.host_path).unwrap();
    f.record_fresh_provider();
    let lock = f.lock_host();
    f.provider_status();
    f.assert_unknown_history();
    lock.unlock().unwrap();
    std::fs::write(&f.host_path, b"malformed host history").unwrap();
    f.provider_status();
    f.assert_unknown_history();
}

#[test]
fn failed_host_status_clears_the_hint_and_a_readable_status_recovers_it() {
    let mut f = HistoryFixture::new();
    f.host_status();
    assert!(f.state().verified.contains_key(&f.repository));
    std::fs::write(&f.host_path, b"malformed host history").unwrap();
    f.host_status();
    assert!(f.state().error.is_some());
    assert!(!f.state().verified.contains_key(&f.repository));
    f.save_host();
    f.host_status();
    assert_eq!(f.state().verified[&f.repository].image, f.host.image);
}

#[test]
fn revoking_an_already_revoked_previous_generation_keeps_the_current_hint() {
    let mut f = HistoryFixture::new();
    f.host_status();
    let previous = "previous-generation";
    let draft = f.app.cloud_prototype.production.setup.draft.as_mut().unwrap();
    draft.settings.registries.as_mut().unwrap().bindings[0]
        .retired
        .push(previous.into());
    draft.registries[0]
        .original
        .as_mut()
        .unwrap()
        .retired
        .push(previous.into());
    f.write_record(
        &f.provider_path.with_file_name(format!("{previous}.json")),
        previous,
        &f.account,
        &RegistryState::Revoked,
        &f.provider,
    );
    f.act(Action::Revoke {
        repository: f.repository.clone(),
        generation: previous.into(),
    });
    assert!(f.state().error.is_none());
    assert_eq!(f.state().verified[&f.repository].image, f.host.image);
}

#[test]
fn a_host_pull_status_is_allowed_with_an_unsaved_compute_key() {
    let (temp, mut app) = test_app();
    let root = temp.path().join("cloud");
    std::fs::create_dir(&root).unwrap();
    let settings = serde_json::json!({
        "runpod_key_file":root.join("absent-compute"),"ssh_identity_file":root.join("absent-identity"),
        "docker_config":root.join("docker"),"registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
        "registries":{"root":root.join("registry"),"bindings":[{
            "repository":"registry.example/team/worker","generation":"synthetic-generation",
            "publish":null,"pull":{"username":"reader","secret_file":root.join("absent-pull"),"expires_at":null},
            "read_only_confirmed":true,"retired":[]
        }]}
    });
    std::fs::write(root.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
    app.cloud_prototype.root = Some(root.clone());
    let ctx = egui::Context::default();
    app.open_cloud_accounts(&ctx, false);
    wait(&mut app, &ctx);
    *app.cloud_prototype.production.setup.draft.as_mut().unwrap().runpod_key = "synthetic-unsaved".into();
    app.manage_cloud_registry(
        &ctx,
        registry::Request::for_binding(
            app.cloud_prototype.production.setup.draft.as_ref().unwrap().registries[0]
                .original
                .as_ref()
                .unwrap(),
            Action::PullStatus {
                repository: "registry.example/team/worker".into(),
                generation: "synthetic-generation".into(),
            },
        ),
    );
    assert!(app.cloud_prototype.production.setup.receiver.is_some());
    wait(&mut app, &ctx);
    let state = &app.cloud_prototype.production.setup;
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(
        state
            .registry_status
            .as_deref()
            .unwrap()
            .starts_with("Host pull access: Not checked")
    );
    assert!(!root.join("registry/synthetic-generation.json").exists());
}

#[test]
fn an_unreadable_host_journal_does_not_fall_back_to_provider_access() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let key = root.path().join("compute");
    std::fs::write(&key, "synthetic-compute").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let registry = root.path().join("registry");
    let hosts = registry.join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    for path in [&registry, &hosts] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(hosts.join("synthetic-generation.json"), "not-json").unwrap();
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":key,"ssh_identity_file":root.path().join("identity"),"docker_config":root.path().join("docker"),
        "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
        "registries":{"root":registry,"bindings":[{
            "repository":"registry.example/team/worker","generation":"synthetic-generation","publish":null,
            "pull":{"username":"reader","secret_file":root.path().join("absent-pull"),"expires_at":null},
            "read_only_confirmed":true,"retired":[]
        }]}
    })).unwrap();
    assert!(cached_pull_validation(&settings, "registry.example/team/worker", "synthetic-generation").is_err());
    assert!(
        !registry.join("synthetic-generation.lock").exists(),
        "provider status was never opened"
    );
}

#[test]
fn status_cannot_restore_a_refused_pull_but_a_fresh_check_can() {
    use horizon_core::cloud_runtime::registry::{Action, Scope, State as RegistryState, Status};
    let (temp, mut app) = test_app();
    let draft = bound_registry_draft(&temp.path().join("cloud"));
    let binding = draft.registries[0].original.clone().unwrap();
    let ctx = egui::Context::default();
    app.cloud_prototype.production.setup = State {
        draft: Some(Box::new(draft)),
        ..State::default()
    };
    app.cloud_prototype
        .production
        .setup
        .verified
        .insert(binding.repository.clone(), validation());
    app.manage_cloud_registry(
        &ctx,
        registry::Request::for_binding(
            &binding,
            Action::VerifyPull {
                image: format!("{}:mutable", binding.repository),
            },
        ),
    );
    wait(&mut app, &ctx);
    let state = &mut app.cloud_prototype.production.setup;
    assert!(state.error.as_deref().unwrap().contains("immutable image"));
    assert!(!state.verified.contains_key(&binding.repository));
    let status = Status {
        scope: Scope::Host,
        repository: binding.repository.clone(),
        generation: binding.generation.clone(),
        state: RegistryState::Prepared,
        validation: Some(validation()),
        configured_pull_expiry: None,
    };
    let historical = Action::PullStatus {
        repository: binding.repository.clone(),
        generation: binding.generation.clone(),
    };
    state.record_registry(registry_outcome(&status, &historical));
    assert!(
        !state.verified.contains_key(&binding.repository),
        "metadata cannot undo a refused verification"
    );
    let verified = Action::VerifyPull {
        image: validation().image,
    };
    state.record_registry(registry_outcome(&status, &verified));
    assert!(state.verified.contains_key(&binding.repository));
}

#[test]
fn a_fresh_check_shows_its_own_image_instead_of_older_other_scope_history() {
    use horizon_core::cloud_runtime::registry::{Action, Scope, State as RegistryState, Status};
    let temp = tempfile::tempdir().unwrap();
    let draft = bound_registry_draft(temp.path());
    let binding = draft.registries[0].original.clone().unwrap();
    let mut state = State {
        draft: Some(Box::new(draft)),
        ..State::default()
    };
    state.verified.insert(binding.repository.clone(), validation());
    let mut checked = validation();
    checked.image = format!("{}@sha256:new", binding.repository);
    let status = Status {
        scope: Scope::Host,
        repository: binding.repository.clone(),
        generation: binding.generation,
        state: RegistryState::Prepared,
        validation: Some(checked.clone()),
        configured_pull_expiry: None,
    };
    let action = Action::VerifyPull {
        image: checked.image.clone(),
    };
    state.record_registry(registry_outcome(&status, &action));
    assert_eq!(state.verified[&binding.repository].image, checked.image);
    assert!(state.registry_status.as_deref().unwrap().contains(&checked.image));
}

#[test]
fn closing_and_reopening_settings_cannot_restore_a_refused_pull_hint() {
    let mut f = HistoryFixture::new();
    f.app.cloud_prototype.root = Some(f.root.path().join("cloud"));
    f.host_status();
    assert!(f.state().verified.contains_key(&f.repository));
    f.act(Action::VerifyPull {
        image: format!("{}:latest", f.repository),
    });
    assert!(f.state().error.as_deref().unwrap().contains("immutable image"));
    assert!(!f.state().verified.contains_key(&f.repository));
    f.app.cancel_cloud_accounts();
    f.app.open_cloud_accounts(&f.ctx, false);
    wait(&mut f.app, &f.ctx);
    assert!(!f.state().verified.contains_key(&f.repository));
    f.host_status();
    assert!(!f.state().verified.contains_key(&f.repository));
    let status = Status {
        scope: Scope::Host,
        repository: f.repository.clone(),
        generation: f.generation.clone(),
        state: RegistryState::Prepared,
        validation: Some(f.host.clone()),
        configured_pull_expiry: None,
    };
    f.app.cloud_prototype.production.setup.record_registry(registry_outcome(
        &status,
        &Action::VerifyPull {
            image: f.host.image.clone(),
        },
    ));
    assert!(f.state().verified.contains_key(&f.repository));
    f.app.cancel_cloud_accounts();
    f.app.open_cloud_accounts(&f.ctx, false);
    wait(&mut f.app, &f.ctx);
    assert!(f.state().verified.contains_key(&f.repository));
}

#[test]
fn session_switch_cannot_restore_a_refused_pull_hint() {
    let mut f = HistoryFixture::new();
    f.app.cloud_prototype.root = Some(f.root.path().join("cloud"));
    f.host_status();
    f.act(Action::VerifyPull {
        image: format!("{}:latest", f.repository),
    });
    assert!(!f.state().verified.contains_key(&f.repository));
    f.app.cloud_prototype.production.setup.open = true;
    f.app.cloud_prototype.production.setup.continuation = Continuation::StartWhenReady;
    f.app.close_cloud_creation_for_session_switch();
    assert!(!f.state().open);
    f.app.open_cloud_accounts(&f.ctx, false);
    wait(&mut f.app, &f.ctx);
    assert!(
        !f.state().verified.contains_key(&f.repository),
        "session switch must retain the refusal"
    );
    f.host_status();
    assert!(!f.state().verified.contains_key(&f.repository));
}

#[test]
fn malformed_image_cannot_leave_the_selected_binding_verified() {
    let mut f = HistoryFixture::new();
    f.app.cloud_prototype.root = Some(f.root.path().join("cloud"));
    f.host_status();
    assert!(f.state().verified.contains_key(&f.repository));
    f.act(Action::VerifyPull {
        image: format!("{}@sha256:0", f.repository),
    });
    assert!(f.state().error.is_some());
    assert!(
        !f.state().verified.contains_key(&f.repository),
        "invalid image must invalidate the selected binding"
    );
    f.app.cancel_cloud_accounts();
    f.app.open_cloud_accounts(&f.ctx, false);
    wait(&mut f.app, &f.ctx);
    assert!(!f.state().verified.contains_key(&f.repository));
    f.host_status();
    assert!(!f.state().verified.contains_key(&f.repository));
}

#[test]
fn image_from_another_saved_binding_cannot_validate_the_selected_card() {
    let mut f = HistoryFixture::new();
    f.host_status();
    let draft = f.app.cloud_prototype.production.setup.draft.as_mut().unwrap();
    let mut other = draft.registries[0].original.clone().unwrap();
    other.repository = "registry.example/other/worker".into();
    other.generation = "other-generation".into();
    draft.settings.registries.as_mut().unwrap().bindings.push(other.clone());
    f.act(Action::VerifyPull {
        image: format!("{}@sha256:{}", other.repository, "a".repeat(64)),
    });
    assert!(f.state().error.as_deref().unwrap().contains("this saved repository"));
    assert!(f.state().receiver.is_none());
    assert!(!f.state().verified.contains_key(&f.repository));
    assert!(!f.host_path.with_file_name("other-generation.json").exists());
}

#[test]
fn a_stale_validation_request_cannot_invalidate_a_new_grant() {
    let mut f = HistoryFixture::new();
    f.host_status();
    let mut saved = f.state().draft.as_ref().unwrap().registries[0]
        .original
        .clone()
        .unwrap();
    saved.generation = "retired-generation".into();
    let request = registry::Request::for_binding(
        &saved,
        Action::VerifyPull {
            image: "malformed".into(),
        },
    );
    f.app.manage_cloud_registry(&f.ctx, request);
    assert!(f.state().error.as_deref().unwrap().contains("saved pull grant changed"));
    assert!(f.state().receiver.is_none());
    assert!(f.state().verified.contains_key(&f.repository));
    assert!(!f.state().refused_pulls.contains(&f.repository));
}

#[test]
fn worker_refusal_is_retained_before_settings_load_can_fail_or_be_replaced() {
    for failed_load in [true, false] {
        let mut f = HistoryFixture::new();
        let root = f.root.path().join("cloud");
        f.app.cloud_prototype.root = Some(root.clone());
        let path = root.join("settings.json");
        let original = std::fs::read(&path).unwrap();
        if failed_load {
            std::fs::write(&path, b"invalid settings").unwrap();
        }
        let image = format!("{}@sha256:{}", f.repository, "a".repeat(64));
        f.app.open_cloud_settings(&f.ctx, false, Some((image, true)));
        assert!(
            f.state().refused_pulls.contains(&f.repository),
            "record refusal before spawning a loader"
        );
        if failed_load {
            wait(&mut f.app, &f.ctx);
            assert!(f.state().error.is_some());
            std::fs::write(&path, &original).unwrap();
        } else {
            f.app.cancel_cloud_accounts();
        }
        f.app.open_cloud_accounts(&f.ctx, false);
        wait(&mut f.app, &f.ctx);
        assert!(!f.state().verified.contains_key(&f.repository));
        f.host_status();
        assert!(!f.state().verified.contains_key(&f.repository));
    }
}
