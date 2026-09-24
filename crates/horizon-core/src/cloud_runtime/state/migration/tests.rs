use super::*;
use crate::cloud_runtime::{
    Stage,
    state::{Deployment, Store},
};
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    controller: ControllerId,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("legacy-cloud");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let deployment: Deployment = serde_json::from_value(json!({
            "version":1,"cloud_id":"legacy-cloud","repository":"/synthetic/repository","revision":"initial-commit",
            "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
            "stage":"Sessions","operation":{"state":"requested"},"spec":null,"worker":null,
            "sessions":[{"panel_id":"agent1","agent":"shell","tmux":"original-session","branch":"original-branch","worktree":"/workspace/original"}],
            "source_ready":true,"browserstack_released":false,"browserstack_targets":["owned-target"]
        })).unwrap();
        Store::lock(&root).unwrap().save(&deployment).unwrap();
        std::fs::write(root.join("known-hosts-worker1"), "synthetic-unchanged-pin").unwrap();
        Self {
            _temp: temp,
            root,
            controller: ControllerId::generate(),
        }
    }
    fn migrate(&self) -> Result<MigratedStore> {
        MigratedStore::migrate(&self.root, "saved-session", "workspace", self.controller)
    }
    fn fail_at(&self, stop: Boundary) {
        let result = migrate_with(&self.root, "saved-session", "workspace", self.controller, &mut |at| {
            if at == stop {
                Err(Error::Invalid("simulated crash"))
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(Error::Invalid("simulated crash"))), "{stop:?}");
    }
    fn intent(&self) -> Intent {
        read_intent(&self.root).unwrap().unwrap()
    }
    fn allocation_root(&self) -> PathBuf {
        self.root
            .parent()
            .unwrap()
            .join(".allocations")
            .join(self.intent().allocation.to_string())
    }
}

