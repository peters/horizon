use super::*;

struct CanonicalTemp {
    _directory: tempfile::TempDir,
    path: PathBuf,
}
impl CanonicalTemp {
    fn path(&self) -> &Path {
        &self.path
    }
}
fn canonical_temp() -> CanonicalTemp {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap();
    CanonicalTemp {
        _directory: directory,
        path,
    }
}

fn journal(path: &Path, realm: char) -> Journal {
    Journal {
        store: Store::open(path).unwrap(),
        realm: realm.to_string().repeat(64),
    }
}

fn quota() -> Result<Capacity> {
    Capacity::observed(
        Quota {
            parallel_sessions_max_allowed: 2,
            team_parallel_sessions_max_allowed: 2,
            parallel_sessions_running: 0,
            queued_sessions: 0,
        },
        BTreeSet::new(),
    )
}

#[test]
fn restart_preserves_owned_intent_and_never_serializes_provider_ids_to_status() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let owner = Uuid::new_v4();
    let first = journal(&state, 'a');
    let operation = first
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    first.reserve(owner, operation.id, quota).unwrap();
    first
        .allocated(owner, operation.id, "synthetic_session_0123456789")
        .unwrap();
    drop(first);
    let reopened = journal(&state, 'a');
    let status = reopened.status(owner, operation.id).unwrap();
    assert_eq!(status.phase, Phase::Active);
    assert_eq!(status.pending_resources, 1);
    let output = serde_json::to_string(&status).unwrap();
    assert!(!output.contains("synthetic_session"));
    assert!(!output.contains(&"a".repeat(64)));
    assert_eq!(
        reopened.status(Uuid::new_v4(), operation.id).err(),
        Some(Error::OwnershipRefused)
    );
    assert_eq!(
        journal(&state, 'b').status(owner, operation.id).err(),
        Some(Error::OwnershipRefused)
    );
    assert_eq!(reopened.pending(owner).unwrap().len(), 1);
    assert_eq!(
        reopened.retire(owner, operation.id).err(),
        Some(Error::OperationInvalid)
    );
    reopened.confirm_released(owner, operation.id).unwrap();
    reopened.confirm_released(owner, operation.id).unwrap();
    reopened.retire(owner, operation.id).unwrap();
    assert!(reopened.pending(owner).unwrap().is_empty());
}

#[test]
fn simultaneous_profiles_share_capacity_without_crossing_credential_ownership() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let (ready_send, ready) = std::sync::mpsc::channel();
    let workers = (0..4)
        .map(|index| {
            let state = state.clone();
            let root = folder.path().to_owned();
            let ready_send = ready_send.clone();
            let (release, released) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let prepared = (|| {
                    let journal = Journal {
                        store: Store::open(&state)?,
                        realm: if index % 2 == 0 { "a" } else { "b" }.repeat(64),
                    };
                    let owner = Uuid::new_v4();
                    let operation = journal.start(owner, &root, Kind::Session, Duration::from_secs(30))?;
                    Ok::<_, Error>((journal, owner, operation))
                })();
                let _ = ready_send.send(prepared.is_ok());
                let (journal, owner, operation) = prepared?;
                if released.recv_timeout(Duration::from_secs(10)) != Ok(true) {
                    return Err(Error::JournalUnavailable);
                }
                journal.reserve(owner, operation.id, quota)
            });
            (release, worker)
        })
        .collect::<Vec<_>>();
    let initialization = (0..4)
        .map(|_| ready.recv_timeout(Duration::from_secs(10)) == Ok(true))
        .collect::<Vec<_>>();
    let admitted = initialization.iter().all(|ready| *ready);
    for (release, _) in &workers {
        let _ = release.send(admitted);
    }
    // Join every worker before any assertion can destroy the fixture directory.
    let outcomes = workers.into_iter().map(|(_, worker)| worker.join()).collect::<Vec<_>>();
    assert!(admitted, "native journal initialization failed: {outcomes:?}");
    let outcomes = outcomes.into_iter().map(|result| result.unwrap()).collect::<Vec<_>>();
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 2);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == Err(Error::CapacityUnavailable))
            .count(),
        2
    );
}

#[test]
fn remote_visibility_does_not_double_count_owned_allocations_and_uncertainty_holds_capacity() {
    let folder = canonical_temp();
    let journal = journal(&folder.path().join("state"), 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    journal.allocated(owner, a.id, "synthetic_session_0123456789").unwrap();
    let b = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal
        .reserve(owner, b.id, || {
            Capacity::observed(
                Quota {
                    parallel_sessions_running: 1,
                    ..quota().unwrap().quota
                },
                BTreeSet::from(["synthetic_session_0123456789".to_owned()]),
            )
        })
        .unwrap();
    journal.uncertain(owner, b.id).unwrap();
    let c = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        journal.reserve(owner, c.id, quota).err(),
        Some(Error::CapacityUnavailable)
    );
    journal.confirm_released(owner, b.id).unwrap();
    journal.reserve(owner, c.id, quota).unwrap();
}

