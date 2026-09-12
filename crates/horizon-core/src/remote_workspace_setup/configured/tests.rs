//! Coordinator tests use local stores and fake dispatch, never provider or credential access.

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_refuses() {
    assert_eq!(super::platform(), Err(super::Error::UnsupportedPlatform));
}

#[cfg(target_os = "linux")]
mod linux {
    use super::super::*;
    use crate::cloud_run::{GitCommitSha, local_docker::LocalDockerProfile, runpod::RunPodProfile};
    use std::{
        cell::Cell,
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering},
        },
    };

    const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    const OTHER: &str = "00000000-0000-4000-8000-000000000002";

    struct Fixture {
        directory: tempfile::TempDir,
        home: HorizonHome,
        config: RemoteProviderConfig,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("fixture");
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
            let home = HorizonHome::from_root(directory.path().join("home"));
            let config = RemoteProviderConfig {
                local_docker: vec![LocalDockerProfile {
                    name: "local".into(),
                    docker_host: "unix:///tmp/setup-fixture.sock".into(),
                }],
                runpod: vec![RunPodProfile {
                    name: "cloud".into(),
                    gpu_type_ids: vec!["fixture-gpu".into()],
                    gpu_count: 1,
                    allowed_cuda_versions: vec![],
                    data_center_id: Some("fixture-dc".into()),
                    ports: vec!["22/tcp".into()],
                    volume_gib: 0,
                    min_download_mbps: None,
                    min_upload_mbps: None,
                    min_disk_bandwidth_mbps: None,
                    container_registry_auth_id: None,
                }],
            };
            Self {
                directory,
                home,
                config,
            }
        }
        fn draft(cloud: bool) -> RemoteWorkspaceSetupDraft {
            RemoteWorkspaceSetupDraft {
                target: WorkerTarget {
                    provider: if cloud {
                        CloudProvider::RunPod
                    } else {
                        CloudProvider::LocalDocker
                    },
                    profile: if cloud { "cloud" } else { "local" }.into(),
                    image: format!("example/worker@sha256:{}", "a".repeat(64)),
                    disk_gib: 20,
                    lifetime: WorkerLifetime::Persistent,
                    max_hourly_cost_micros: cloud.then_some(1_000_000),
                },
                repository: GitSource {
                    repository: "example/project".into(),
                    commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
                    branch: Some("work/fixture".into()),
                },
                working_directory: ".".into(),
                command: RemotePanelCommand {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "printf '%s' 'two words'".into()],
                },
                panel_directory: Some("src".into()),
                retain_until_millis: i64::MAX,
                network_volume: cloud.then(|| RunPodNetworkVolumeExpectation {
                    volume_id: "fixture-volume".into(),
                    data_center_id: "fixture-dc".into(),
                    minimum_size_gb: 10,
                }),
            }
        }
        fn preview(&self, cloud: bool) -> PreparedRemoteWorkspaceSetup {
            preview_configured_remote_workspace(&self.home, &self.config, OWNER, Self::draft(cloud)).expect("preview")
        }
        fn store(&self) -> CloudWorkflowStore {
            CloudWorkflowStore::open(&self.home).expect("store")
        }
        fn submit(&self, prepared: &PreparedRemoteWorkspaceSetup) -> Result<StoredRemoteAllocation, Error> {
            submit_with(
                &self.home,
                &self.config,
                OWNER,
                prepared,
                &consent(prepared),
                |store, saved| allocate(store, saved, prepared),
            )
        }
        fn check(&self, locator: &RemoteWorkspaceSetupLocator) -> Result<ConfiguredWorkspaceSetupObservation, Error> {
            check_with(&self.home, &self.config, OWNER, locator, |_, _| {
                panic!("must not recover")
            })
        }
    }
    fn consent(prepared: &PreparedRemoteWorkspaceSetup) -> RemoteWorkspaceSetupConsent {
        let image = prepared.spec().target.image.clone();
        match prepared.network_volume() {
            Some(volume) => RemoteWorkspaceSetupConsent::RunPodHps {
                image,
                volume: volume.clone(),
            },
            None => RemoteWorkspaceSetupConsent::LocalDocker { image },
        }
    }
    fn allocate(
        store: &CloudWorkflowStore,
        saved: &StoredRemoteWorkspace,
        prepared: &PreparedRemoteWorkspaceSetup,
    ) -> Result<StoredRemoteAllocation, Error> {
        let allocation = store
            .allocate_remote_runtime(saved, prepared.retain_until_millis())
            .map_err(storage)?;
        if let Some(volume) = prepared.network_volume() {
            store
                .record_remote_network_volume_selection(&allocation, volume)
                .map_err(storage)?;
        }
        Ok(allocation)
    }
    fn duplicate(prepared: &PreparedRemoteWorkspaceSetup) -> PreparedRemoteWorkspaceSetup {
        // Competing confirmations with the same IDs are a store-fence fixture, not a public Clone API.
        PreparedRemoteWorkspaceSetup {
            locator: prepared.locator.clone(),
            config: prepared.config.clone(),
            state: prepared.state.clone(),
            retain_until_millis: prepared.retain_until_millis,
            network_volume: prepared.network_volume.clone(),
        }
    }
    fn reserve(store: &CloudWorkflowStore, allocation: &StoredRemoteAllocation) -> StoredRemoteAllocation {
        use base64::Engine as _;
        let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        key.extend([7; 32]);
        let key = format!("ssh-ed25519 {}", base64::engine::general_purpose::STANDARD.encode(key));
        // Synthetic reserved public identity isolates this coordinator; no private-key proof is claimed.
        store.reserve_remote_worker_request(allocation, &key).expect("reserve")
    }
    fn expire(store: &CloudWorkflowStore, allocation: &StoredRemoteAllocation) {
        let mut workflow = allocation.workflow().workflow().clone();
        workflow.created_at_millis = 1000;
        workflow.updated_at_millis = 1000;
        workflow.retain_until_millis = 2000;
        rusqlite::Connection::open(store.path()).expect("fixture database").execute(
            "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000, retain_until_millis=2000, snapshot=?1 WHERE workflow_id=?2",
            rusqlite::params![serde_json::to_vec(&workflow).expect("snapshot"), workflow.id.to_string()],
        ).expect("expire fixture");
    }

    #[test]
    fn preview_is_inert_and_binds_exact_fresh_intent() {
        let f = Fixture::new();
        for cloud in [false, true] {
            let prepared = f.preview(cloud);
            assert_eq!(prepared.spec().generation, 0);
            assert_eq!(prepared.spec().panels.len(), 1);
            assert_eq!(prepared.spec().panels[0].kind, PanelKind::Shell);
            assert_eq!(
                prepared.spec().panels[0].command.as_ref(),
                Some(&Fixture::draft(cloud).command)
            );
            assert_eq!(prepared.spec().repository, Fixture::draft(cloud).repository);
            assert_ne!(
                prepared.locator().workspace_local_id,
                f.preview(cloud).locator().workspace_local_id
            );
            assert!(prepared.state.runtime.is_none());
        }
        assert!(!f.home.root().exists());
    }

    #[test]
    fn malformed_or_unsupported_previews_have_no_side_effects() {
        let f = Fixture::new();
        for mutate in [
            |d: &mut RemoteWorkspaceSetupDraft| d.target.image = "example/worker:latest".into(),
            |d: &mut RemoteWorkspaceSetupDraft| d.target.provider = CloudProvider::Azure,
            |d: &mut RemoteWorkspaceSetupDraft| d.target.lifetime = WorkerLifetime::TimeLimited { seconds: 900 },
            |d: &mut RemoteWorkspaceSetupDraft| d.repository.branch = None,
            |d: &mut RemoteWorkspaceSetupDraft| d.working_directory = "../outside".into(),
            |d: &mut RemoteWorkspaceSetupDraft| d.command.program.clear(),
            |d: &mut RemoteWorkspaceSetupDraft| d.command.args.push("bad\0argument".into()),
            |d: &mut RemoteWorkspaceSetupDraft| d.retain_until_millis = 0,
        ] {
            let mut draft = Fixture::draft(false);
            mutate(&mut draft);
            assert!(preview_configured_remote_workspace(&f.home, &f.config, OWNER, draft).is_err());
        }
        for owner in ["", "nil", "00000000-0000-0000-0000-000000000000"] {
            assert!(preview_configured_remote_workspace(&f.home, &f.config, owner, Fixture::draft(false)).is_err());
        }
        for mutate in [
            |d: &mut RemoteWorkspaceSetupDraft| d.network_volume = None,
            |d: &mut RemoteWorkspaceSetupDraft| d.network_volume.as_mut().expect("volume").minimum_size_gb = 9,
            |d: &mut RemoteWorkspaceSetupDraft| {
                d.network_volume.as_mut().expect("volume").data_center_id = "elsewhere".into();
            },
            |d: &mut RemoteWorkspaceSetupDraft| d.target.max_hourly_cost_micros = None,
        ] {
            let mut draft = Fixture::draft(true);
            mutate(&mut draft);
            assert!(preview_configured_remote_workspace(&f.home, &f.config, OWNER, draft).is_err());
        }
        assert!(!f.home.root().exists());
    }

    #[test]
    fn confirmation_rechecks_home_owner_full_configuration_consent_and_expiry() {
        let f = Fixture::new();
        for mode in 0..7 {
            let mut p = f.preview(true);
            let mut config = f.config.clone();
            let mut approval = consent(&p);
            let other_home = HorizonHome::from_root(f.directory.path().join("other"));
            let home = if mode == 0 { &other_home } else { &f.home };
            let owner = if mode == 1 { OTHER } else { OWNER };
            match mode {
                2 => config.runpod[0].gpu_count = 2,
                3 => {
                    approval = RemoteWorkspaceSetupConsent::LocalDocker {
                        image: p.spec().target.image.clone(),
                    }
                }
                4 | 6 => {
                    let RemoteWorkspaceSetupConsent::RunPodHps { image, volume } = &mut approval else {
                        panic!("RunPod consent");
                    };
                    if mode == 4 {
                        volume.volume_id = "foreign".into();
                    } else {
                        *image = format!("example/worker@sha256:{}", "c".repeat(64));
                    }
                }
                5 => p.retain_until_millis = 0,
                _ => {}
            }
            if mode == 0 {
                let locator = p.locator().clone();
                let attempt = submit_configured_remote_workspace(home, &config, owner, p, approval);
                assert!(attempt.locator == locator);
                assert!(matches!(attempt.result, Err(Error::ContextChanged)));
            } else {
                assert!(submit_with(home, &config, owner, &p, &approval, |_, _| panic!("no dispatch")).is_err());
            }
            assert!(!f.home.root().exists());
            assert!(!other_home.root().exists());
        }
    }

    fn retargeted_submit(relative: Option<&str>) {
        let f = Fixture::new();
        let destinations = [f.directory.path().join("first"), f.directory.path().join("second")];
        for destination in &destinations {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(destination)
                .expect("target");
        }
        let link = f.directory.path().join("selected");
        symlink(&destinations[0], &link).expect("initial selection");
        let home = HorizonHome::from_root(relative.map_or_else(|| link.clone(), |relative| link.join(relative)));
        let prepared = preview_configured_remote_workspace(&home, &f.config, OWNER, Fixture::draft(false))
            .expect("filesystem-free preview");
        let locator = prepared.locator().clone();
        let approval = consent(&prepared);
        std::fs::remove_file(&link).expect("remove owned link");
        symlink(&destinations[1], &link).expect("retarget owned link");
        let attempt = submit_configured_remote_workspace(&home, &f.config, OWNER, prepared, approval);
        assert!(attempt.locator == locator);
        for destination in &destinations {
            let destination = HorizonHome::from_root(destination.join(relative.unwrap_or_default()));
            assert!(!destination.root().join("remote-ssh-identities").exists());
            assert!(
                !destination.cloud_workflow_store_path().exists(),
                "no wrong-home database"
            );
            assert!(!destination.root().join("cloud-run").exists());
        }
        assert!(matches!(attempt.result, Err(Error::StorageUnavailable)));
    }

    #[test]
    fn public_submit_refuses_retargeted_home_before_writing() {
        for relative in [None, Some(""), Some(".")] {
            retargeted_submit(relative);
        }
    }

    #[test]
    fn public_submit_refuses_retargeted_ancestor_before_writing() {
        retargeted_submit(Some("home"));
    }

    fn shared_parent_fixture() -> Fixture {
        let mut fixture = Fixture::new();
        let parent = fixture.directory.path().join("shared");
        std::fs::create_dir(&parent).expect("shared parent");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o1777)).expect("sticky mode");
        fixture.home = HorizonHome::from_root(parent.join("home"));
        fixture
    }

    #[test]
    fn missing_home_under_shared_sticky_parent_refuses_before_writes() {
        let f = shared_parent_fixture();
        let prepared = f.preview(false);
        let result = submit_with(&f.home, &f.config, OWNER, &prepared, &consent(&prepared), |_, _| {
            Err(Error::SetupUnconfirmed)
        });
        assert!(!f.home.root().exists(), "no control store below shared parent");
        assert!(matches!(result, Err(Error::StorageUnavailable)));
        let locator = prepared.locator().clone();
        let approval = consent(&prepared);
        let attempt = submit_configured_remote_workspace(&f.home, &f.config, OWNER, prepared, approval);
        assert!(attempt.locator == locator);
        assert!(matches!(attempt.result, Err(Error::StorageUnavailable)));
        assert!(!f.home.root().exists());
        assert!(!f.home.root().join("remote-ssh-identities").exists());
    }

    #[test]
    fn existing_owned_home_below_shared_sticky_parent_remains_valid() {
        let f = shared_parent_fixture();
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(f.home.root())
            .expect("owned home");
        let allocation = f.submit(&f.preview(false)).expect("fake dispatch");
        assert_eq!(allocation.workspace().state().spec.generation, 1);
        assert!(!f.home.root().join("remote-ssh-identities").exists());
    }

    #[test]
    fn recovery_refuses_a_linked_home_before_adopting_a_saved_record() {
        let f = Fixture::new();
        let prepared = f.preview(false);
        f.submit(&prepared).expect("saved allocation");
        let link = f.directory.path().join("selected");
        symlink(f.home.root(), &link).expect("linked home");
        let home = HorizonHome::from_root(link);
        let locator = RemoteWorkspaceSetupLocator::new(&home, OWNER, &prepared.locator.workspace_local_id)
            .expect("lexical locator");
        let before = std::fs::read(f.home.cloud_workflow_store_path()).expect("saved bytes");
        assert!(matches!(
            check_configured_remote_workspace_setup(&home, &f.config, OWNER, &locator),
            Err(Error::StorageUnavailable)
        ));
        assert_eq!(
            std::fs::read(f.home.cloud_workflow_store_path()).expect("unchanged"),
            before
        );
    }

    #[test]
    fn recovery_rechecks_home_before_opening_a_writable_store() {
        let f = Fixture::new();
        let prepared = f.preview(false);
        let allocation = reserve(&f.store(), &f.submit(&prepared).expect("allocation"));
        let link = f.directory.path().join("selected");
        symlink(f.home.root(), &link).expect("initial home");
        let home = HorizonHome::from_root(link.clone());
        let observed = CloudWorkflowStore::open_read_only(&home).expect("original read-only store");
        let other = HorizonHome::from_root(f.directory.path().join("other"));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(other.root())
            .expect("other");
        std::fs::remove_file(&link).expect("remove owned link");
        symlink(other.root(), &link).expect("retarget owned link");
        let result = check_saved_with(&home, &f.config, &observed, allocation.workspace().clone(), |_, _| {
            panic!("no recovery")
        });
        assert!(
            !other.cloud_workflow_store_path().exists(),
            "no redirected recovery store"
        );
        assert!(!other.root().join("cloud-run").exists());
        assert!(matches!(result, Err(Error::StorageUnavailable)));
        assert_eq!(
            observed
                .load_remote_allocation(OWNER, &prepared.locator.workspace_local_id)
                .expect("load"),
            Some(allocation)
        );
    }

    #[test]
    fn untrusted_home_components_refuse_before_storage_or_dispatch() {
        for mode in 0..5 {
            let mut f = Fixture::new();
            match mode {
                0 => {
                    std::fs::create_dir(f.home.root()).expect("home");
                    std::fs::set_permissions(f.home.root(), std::fs::Permissions::from_mode(0o770)).expect("mode");
                }
                1 => std::fs::set_permissions(f.directory.path(), std::fs::Permissions::from_mode(0o770))
                    .expect("untrusted parent"),
                2 => std::fs::write(f.home.root(), b"not a directory").expect("file"),
                3 => f.home = HorizonHome::from_root(f.directory.path().join("missing-parent/home")),
                _ => {
                    std::fs::create_dir(f.directory.path().join("parent")).expect("parent");
                    f.home = HorizonHome::from_root(f.directory.path().join("parent/../home"));
                }
            }
            let prepared = f.preview(false);
            let result = submit_with(&f.home, &f.config, OWNER, &prepared, &consent(&prepared), |_, _| {
                panic!("no dispatch")
            });
            assert!(matches!(result, Err(Error::StorageUnavailable)));
            assert!(!f.home.cloud_workflow_store_path().exists());
            assert!(!f.home.root().join("remote-ssh-identities").exists());
        }
    }

    #[test]
    fn exact_submit_preserves_one_workspace_panel_generation_and_volume() {
        for cloud in [false, true] {
            let f = Fixture::new();
            let p = f.preview(cloud);
            let allocation = f.submit(&p).expect("submitted");
            assert_eq!(allocation.workspace().state().spec.generation, 1);
            assert_eq!(allocation.workspace().state().spec.panels, p.spec().panels);
            let store = f.store();
            assert_eq!(
                store
                    .load_remote_allocation(OWNER, &p.locator.workspace_local_id)
                    .expect("load")
                    .as_ref(),
                Some(&allocation)
            );
            assert_eq!(
                store
                    .load_remote_network_volume_selection(&allocation)
                    .expect("selection")
                    .as_ref(),
                p.network_volume()
            );
            assert_eq!(f.submit(&p).err(), Some(Error::SaveConflict));
            assert!(!f.home.root().join("remote-ssh-identities").exists());
        }
    }

    #[test]
    fn concurrent_duplicate_confirmations_dispatch_only_once() {
        let f = Fixture::new();
        let p = f.preview(false);
        let other = duplicate(&p);
        let _store = f.store();
        let barrier = Arc::new(Barrier::new(2));
        let calls = AtomicUsize::new(0);
        let results = std::thread::scope(|scope| {
            let (fixture, barrier, calls) = (&f, &barrier, &calls);
            let handles = [&p, &other].map(|prepared| {
                scope.spawn(move || {
                    barrier.wait();
                    submit_with(
                        &fixture.home,
                        &fixture.config,
                        OWNER,
                        prepared,
                        &consent(prepared),
                        |store, saved| {
                            calls.fetch_add(1, Ordering::SeqCst);
                            allocate(store, saved, prepared)
                        },
                    )
                })
            });
            handles.map(|handle| handle.join().expect("concurrent confirmation"))
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(results.iter().any(|result| matches!(result, Err(Error::SaveConflict))));
    }

    #[test]
    fn partial_save_and_pre_request_interruption_are_locatable_without_repair() {
        for cloud in [false, true] {
            for allocated in [false, true] {
                let f = Fixture::new();
                let p = f.preview(cloud);
                let locator = p.locator().clone();
                let result = submit_with(&f.home, &f.config, OWNER, &p, &consent(&p), |store, saved| {
                    if allocated {
                        store.allocate_remote_runtime(saved, i64::MAX).expect("allocation");
                    }
                    Err(Error::SetupUnconfirmed)
                });
                assert_eq!(result.err(), Some(Error::SetupUnconfirmed));
                let before = f
                    .store()
                    .load_remote_workspace(OWNER, &locator.workspace_local_id)
                    .expect("saved");
                for _ in 0..2 {
                    match f.check(&locator).expect("check") {
                        ConfiguredWorkspaceSetupObservation::Interrupted(_) if allocated => {}
                        ConfiguredWorkspaceSetupObservation::SavedOnly(_) if !allocated => {}
                        _ => panic!("unexpected recovery stage"),
                    }
                }
                assert_eq!(
                    f.store()
                        .load_remote_workspace(OWNER, &locator.workspace_local_id)
                        .expect("unchanged"),
                    before
                );
            }
        }
    }

    #[test]
    fn runpod_reserved_request_without_initial_intent_is_not_bootstrapped() {
        let f = Fixture::new();
        let p = f.preview(true);
        let allocation = f.submit(&p).expect("allocated");
        let store = f.store();
        let reserved = reserve(&store, &allocation);
        assert!(matches!(
            f.check(p.locator()).expect("check"),
            ConfiguredWorkspaceSetupObservation::Interrupted(_)
        ));
        assert_eq!(
            store
                .load_remote_allocation(OWNER, &p.locator.workspace_local_id)
                .expect("load"),
            Some(reserved)
        );
    }

    #[test]
    fn lost_reply_and_expired_setup_recover_only_the_retained_generation() {
        for cloud in [false, true] {
            let f = Fixture::new();
            let p = f.preview(cloud);
            assert_eq!(
                submit_with(&f.home, &f.config, OWNER, &p, &consent(&p), |store, saved| {
                    let allocation = reserve(store, &allocate(store, saved, &p)?);
                    if cloud {
                        store
                            .record_remote_first_pin_intent(&allocation)
                            .expect("initial intent");
                    }
                    let request = allocation.worker_request().expect("request");
                    assert!(
                        store
                            .claim_worker_creation(
                                request.workflow_id,
                                request.job_id,
                                &request.target,
                                "synthetic-worker"
                            )
                            .expect("claim")
                    );
                    expire(store, &allocation);
                    Err(Error::SetupUnconfirmed)
                })
                .err(),
                Some(Error::SetupUnconfirmed)
            );
            let store = f.store();
            let before = store
                .load_remote_allocation(OWNER, &p.locator.workspace_local_id)
                .expect("load")
                .expect("allocation");
            let calls = Cell::new(0);
            let observed = check_with(&f.home, &f.config, OWNER, p.locator(), |store, allocation| {
                calls.set(calls.get() + 1);
                store.record_remote_worker_recovery(allocation, None).map_err(storage)
            })
            .expect("noncreating check");
            let ConfiguredWorkspaceSetupObservation::Observed(after) = observed else {
                panic!("recovery");
            };
            assert_eq!(
                before.worker_request().expect("before"),
                after.worker_request().expect("after")
            );
            assert_eq!(
                before.workflow().workflow().retain_until_millis,
                after.workflow().workflow().retain_until_millis
            );
            assert_eq!(before.workspace().state().spec, after.workspace().state().spec);
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn split_snapshot_reads_refuse_before_recovery() {
        let f = Fixture::new();
        for cloud in [false, true] {
            let p = f.preview(cloud);
            let allocation = f.submit(&p).expect("allocation");
            let store = f.store();
            reserve(&store, &allocation);
            let result = check_saved_with(&f.home, &f.config, &store, allocation.workspace().clone(), |_, _| {
                panic!("recovery")
            });
            assert!(matches!(result, Err(Error::ContextChanged)));
            if !cloud {
                let identities = RemoteSshIdentityStore::new(&f.home);
                let provider = local_provider(&store, &f.config, &p.spec().target).expect("local provider");
                assert!(matches!(
                    super::super::super::recover(&store, &identities, &provider, &allocation),
                    Err(super::super::super::RemoteWorkspaceSetupError::Recovery(
                        crate::remote_workspace_recovery::RemoteWorkspaceRecoveryError::StateChanged
                    ))
                ));
            }
        }
    }

    #[test]
    fn callback_drift_cannot_be_published_as_success() {
        let f = Fixture::new();
        let p = f.preview(false);
        let allocation = f.submit(&p).expect("allocation");
        reserve(&f.store(), &allocation);
        let result = check_with(&f.home, &f.config, OWNER, p.locator(), |store, current| {
            let mut next = current.workspace().state().clone();
            next.spec.working_directory = "changed".into();
            store
                .replace_remote_workspace(current.workspace(), &next)
                .expect("fixture drift");
            Ok(current.clone())
        });
        assert!(matches!(result, Err(Error::SetupUnconfirmed)));
    }

    #[test]
    fn missing_or_foreign_check_never_creates_storage() {
        let f = Fixture::new();
        let p = f.preview(false);
        assert!(matches!(f.check(p.locator()), Err(Error::StorageUnavailable)));
        assert!(!f.home.root().exists());
        assert!(matches!(
            check_with(&f.home, &f.config, OTHER, p.locator(), |_, _| panic!("dispatch")),
            Err(Error::ContextChanged)
        ));
        let _store = f.store();
        assert!(matches!(
            f.check(p.locator()).expect("existing empty store"),
            ConfiguredWorkspaceSetupObservation::Missing
        ));
    }
}