#[test]
fn every_publication_boundary_recovers_the_same_identity_and_complete_state() {
    for boundary in [
        Boundary::Discovered,
        Boundary::Refreshed,
        Boundary::Barrier,
        Boundary::Backup,
        Boundary::Journal(transaction::Boundary::Intent),
        Boundary::Journal(transaction::Boundary::Allocation),
        Boundary::Journal(transaction::Boundary::Project),
        Boundary::Journal(transaction::Boundary::Complete),
        Boundary::Complete,
    ] {
        let fixture = Fixture::new();
        let original = required(&fixture.root.join(DEPLOYMENT)).unwrap();
        fixture.fail_at(boundary);
        let intent = fixture.intent();
        {
            let old_reader = Store::lock(&fixture.root).unwrap();
            if [Boundary::Discovered, Boundary::Refreshed].contains(&boundary) {
                assert!(old_reader.load().is_ok());
            } else {
                assert!(old_reader.load().is_err());
            }
        }
        for _ in 0..2 {
            let mut store = fixture.migrate().unwrap();
            let records = store.load().unwrap();
            assert_eq!(records.identity(), &intent.identity);
            assert_eq!(records.allocation_id(), intent.allocation);
            let expected: Deployment = serde_json::from_slice(&original).unwrap();
            assert_eq!(
                serde_json::to_value(records.deployment()).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
            assert_eq!(required(&fixture.root.join(BACKUP)).unwrap(), original);
            assert_eq!(
                required(&fixture.root.join("known-hosts-worker1")).unwrap(),
                b"synthetic-unchanged-pin"
            );
            assert_eq!(store.root(), fixture.root.canonicalize().unwrap());
        }
    }
}

#[test]
fn old_controller_updates_before_the_barrier_replace_the_discovery_snapshot() {
    for boundary in [Boundary::Discovered, Boundary::Refreshed] {
        let fixture = Fixture::new();
        fixture.fail_at(boundary);
        let intent = fixture.intent();
        let expected = {
            let old = Store::lock(&fixture.root).unwrap();
            let mut state = old.load().unwrap().unwrap();
            state.stage = Stage::Stopping;
            state.stop_requested = true;
            state.sessions[0].tmux = "later-session".into();
            state.browserstack_targets.insert("later-target".into());
            old.save(&state).unwrap();
            std::fs::write(fixture.root.join("known-hosts-worker1"), "later-pin").unwrap();
            state
        };
        let mut store = fixture.migrate().unwrap();
        let records = store.load().unwrap();
        assert_eq!(records.identity(), &intent.identity);
        assert_eq!(records.allocation_id(), intent.allocation);
        assert_eq!(
            serde_json::to_value(records.deployment()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(fixture.intent().companions["known-hosts-worker1"], b"later-pin");
    }
}

#[test]
fn changed_companions_after_the_barrier_preserve_the_fence() {
    for name in [
        "known-hosts-worker1",
        "workspace-volume.required",
        "workspace-volume.json",
    ] {
        let fixture = Fixture::new();
        fixture.fail_at(Boundary::Barrier);
        let marker = required(&fixture.root.join(DEPLOYMENT)).unwrap();
        std::fs::write(fixture.root.join(name), "conflicting-data").unwrap();
        assert!(fixture.migrate().is_err());
        assert_eq!(required(&fixture.root.join(DEPLOYMENT)).unwrap(), marker);
        assert!(!fixture.allocation_root().join("allocation.json").exists());
    }
}

#[test]
fn missing_or_corrupt_companions_block_before_the_old_reader_barrier() {
    for name in ["workspace-volume.required", "workspace-volume.json"] {
        let fixture = Fixture::new();
        let original = required(&fixture.root.join(DEPLOYMENT)).unwrap();
        std::fs::write(fixture.root.join(name), "corrupt").unwrap();
        assert!(fixture.migrate().is_err());
        assert_eq!(required(&fixture.root.join(DEPLOYMENT)).unwrap(), original);
        assert!(!fixture.root.join(INTENT).exists());
    }
}

#[test]
fn foreign_context_and_copied_journals_cannot_adopt_a_migration() {
    let fixture = Fixture::new();
    fixture.fail_at(Boundary::Discovered);
    let before = required(&fixture.root.join(INTENT)).unwrap();
    assert!(MigratedStore::migrate(&fixture.root, "other-session", "workspace", fixture.controller).is_err());
    assert!(MigratedStore::migrate(&fixture.root, "saved-session", "other-workspace", fixture.controller).is_err());
    assert!(MigratedStore::migrate(&fixture.root, "saved-session", "workspace", ControllerId::generate()).is_err());
    let other = Fixture::new();
    std::fs::write(other.root.join(INTENT), &before).unwrap();
    assert!(MigratedStore::migrate(&other.root, "saved-session", "workspace", fixture.controller).is_err());
    assert_eq!(required(&fixture.root.join(INTENT)).unwrap(), before);
}

#[test]
fn completed_journal_never_recreates_missing_or_conflicting_projections() {
    for allocation in [true, false] {
        for replacement in [None, Some(b"corrupt".as_slice())] {
            let fixture = Fixture::new();
            drop(fixture.migrate().unwrap());
            let path = if allocation {
                fixture.allocation_root().join("allocation.json")
            } else {
                fixture.root.join(DEPLOYMENT)
            };
            match replacement {
                Some(bytes) => std::fs::write(&path, bytes).unwrap(),
                None => std::fs::remove_file(&path).unwrap(),
            }
            assert!(fixture.migrate().is_err());
            assert_eq!(transaction::read_optional(&path).unwrap().as_deref(), replacement);
        }
    }
}

#[test]
fn project_and_allocation_locks_exclude_other_controllers_and_release_on_drop() {
    let fixture = Fixture::new();
    let store = fixture.migrate().unwrap();
    assert!(matches!(fixture.migrate(), Err(Error::Busy)));
    assert!(matches!(Store::lock(&fixture.root), Err(Error::Busy)));
    drop(store);
    let allocation_lock = transaction::lock_file(&fixture.allocation_root()).unwrap();
    assert!(matches!(fixture.migrate(), Err(Error::Busy)));
    assert!(Store::lock(&fixture.root).is_ok());
    drop(allocation_lock);
    assert!(fixture.migrate().is_ok());
}

#[test]
fn ordinary_updates_retain_ownership_and_both_records_across_reopen() {
    let fixture = Fixture::new();
    {
        let mut store = fixture.migrate().unwrap();
        let original = store.load().unwrap();
        let mut state = original.deployment();
        state.stage = Stage::Stopping;
        state.stop_requested = true;
        let bytes = serde_json::to_vec(&state).unwrap();
        let next = Records::from_legacy(
            &bytes,
            original.identity().clone(),
            original.allocation_id(),
            fixture.controller,
        )
        .unwrap();
        store.save(&next).unwrap();
        let foreign = Records::from_legacy(
            &bytes,
            original.identity().clone(),
            original.allocation_id(),
            ControllerId::generate(),
        )
        .unwrap();
        assert!(store.save(&foreign).is_err());
    }
    let restored = fixture.migrate().unwrap().load().unwrap();
    assert_eq!(restored.deployment().stage, Stage::Stopping);
    assert!(restored.deployment().stop_requested);
}

#[test]
fn interrupted_runtime_updates_reconcile_only_the_intended_pair() {
    for boundary in [
        transaction::Boundary::Intent,
        transaction::Boundary::Allocation,
        transaction::Boundary::Project,
        transaction::Boundary::Complete,
    ] {
        let fixture = Fixture::new();
        {
            let mut store = fixture.migrate().unwrap();
            let before = store.load().unwrap();
            let mut next = before.deployment();
            next.stage = Stage::Stopping;
            next.stop_requested = true;
            let next = Records::from_legacy(
                &serde_json::to_vec(&next).unwrap(),
                before.identity().clone(),
                before.allocation_id(),
                fixture.controller,
            )
            .unwrap();
            let result = store.pair.save_with(&next, &mut |at| {
                if at == boundary {
                    Err(Error::Invalid("simulated update crash"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err());
        }
        let restored = fixture.migrate().unwrap().load().unwrap();
        assert!(restored.deployment().stop_requested);
        assert_eq!(restored.deployment().stage, Stage::Stopping);
    }
}

#[test]
fn conflicting_pending_update_is_never_overwritten() {
    let fixture = Fixture::new();
    {
        let mut store = fixture.migrate().unwrap();
        let before = store.load().unwrap();
        let mut next = before.deployment();
        next.stage = Stage::Stopping;
        next.stop_requested = true;
        let next = Records::from_legacy(
            &serde_json::to_vec(&next).unwrap(),
            before.identity().clone(),
            before.allocation_id(),
            fixture.controller,
        )
        .unwrap();
        assert!(
            store
                .pair
                .save_with(&next, &mut |_| Err(Error::Invalid("simulated update crash")))
                .is_err()
        );
    }
    let project = fixture.root.join(DEPLOYMENT);
    std::fs::write(&project, "conflicting-project").unwrap();
    let allocation = required(&fixture.allocation_root().join("allocation.json")).unwrap();
    assert!(fixture.migrate().is_err());
    assert_eq!(required(&project).unwrap(), b"conflicting-project");
    assert_eq!(
        required(&fixture.allocation_root().join("allocation.json")).unwrap(),
        allocation
    );
}

#[test]
fn duplicated_provider_ownership_is_flagged_instead_of_merged() {
    let fixture = Fixture::new();
    let sibling = fixture.root.parent().unwrap().join("other-cloud");
    std::fs::create_dir(&sibling).unwrap();
    {
        let old = Store::lock(&fixture.root).unwrap();
        let mut state = old.load().unwrap().unwrap();
        state.operation = horizon_cloud::CreateState::Bound {
            worker_id: "same-worker".into(),
        };
        old.save(&state).unwrap();
        state.cloud_id = "other-cloud".into();
        Store::lock(&sibling).unwrap().save(&state).unwrap();
    }
    assert!(fixture.migrate().is_err());
    assert_eq!(Store::lock(&fixture.root).unwrap().load().unwrap().unwrap().version, 1);
    assert_eq!(Store::lock(&sibling).unwrap().load().unwrap().unwrap().version, 1);
}

fn storage_fixture(fixture: &Fixture) -> serde_json::Value {
    let store = Store::lock(&fixture.root).unwrap();
    let mut state = store.load().unwrap().unwrap();
    state.spec = Some(
        serde_json::from_value(json!({
            "operation_id":"legacy-cloud","image_digest":format!("example/worker@sha256:{}", "a".repeat(64)),
            "profile":state.profile,"public_key":"unchanged-public-key","registry_auth_id":null,
            "gpu_types":[],"cpu_flavors":[],"data_centers":["TEST-1"]
        }))
        .unwrap(),
    );
    store.save(&state).unwrap();
    let volume = horizon_cloud::runpod::volumes::Spec {
        operation_id: "legacy-cloud".into(),
        size: 20,
        data_center_id: "TEST-1".into(),
    };
    let record = json!({"version":1,"worker":state.spec,"spec":volume,"state":{"state":"requested"}});
    std::fs::write(fixture.root.join("workspace-volume.required"), "").unwrap();
    std::fs::write(
        fixture.root.join("workspace-volume.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    record
}

#[test]
fn uncertain_storage_updates_are_refreshed_and_preserved_without_side_effects() {
    let fixture = Fixture::new();
    let mut record = storage_fixture(&fixture);
    fixture.fail_at(Boundary::Discovered);
    record["state"] = json!({"state":"deleting","volume":{"id":"volume1","name":"horizon-volume-legacy-cloud","size":20,"dataCenterId":"TEST-1"}});
    let bytes = serde_json::to_vec(&record).unwrap();
    std::fs::write(fixture.root.join("workspace-volume.json"), &bytes).unwrap();
    drop(fixture.migrate().unwrap());
    assert_eq!(fixture.intent().companions["workspace-volume.json"], bytes);
    assert_eq!(required(&fixture.root.join("workspace-volume.json")).unwrap(), bytes);
    assert!(fixture.root.join("workspace-volume.required").exists());
}

#[test]
fn mismatched_storage_or_missing_required_journal_cannot_cross_the_barrier() {
    for missing in [false, true] {
        let fixture = Fixture::new();
        let mut record = storage_fixture(&fixture);
        if missing {
            std::fs::remove_file(fixture.root.join("workspace-volume.json")).unwrap();
        } else {
            record["spec"]["operation_id"] = json!("other-owner");
            std::fs::write(
                fixture.root.join("workspace-volume.json"),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
        }
        assert!(fixture.migrate().is_err());
        assert!(Store::lock(&fixture.root).unwrap().load().is_ok());
        assert!(fixture.root.join("workspace-volume.required").exists());
    }
}

#[test]
fn completed_reopen_and_live_handle_reject_missing_companion_fences() {
    for name in [
        "workspace-volume.json",
        "workspace-volume.required",
        "known-hosts-worker1",
    ] {
        let fixture = Fixture::new();
        storage_fixture(&fixture);
        let mut store = fixture.migrate().unwrap();
        std::fs::remove_file(fixture.root.join(name)).unwrap();
        assert!(store.load().is_err());
        drop(store);
        assert!(fixture.migrate().is_err());
    }
    let fixture = Fixture::new();
    storage_fixture(&fixture);
    drop(fixture.migrate().unwrap());
    std::fs::write(fixture.root.join("workspace-volume.json"), "corrupt").unwrap();
    assert!(fixture.migrate().is_err());
}

#[test]
fn duplicate_guard_uses_the_neighbors_latest_and_pending_worker_binding() {
    for pending in [false, true] {
        let fixture = Fixture::new();
        {
            let mut store = fixture.migrate().unwrap();
            let before = store.load().unwrap();
            let mut next = before.deployment();
            next.operation = horizon_cloud::CreateState::Bound {
                worker_id: "later-worker".into(),
            };
            let next = Records::from_legacy(
                &serde_json::to_vec(&next).unwrap(),
                before.identity().clone(),
                before.allocation_id(),
                fixture.controller,
            )
            .unwrap();
            if pending {
                assert!(
                    store
                        .pair
                        .save_with(&next, &mut |_| Err(Error::Invalid("simulated update crash")))
                        .is_err()
                );
            } else {
                store.save(&next).unwrap();
            }
        }
        let sibling = fixture.root.parent().unwrap().join("other-cloud");
        std::fs::create_dir(&sibling).unwrap();
        let mut other = records(&fixture.intent()).unwrap().deployment();
        other.cloud_id = "other-cloud".into();
        other.operation = horizon_cloud::CreateState::Bound {
            worker_id: "later-worker".into(),
        };
        Store::lock(&sibling).unwrap().save(&other).unwrap();
        assert!(MigratedStore::migrate(&sibling, "saved-session", "workspace", fixture.controller).is_err());
        assert!(Store::lock(&sibling).unwrap().load().is_ok());
    }
}

#[test]
fn missing_neighbor_projections_and_orphaned_allocations_keep_ownership_fenced() {
    for remove_directory in [false, true] {
        let fixture = Fixture::new();
        {
            let old = Store::lock(&fixture.root).unwrap();
            let mut state = old.load().unwrap().unwrap();
            state.operation = horizon_cloud::CreateState::Bound {
                worker_id: "retained-worker".into(),
            };
            old.save(&state).unwrap();
        }
        drop(fixture.migrate().unwrap());
        let mut other = records(&fixture.intent()).unwrap().deployment();
        let sibling = fixture.root.parent().unwrap().join("other-cloud");
        std::fs::create_dir(&sibling).unwrap();
        other.cloud_id = "other-cloud".into();
        Store::lock(&sibling).unwrap().save(&other).unwrap();
        if remove_directory {
            std::fs::remove_dir_all(&fixture.root).unwrap();
        } else {
            std::fs::remove_file(fixture.root.join(DEPLOYMENT)).unwrap();
        }
        assert!(MigratedStore::migrate(&sibling, "saved-session", "workspace", fixture.controller).is_err());
        assert!(Store::lock(&sibling).unwrap().load().is_ok());
    }
}

#[test]
fn retained_trust_cannot_be_truncated_or_replaced() {
    for replacement in ["", "unrelated-pin"] {
        let fixture = Fixture::new();
        let mut store = fixture.migrate().unwrap();
        std::fs::write(fixture.root.join("known-hosts-worker1"), replacement).unwrap();
        assert!(store.load().is_err());
        drop(store);
        assert!(fixture.migrate().is_err());
    }
}

#[test]
fn recreated_workerless_project_cannot_bypass_its_orphaned_allocation() {
    for operation in [json!({"state":"prepared"}), json!({"state":"requested"})] {
        let fixture = Fixture::new();
        drop(fixture.migrate().unwrap());
        let mut legacy = serde_json::from_slice::<serde_json::Value>(&fixture.intent().legacy).unwrap();
        let allocation = required(&fixture.allocation_root().join("allocation.json")).unwrap();
        std::fs::remove_dir_all(&fixture.root).unwrap();
        std::fs::create_dir(&fixture.root).unwrap();
        legacy["operation"] = operation;
        std::fs::write(fixture.root.join(DEPLOYMENT), serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(fixture.migrate().is_err());
        assert_eq!(Store::lock(&fixture.root).unwrap().load().unwrap().unwrap().version, 1);
        let original = fixture.root.parent().unwrap().join(".allocations");
        assert!(std::fs::read_dir(original).unwrap().any(|entry| {
            transaction::read_optional(&entry.unwrap().path().join("allocation.json"))
                .unwrap()
                .as_ref()
                == Some(&allocation)
        }));
    }
}

#[test]
fn public_migration_canonicalizes_a_symlinked_parent_on_every_reopen() {
    let fixture = Fixture::new();
    let aliases = tempfile::tempdir().unwrap();
    let alias = aliases.path().join("linked-parent");
    std::os::unix::fs::symlink(fixture.root.parent().unwrap(), &alias).unwrap();
    let path = alias.join("legacy-cloud");
    let first = MigratedStore::migrate(&path, "saved-session", "workspace", fixture.controller)
        .unwrap()
        .load()
        .unwrap();
    let restored = MigratedStore::migrate(&path, "saved-session", "workspace", fixture.controller)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(first.allocation_id(), restored.allocation_id());
    assert_eq!(first.identity(), restored.identity());
}