#[test]
fn expired_and_uncertain_reservations_survive_restart_and_block_reallocation() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let journal = journal(&state, 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    journal.uncertain(owner, a.id).unwrap();
    journal
        .edit(|ledger| {
            let record = journal.record(ledger, owner, a.id)?;
            record.created = now()? - 10;
            record.deadline = now()? - 1;
            Ok(())
        })
        .unwrap();
    assert!(journal.status(owner, a.id).unwrap().expired);
    let b = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        journal
            .reserve(owner, b.id, || Capacity::observed(
                Quota {
                    parallel_sessions_max_allowed: 1,
                    ..quota().unwrap().quota
                },
                BTreeSet::new()
            ))
            .err(),
        Some(Error::CapacityUnavailable)
    );
    drop(journal);
    assert_eq!(super::tests::journal(&state, 'a').pending(owner).unwrap().len(), 2);
}

#[test]
fn journal_rejects_symlinks_fifos_shared_permissions_and_corruption() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let journal = journal(&state, 'a');
    let target = folder.path().join("target");
    std::fs::write(&target, b"private").unwrap();
    std::fs::remove_file(state.join("journal.json")).unwrap();
    symlink(&target, state.join("journal.json")).unwrap();
    assert_eq!(journal.pending(Uuid::new_v4()).err(), Some(Error::JournalUnavailable));
    std::fs::remove_file(state.join("journal.json")).unwrap();
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .args(["-m", "600"])
            .arg(state.join("journal.json"))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(journal.pending(Uuid::new_v4()).err(), Some(Error::JournalUnavailable));
    std::fs::remove_file(state.join("journal.json")).unwrap();
    std::fs::write(state.join("journal.json"), b"not json").unwrap();
    std::fs::set_permissions(state.join("journal.json"), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(journal.pending(Uuid::new_v4()).err(), Some(Error::JournalInvalid));
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Store::open(&state).is_err());
    let alias = folder.path().join("alias");
    symlink(&state, &alias).unwrap();
    assert!(Store::open(&alias).is_err());
}

