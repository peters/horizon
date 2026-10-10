use super::*;
use crate::cloud_runtime::{CreateState, Stage, state::Deployment};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

struct Fixture {
    root: tempfile::TempDir,
    deleted: AtomicUsize,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("tailnets.json"),
            br#"{"tailnets":[{"id":"work","name":"Work"},{"id":"other","name":"Other"}]}"#,
        )
        .unwrap();
        Self {
            root,
            deleted: AtomicUsize::new(0),
        }
    }

    fn cloud(&self, name: &str, selected: Option<&str>) -> PathBuf {
        let cloud = self.root.path().join(name);
        let _held = state::Store::lock(&cloud).unwrap();
        Selection::save(&cloud, selected, &store(self.root.path()).load().unwrap()).unwrap();
        cloud
    }

    fn remove(&self, checkpoint: impl FnMut(Boundary) -> Result<()>) -> Result<Catalog> {
        remove_with(self.root.path(), "work", checkpoint, |ownership| {
            self.deleted.fetch_add(1, Ordering::SeqCst);
            assert!(
                ownership
                    .load()
                    .unwrap()
                    .tailnets
                    .iter()
                    .any(|network| network.id == "work")
            );
            // The synthetic credential sink cannot access any OS credential store.
            std::fs::write(
                self.root.path().join("tailnets.json"),
                br#"{"tailnets":[{"id":"other","name":"Other"}]}"#,
            )?;
            Ok(Catalog { tailnets: vec![] })
        })
    }

    fn retained(&self) {
        assert_eq!(self.deleted.load(Ordering::SeqCst), 0);
        assert!(
            store(self.root.path())
                .load()
                .unwrap()
                .tailnets
                .iter()
                .any(|network| network.id == "work")
        );
    }
}