#[test]
fn cleanup_failure_preserves_private_identity_and_never_replays_upload_or_allocation() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let journal = journal(&state, 'a');
    let owner = Uuid::new_v4();
    let session = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, session.id, quota).unwrap();
    journal
        .allocated(owner, session.id, "synthetic_session_0123456789")
        .unwrap();
    assert_eq!(
        journal
            .release_session(owner, session.id, |reference| {
                assert_eq!(reference, "synthetic_session_0123456789");
                Err(Error::CapacityUnavailable)
            })
            .err(),
        Some(Error::CapacityUnavailable)
    );
    assert_eq!(journal.status(owner, session.id).unwrap().phase, Phase::Uncertain);
    let upload = journal
        .start(owner, folder.path(), Kind::Upload, Duration::from_secs(30))
        .unwrap();
    journal.upload_intent(owner, upload.id, &"a".repeat(64)).unwrap();
    journal
        .uploaded(owner, upload.id, "bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .unwrap();
    assert!(
        !serde_json::to_string(&journal.status(owner, upload.id).unwrap())
            .unwrap()
            .contains("bs://")
    );
    assert_eq!(
        journal
            .release_upload(Uuid::new_v4(), upload.id, |_| panic!("foreign cleanup dispatched"))
            .err(),
        Some(Error::OwnershipRefused)
    );
    journal
        .release_upload(owner, upload.id, |reference| {
            assert_eq!(reference, "bs://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
            Ok(())
        })
        .unwrap();
    journal
        .release_upload(owner, upload.id, |_| panic!("confirmed cleanup replayed"))
        .unwrap();
    journal.release_session(owner, session.id, |_| Ok(())).unwrap();
    assert_eq!(journal.status(owner, session.id).unwrap().phase, Phase::Complete);
}

#[test]
fn count_overlap_requires_exact_provider_identity_and_uncertainty_cannot_free_capacity() {
    let folder = canonical_temp();
    let journal = journal(&folder.path().join("state"), 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    journal.allocated(owner, a.id, "synthetic_session_0123456789").unwrap();
    journal.uncertain(owner, a.id).unwrap();
    let b = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        journal
            .reserve(owner, b.id, || Capacity::observed(
                Quota {
                    parallel_sessions_running: 1,
                    ..quota().unwrap().quota
                },
                BTreeSet::from(["external_session_0123456789".into()])
            ))
            .err(),
        Some(Error::CapacityUnavailable)
    );
    journal
        .reserve(owner, b.id, || {
            Capacity::observed(
                Quota {
                    parallel_sessions_running: 1,
                    ..quota().unwrap().quota
                },
                BTreeSet::from(["synthetic_session_0123456789".into()]),
            )
        })
        .unwrap();
}

#[test]
fn lost_initialized_state_or_lock_fails_closed_and_failed_snapshot_leaves_no_partial() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let journal = journal(&state, 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    let before = std::fs::read(state.join("journal.json")).unwrap();
    store::FAIL_REPLACE.with(|flag| flag.set(true));
    assert_eq!(journal.uncertain(owner, a.id).err(), Some(Error::JournalUnavailable));
    store::FAIL_REPLACE.with(|flag| flag.set(false));
    assert_eq!(before, std::fs::read(state.join("journal.json")).unwrap());
    assert!(
        std::fs::read_dir(&state).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial"))
    );
    std::fs::remove_file(state.join("journal.json")).unwrap();
    assert_eq!(journal.pending(owner).err(), Some(Error::JournalInvalid));
    assert!(Store::open(&state).is_err());
    std::fs::write(state.join("journal.json"), before).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state.join("journal.json"), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::remove_file(state.join("journal.lock")).unwrap();
    assert_eq!(journal.pending(owner).err(), Some(Error::JournalInvalid));
    assert!(Store::open(&state).is_err());
}

#[test]
fn inconsistent_lifecycle_shapes_and_duplicate_provider_ids_block_admission() {
    let folder = canonical_temp();
    let journal = journal(&folder.path().join("state"), 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    journal.allocated(owner, a.id, "synthetic_session_0123456789").unwrap();
    assert_eq!(
        journal
            .edit(|ledger| {
                journal.record(ledger, owner, a.id)?.slot = None;
                Ok(())
            })
            .err(),
        Some(Error::JournalInvalid)
    );
    assert_eq!(
        journal
            .edit(|ledger| {
                journal.record(ledger, owner, a.id)?.kind = Kind::Upload;
                Ok(())
            })
            .err(),
        Some(Error::JournalInvalid)
    );
    let b = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal
        .reserve(owner, b.id, || {
            Capacity::observed(
                Quota {
                    parallel_sessions_running: 1,
                    ..quota().unwrap().quota
                },
                BTreeSet::from(["synthetic_session_0123456789".into()]),
            )
        })
        .unwrap();
    assert_eq!(
        journal.allocated(owner, b.id, "synthetic_session_0123456789").err(),
        Some(Error::JournalInvalid)
    );
}

#[test]
fn inconsistent_on_disk_state_blocks_reads_and_capacity_before_provider_queries() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let journal = journal(&state, 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    journal.allocated(owner, a.id, "synthetic_session_0123456789").unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join("journal.json")).unwrap()).unwrap();
    value["records"][a.id.to_string()]["slot"] = serde_json::Value::Null;
    std::fs::write(state.join("journal.json"), serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(journal.pending(owner).err(), Some(Error::JournalInvalid));
    assert_eq!(
        journal
            .reserve(owner, a.id, || panic!("malformed state contacted provider"))
            .err(),
        Some(Error::JournalInvalid)
    );
}

#[test]
fn duplicate_json_operation_keys_cannot_hide_an_owned_reservation() {
    let folder = canonical_temp();
    let state = folder.path().join("state");
    let journal = journal(&state, 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(30))
        .unwrap();
    journal.reserve(owner, a.id, quota).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(state.join("journal.json")).unwrap()).unwrap();
    let held = &value["records"][a.id.to_string()];
    let mut empty = held.clone();
    empty["phase"] = serde_json::json!("preparing");
    empty["slot"] = serde_json::Value::Null;
    empty["resources"] = serde_json::json!([]);
    let raw = format!(
        "{{\"version\":1,\"records\":{{\"{}\":{},\"{}\":{}}}}}",
        a.id, held, a.id, empty
    );
    std::fs::write(state.join("journal.json"), raw).unwrap();
    assert_eq!(journal.pending(owner).err(), Some(Error::JournalInvalid));
    assert_eq!(
        journal
            .reserve(owner, a.id, || panic!("duplicate state contacted provider"))
            .err(),
        Some(Error::JournalInvalid)
    );
}

#[test]
fn slow_capacity_reply_cannot_admit_an_expired_operation() {
    let folder = canonical_temp();
    let journal = journal(&folder.path().join("state"), 'a');
    let owner = Uuid::new_v4();
    let a = journal
        .start(owner, folder.path(), Kind::Session, Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        journal
            .reserve(owner, a.id, || {
                std::thread::sleep(Duration::from_millis(1100));
                quota()
            })
            .err(),
        Some(Error::OperationExpired)
    );
    let status = journal.status(owner, a.id).unwrap();
    assert_eq!(status.phase, Phase::Preparing);
    assert_eq!(status.pending_resources, 0);
}