fn deployment(cloud: &Path, operation: CreateState, stage: Stage) -> Deployment {
    let mut value: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":cloud.file_name().unwrap().to_str().unwrap(),"repository":cloud.join("synthetic-repository"),"revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    })).unwrap();
    value.operation = operation;
    value.stage = stage;
    value
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn removal_rereads_selection_and_allocation_after_its_initial_enumeration() {
    let f = Fixture::new();
    let cloud = f.cloud("cloud-a", None);
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            barrier.wait();
            let outcome = (|| {
                crate::cloud_runtime::tailnet::change(f.root.path(), "cloud-a", Some("work"))?;
                let held = state::Store::lock(&cloud)?;
                held.save(&deployment(&cloud, CreateState::Requested, Stage::Provision))
            })();
            barrier.wait();
            outcome.unwrap();
        });
        assert!(
            f.remove(|boundary| {
                if boundary == Boundary::Enumerated {
                    barrier.wait();
                    barrier.wait();
                }
                Ok(())
            })
            .is_err()
        );
        writer.join().unwrap();
    });
    f.retained();
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn a_new_cloud_between_enumeration_and_catalog_ownership_refuses_deletion() {
    let f = Fixture::new();
    f.cloud("cloud-a", None);
    assert!(matches!(
        f.remove(|boundary| {
            if boundary == Boundary::Enumerated {
                f.cloud("cloud-new", Some("work"));
            }
            Ok(())
        }),
        Err(Error::Busy)
    ));
    f.retained();
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn removal_holds_every_cloud_lock_until_credential_deletion_finishes() {
    let f = Fixture::new();
    let cloud = f.cloud("cloud-a", Some("work"));
    let barrier = Arc::new(Barrier::new(2));
    let provider_calls = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let worker_barrier = Arc::clone(&barrier);
        let cloud = &cloud;
        let provider_calls = &provider_calls;
        let writer = scope.spawn(move || {
            worker_barrier.wait();
            let result = state::Store::lock(cloud).and_then(|_held| {
                crate::cloud_runtime::tailnet::validate_pending(cloud)?;
                provider_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
            worker_barrier.wait();
            assert!(matches!(result, Err(Error::Busy)));
        });
        f.remove(|boundary| {
            if boundary == Boundary::Owned {
                barrier.wait();
                barrier.wait();
            }
            Ok(())
        })
        .unwrap();
        writer.join().unwrap();
    });
    assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.deleted.load(Ordering::SeqCst), 1);
    assert!(crate::cloud_runtime::tailnet::validate_pending(&cloud).is_err());
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn a_new_selection_waits_for_catalog_deletion_then_refuses_the_missing_binding() {
    let f = Fixture::new();
    f.cloud("cloud-a", None);
    let (started, ready) = mpsc::channel();
    let (result, outcome) = mpsc::channel();
    std::thread::scope(|scope| {
        let mut writer = None;
        f.remove(|boundary| {
            if boundary == Boundary::Owned {
                writer = Some(scope.spawn(|| {
                    let cloud = f.cloud("cloud-new", None);
                    started.send(()).unwrap();
                    let changed = crate::cloud_runtime::tailnet::change(f.root.path(), "cloud-new", Some("work"));
                    result.send(changed.is_err()).unwrap();
                    assert!(Selection::load(&cloud).unwrap().tailnet.is_none());
                }));
                ready.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                assert!(
                    outcome.try_recv().is_err(),
                    "catalog ownership fences the new selection"
                );
            }
            Ok(())
        })
        .unwrap();
        assert!(outcome.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
        writer.unwrap().join().unwrap();
    });
}

fn pending_request(cloud: &Path, selected: &str, claimed: bool) -> horizon_cloud_protocol::OperationId {
    let operation = horizon_cloud_protocol::OperationId::generate();
    std::fs::write(
        cloud.join(format!("tailnet-request-{operation}.json")),
        serde_json::to_vec(&Selection {
            tailnet: Some(selected.into()),
        })
        .unwrap(),
    )
    .unwrap();
    std::fs::write(cloud.join(format!("tailnet-commit-{operation}.pending")), b"1").unwrap();
    if claimed {
        std::fs::write(
            cloud.join("companion-operation.json"),
            serde_json::to_vec(&serde_json::json!({
                "owner":{"scope":{"session_id":"session","workspace_id":"workspace"},"cloud_id":"source"},
                "id":operation,"phase":"submitted"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    operation
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn claimed_and_unclaimed_requests_fence_removal_even_when_selection_differs() {
    for claimed in [false, true] {
        let f = Fixture::new();
        let cloud = f.cloud("cloud-a", Some("other"));
        pending_request(&cloud, "work", claimed);
        let before = std::fs::read_dir(&cloud).unwrap().count();
        assert!(f.remove(|_| Ok(())).is_err());
        f.retained();
        assert_eq!(std::fs::read_dir(&cloud).unwrap().count(), before);
        assert!(crate::cloud_runtime::tailnet::change(f.root.path(), "cloud-a", Some("work")).is_err());
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn malformed_terminal_marker_keeps_credentials_and_valid_terminal_marker_stays_settled() {
    for phase in ["ready", "stopped", "refused", "retry_required"] {
        for corrupt in [false, true] {
            let f = Fixture::new();
            let cloud = f.cloud("cloud-a", Some("other"));
            let operation = pending_request(&cloud, "work", false);
            std::fs::write(
                cloud.join("companion-operation.json"),
                serde_json::to_vec(&serde_json::json!({
                    "owner":{"scope":{"session_id":"session","workspace_id":"workspace"},"cloud_id":"source"},
                    "id":operation,"phase":phase
                }))
                .unwrap(),
            )
            .unwrap();
            let marker = cloud.join(format!("tailnet-commit-{operation}.pending"));
            let request = cloud.join(format!("tailnet-request-{operation}.json"));
            if corrupt {
                std::fs::write(&marker, b"uncertain").unwrap();
            }
            let saved_marker = std::fs::read(&marker).unwrap();
            let saved_request = std::fs::read(&request).unwrap();
            let catalog = std::fs::read(f.root.path().join("tailnets.json")).unwrap();
            let mut provider_calls = 0;
            let held = state::Store::lock(&cloud).unwrap();
            let admission = crate::cloud_runtime::tailnet::validate_pending(&cloud).map(|()| provider_calls += 1);
            drop(held);
            if corrupt {
                assert!(matches!(
                    admission,
                    Err(Error::Invalid("Invalid pending tailnet marker"))
                ));
                assert_eq!(provider_calls, 0);
                assert!(f.remove(|_| Ok(())).is_err());
                f.retained();
                assert_eq!(std::fs::read(f.root.path().join("tailnets.json")).unwrap(), catalog);
            } else {
                admission.unwrap();
                assert_eq!(provider_calls, 1);
                f.remove(|_| Ok(())).unwrap();
                assert_eq!(f.deleted.load(Ordering::SeqCst), 1);
            }
            assert_eq!(std::fs::read(&marker).unwrap(), saved_marker);
            assert_eq!(std::fs::read(&request).unwrap(), saved_request);
        }
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn malformed_catalog_selection_deployment_and_pending_state_never_delete_credentials() {
    for file in [
        "tailnets.json",
        "tailnets.pending.json",
        "tailnet.json",
        "deployment.json",
        "companion-operation.json",
        "tailnet-commit-invalid.pending",
    ] {
        let f = Fixture::new();
        let cloud = f.cloud("cloud-a", Some("other"));
        let path = if file.starts_with("tailnets.") {
            f.root.path().join(file)
        } else {
            cloud.join(file)
        };
        std::fs::write(&path, b"corrupt synthetic state").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(f.remove(|_| Ok(())).is_err(), "{file}");
        assert_eq!(f.deleted.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn incomplete_or_malformed_unclaimed_reservations_keep_credentials() {
    for defect in [
        "missing request",
        "corrupt request",
        "invalid ID",
        "oversize request",
        "corrupt marker",
    ] {
        let f = Fixture::new();
        let cloud = f.cloud("cloud-a", Some("other"));
        let operation = horizon_cloud_protocol::OperationId::generate();
        let request = cloud.join(format!("tailnet-request-{operation}.json"));
        let marker = cloud.join(format!("tailnet-commit-{operation}.pending"));
        std::fs::write(&marker, b"1").unwrap();
        match defect {
            "missing request" => {}
            "corrupt request" => std::fs::write(&request, b"malformed").unwrap(),
            "invalid ID" => std::fs::write(&request, br#"{"tailnet":"../work"}"#).unwrap(),
            "oversize request" => std::fs::write(&request, vec![b' '; 1025]).unwrap(),
            "corrupt marker" => {
                std::fs::write(&request, br#"{"tailnet":"work"}"#).unwrap();
                std::fs::write(&marker, b"uncertain").unwrap();
            }
            _ => unreachable!(),
        }
        let saved_request = std::fs::read(&request).ok();
        let saved_marker = std::fs::read(&marker).unwrap();
        assert!(f.remove(|_| Ok(())).is_err(), "{defect}");
        f.retained();
        assert_eq!(std::fs::read(&request).ok(), saved_request);
        assert_eq!(std::fs::read(&marker).unwrap(), saved_marker);
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn unknown_directories_fail_closed_but_legacy_registry_is_not_a_cloud() {
    let f = Fixture::new();
    f.cloud("cloud-a", None);
    std::fs::create_dir(f.root.path().join("unknown.directory")).unwrap();
    assert!(f.remove(|_| Ok(())).is_err());
    f.retained();
    let f = Fixture::new();
    f.cloud("cloud-a", None);
    std::fs::create_dir(f.root.path().join(".allocations")).unwrap();
    f.remove(|_| Ok(())).unwrap();
    assert_eq!(f.deleted.load(Ordering::SeqCst), 1);
}

#[test]
#[cfg(unix)]
fn symlinked_cloud_or_legacy_registry_refuses_removal() {
    for name in ["linked-cloud", ".allocations"] {
        let f = Fixture::new();
        f.cloud("cloud-a", None);
        std::os::unix::fs::symlink("cloud-a", f.root.path().join(name)).unwrap();
        assert!(f.remove(|_| Ok(())).is_err());
        f.retained();
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn unknown_provider_state_and_busy_clouds_preserve_the_catalog() {
    for provider_file in ["hetzner.json", "workspace-volume.json", "workspace-volume.required"] {
        let f = Fixture::new();
        let cloud = f.cloud("cloud-a", Some("work"));
        std::fs::write(cloud.join(provider_file), b"uncertain synthetic provider state").unwrap();
        assert!(f.remove(|_| Ok(())).is_err());
        f.retained();
    }
    let f = Fixture::new();
    let cloud = f.cloud("cloud-a", Some("other"));
    let _owned = state::Store::lock(&cloud).unwrap();
    assert!(matches!(f.remove(|_| Ok(())), Err(Error::Busy)));
    f.retained();
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn missing_catalog_membership_fails_before_a_new_provider_request() {
    let f = Fixture::new();
    let cloud = f.cloud("cloud-a", Some("work"));
    f.remove(|_| Ok(())).unwrap();
    let mut provider_calls = 0;
    let _owned = state::Store::lock(&cloud).unwrap();
    let result = crate::cloud_runtime::tailnet::validate_pending(&cloud).map(|()| provider_calls += 1);
    assert!(result.is_err());
    assert_eq!(provider_calls, 0);
    assert_eq!(Selection::load(&cloud).unwrap().tailnet.as_deref(), Some("work"));
}

#[test]
#[cfg(windows)]
fn windows_cloud_ownership_refuses_before_selection_or_credential_mutation() {
    let f = Fixture::new();
    let cloud = f.root.path().join("cloud-a");
    std::fs::create_dir(&cloud).unwrap();
    let selection = cloud.join("tailnet.json");
    std::fs::write(&selection, br#"{"tailnet":"work"}"#).unwrap();
    let catalog_path = f.root.path().join("tailnets.json");
    let catalog = std::fs::read(&catalog_path).unwrap();
    let original = std::fs::read(&selection).unwrap();
    for result in [
        remove_saved(f.root.path(), "work").map(|_| ()),
        crate::cloud_runtime::tailnet::change(f.root.path(), "cloud-a", Some("other")),
        f.remove(|_| Ok(())).map(|_| ()),
    ] {
        assert!(matches!(result, Err(Error::Io(ref error)) if error.kind() == std::io::ErrorKind::Unsupported));
        assert_eq!(std::fs::read(&catalog_path).unwrap(), catalog);
        assert_eq!(std::fs::read(&selection).unwrap(), original);
        assert_eq!(f.deleted.load(Ordering::SeqCst), 0);
    }
    assert!(!cloud.join("operation.lock").exists());
}

fn setup_files(f: &Fixture) -> Vec<(PathBuf, Vec<u8>)> {
    let files = [
        ("credentials/compute-fixture", b"synthetic credential".as_slice()),
        (
            "credentials/registry-pull-fixture",
            b"synthetic registry credential".as_slice(),
        ),
        ("identity-fixture/ed25519", b"synthetic private identity".as_slice()),
        ("identity-fixture/ed25519.pub", b"synthetic public identity".as_slice()),
    ];
    files
        .into_iter()
        .map(|(name, bytes)| {
            let path = f.root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            (path, bytes.to_vec())
        })
        .collect()
}

fn setup_retained(f: &Fixture, files: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in files {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
        assert!(!path.parent().unwrap().join("operation.lock").exists());
    }
    assert_eq!(std::fs::read_dir(f.root.path().join("credentials")).unwrap().count(), 2);
    assert_eq!(
        std::fs::read_dir(f.root.path().join("identity-fixture"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn setup_directories_are_untouched_while_real_pending_clouds_still_fence_removal() {
    for pending in [false, true] {
        let f = Fixture::new();
        let files = setup_files(&f);
        let cloud = f.cloud("cloud-a", Some("other"));
        if pending {
            pending_request(&cloud, "work", false);
            assert!(f.remove(|_| Ok(())).is_err());
            f.retained();
        } else {
            f.remove(|_| Ok(())).unwrap();
            assert_eq!(f.deleted.load(Ordering::SeqCst), 1);
        }
        setup_retained(&f, &files);
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn settings_namespace_collisions_preserve_every_cloud_evidence_file() {
    for directory in ["credentials", "identity-fixture"] {
        for file in [
            "tailnet.json",
            "deployment.json",
            "tailnet-commit-fixture.pending",
            "hetzner.json",
            "operation.lock",
        ] {
            let f = Fixture::new();
            let files = setup_files(&f);
            let path = f.root.path().join(directory).join(file);
            std::fs::write(&path, b"retained cloud evidence").unwrap();
            assert!(f.remove(|_| Ok(())).is_err());
            f.retained();
            assert_eq!(std::fs::read(&path).unwrap(), b"retained cloud evidence");
            for (path, bytes) in &files {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
        }
    }
}

#[test]
fn incomplete_identity_and_unknown_settings_entries_refuse_without_cloud_lock_creation() {
    for directory in ["identity-empty", "credentials"] {
        let f = Fixture::new();
        let path = f.root.path().join(directory);
        std::fs::create_dir(&path).unwrap();
        if directory == "credentials" {
            std::fs::write(path.join("unknown-state"), b"retained unknown evidence").unwrap();
        }
        assert!(f.remove(|_| Ok(())).is_err());
        assert!(!path.join("operation.lock").exists());
        f.retained();
    }
}

// These fixtures require Unix directory symlink semantics.
#[cfg(unix)]
#[test]
fn settings_directory_and_key_symlinks_remain_fail_closed() {
    use std::os::unix::fs::symlink;
    for directory_link in [false, true] {
        let f = Fixture::new();
        let files = setup_files(&f);
        let target = f.root.path().join("original-fixture");
        if directory_link {
            std::fs::rename(f.root.path().join("credentials"), &target).unwrap();
            symlink(&target, f.root.path().join("credentials")).unwrap();
        } else {
            let key = f.root.path().join("identity-fixture/ed25519");
            std::fs::rename(&key, &target).unwrap();
            symlink(&target, &key).unwrap();
        }
        assert!(f.remove(|_| Ok(())).is_err());
        f.retained();
        assert!(!f.root.path().join("credentials/operation.lock").exists());
        assert!(!f.root.path().join("identity-fixture/operation.lock").exists());
        assert!(std::fs::symlink_metadata(&target).is_ok());
        assert_eq!(files.len(), 4);
    }
}

fn registry_settings(f: &Fixture, directory: &str) {
    std::fs::write(f.root.path().join("settings.json"), serde_json::to_vec(&serde_json::json!({
        "runpod_key_file": f.root.path().join("credentials/compute-fixture"),
        "ssh_identity_file": f.root.path().join("identity-fixture/ed25519"),
        "docker_config": f.root.path().join("docker"), "registry_pull_auth_id": null,
        "cpu_flavors": [], "gpu_types": [],
        "registries": {"root": f.root.path().join(directory), "bindings": [{
            "repository": "registry.example/worker", "publish": null,
            "pull": {"username": "fixture", "secret_file": f.root.path().join("credentials/registry-pull-fixture"), "expires_at": null},
            "read_only_confirmed": true, "generation": "fixture-generation", "retired": []
        }]}
    })).unwrap()).unwrap();
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn only_the_bound_registry_directory_is_excluded_and_membership_drift_refuses() {
    for drift in ["none", "cloud state", "settings", "credential membership"] {
        let f = Fixture::new();
        let files = setup_files(&f);
        registry_settings(&f, "custom-registry");
        let registry = f.root.path().join("custom-registry");
        std::fs::create_dir(&registry).unwrap();
        let journal = registry.join("fixture-generation.json");
        std::fs::write(&journal, b"synthetic registry journal").unwrap();
        f.cloud("registry", Some("other"));
        let result = f.remove(|boundary| {
            if boundary == Boundary::Owned {
                match drift {
                    "cloud state" => std::fs::write(registry.join("deployment.json"), b"retained cloud evidence")?,
                    "settings" => registry_settings(&f, "different-registry"),
                    "credential membership" => std::fs::write(
                        f.root.path().join("credentials/compute-new"),
                        b"new synthetic credential",
                    )?,
                    _ => {}
                }
            }
            Ok(())
        });
        // The final classification guard must run after the Owned checkpoint too.
        if drift == "none" {
            result.unwrap();
            setup_retained(&f, &files);
        } else {
            assert!(result.is_err());
            f.retained();
        }
        assert_eq!(std::fs::read(&journal).unwrap(), b"synthetic registry journal");
        assert!(!registry.join("operation.lock").exists());
        assert!(f.root.path().join("registry/operation.lock").exists());
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud lifecycle ownership requires Unix directory durability")]
fn custom_credential_names_require_exact_settings_binding_and_never_hide_cloud_state() {
    for name in [
        "custom-provider-token",
        "browser-config.yaml",
        "deployment.json",
        "tailnet-commit-fixture.pending",
    ] {
        for bound in [false, true] {
            let f = Fixture::new();
            let files = setup_files(&f);
            registry_settings(&f, "custom-registry");
            let custom = f.root.path().join("credentials").join(name);
            std::fs::write(&custom, b"synthetic custom credential").unwrap();
            if bound {
                let settings_path = f.root.path().join("settings.json");
                let mut settings: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
                settings["runpod_key_file"] = serde_json::to_value(&custom).unwrap();
                std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
            }
            f.cloud("cloud-a", Some("other"));
            let allowed = bound && !matches!(name, "deployment.json" | "tailnet-commit-fixture.pending");
            let result = f.remove(|_| Ok(()));
            assert_eq!(result.is_ok(), allowed, "{name} bound={bound}");
            if !allowed {
                f.retained();
            }
            assert_eq!(std::fs::read(&custom).unwrap(), b"synthetic custom credential");
            assert!(!custom.parent().unwrap().join("operation.lock").exists());
            for (path, bytes) in &files {
                assert_eq!(&std::fs::read(path).unwrap(), bytes);
            }
        }
    }
}
