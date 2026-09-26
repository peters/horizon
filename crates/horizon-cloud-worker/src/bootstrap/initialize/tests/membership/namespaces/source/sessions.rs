use super::*;
use crate::bootstrap::sessions::{self, Boundary as SessionBoundary};
use horizon_cloud_protocol::membership::SessionId;
use std::time::Instant;

fn ensure(
    store: &Store,
    manifest: &Manifest,
    receipt: &Receipt,
    session_id: SessionId,
    deadline: Instant,
    checkpoint: &mut impl FnMut(SessionBoundary) -> io::Result<()>,
) -> io::Result<()> {
    let source = source::published(store, manifest, &receipt.identity, deadline)?;
    sessions::ensure(store, manifest, receipt, session_id, &source, deadline, checkpoint)
}

fn reserved_session() -> (Fixture, ProjectIdentity, SessionId) {
    let f = Fixture::ready();
    let (descriptor, bytes) = bytes();
    let project = agent_project(&f, "one", 8000);
    import(&f, &request(&f, &project, descriptor.clone()), &bytes, &mut |_| Ok(())).unwrap();
    let session = session(&descriptor.revision, Agent::Codex);
    f.change(&session_request(&f, &project, session.clone())).unwrap();
    (f, project, session.id)
}
fn prepare_request(f: &Fixture, project: &ProjectIdentity, session_id: SessionId) -> RecoveryRequest {
    f.membership(
        project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::PrepareSession { session_id },
    )
}
fn root(f: &Fixture, project: &ProjectIdentity, session_id: SessionId) -> std::path::PathBuf {
    project_root(f, project).join("worktrees").join(session_id.to_string())
}
fn anchored_intent(f: &Fixture, request: &RecoveryRequest) -> Receipt {
    let store = Store::open(&f.root()).unwrap();
    let original = store.read(MANIFEST).unwrap().unwrap();
    let manifest: Manifest = serde_json::from_slice(&original).unwrap();
    let (next, receipt) = manifest.next(&request.message, &request.payload).unwrap();
    store
        .write(MANIFEST, Some(&original), &serde_json::to_vec(&next).unwrap())
        .unwrap();
    receipt
}

#[test]
fn writable_sessions_preserve_edits_commits_config_and_siblings_after_restart() {
    let f = Fixture::ready();
    let (descriptor, bytes) = bytes();
    let mut saved = Vec::new();
    for (index, name) in ["one", "two", "three"].into_iter().enumerate() {
        let project = agent_project(&f, name, 8000 + u16::try_from(index).unwrap());
        import(&f, &request(&f, &project, descriptor.clone()), &bytes, &mut |_| Ok(())).unwrap();
        for agent in [Agent::Codex, Agent::Claude] {
            let session = session(&descriptor.revision, agent);
            f.change(&session_request(&f, &project, session.clone())).unwrap();
            let request = prepare_request(&f, &project, session.id);
            let receipt = f.change(&request).unwrap();
            let path = root(&f, &project, session.id);
            let checkout = path.join("checkout");
            assert_eq!(git(&checkout, &["status", "--porcelain"]), b"");
            assert_eq!(git(&checkout, &["show", "HEAD:committed"]), b"committed source");
            assert_eq!(
                git(&checkout, &["symbolic-ref", "HEAD"]),
                format!("refs/heads/projects/{}/{}\n", project.project_id(), session.id).as_bytes()
            );
            git(&checkout, &["config", "user.name", "Fixture"]);
            git(&checkout, &["config", "user.email", "fixture@example.invalid"]);
            fs::write(checkout.join("committed"), "new committed edit").unwrap();
            git(&checkout, &["commit", "-am", "Retained edit"]);
            let new_head = git(&checkout, &["rev-parse", "HEAD"]);
            fs::write(checkout.join("committed"), "dirty tracked").unwrap();
            fs::write(checkout.join("untracked"), "dirty untracked").unwrap();
            fs::write(path.join("home/settings"), "private home").unwrap();
            // Recovery must not invoke Git on this now user-controlled config.
            fs::write(checkout.join(".git/config.next"), "invalid git configuration\n").unwrap();
            fs::rename(checkout.join(".git/config.next"), checkout.join(".git/config")).unwrap();
            saved.push((project.clone(), request, receipt, path, new_head));
        }
    }
    boot(&f).unwrap();
    for (_, request, receipt, path, head) in &saved {
        assert_eq!(&f.change(request).unwrap(), receipt);
        assert_eq!(fs::read(path.join("checkout/committed")).unwrap(), b"dirty tracked");
        assert_eq!(fs::read(path.join("checkout/untracked")).unwrap(), b"dirty untracked");
        assert_eq!(fs::read(path.join("home/settings")).unwrap(), b"private home");
        let branch = fs::read_to_string(path.join("checkout/.git/HEAD")).unwrap();
        assert_eq!(
            fs::read(
                path.join("checkout/.git")
                    .join(branch.trim().strip_prefix("ref: ").unwrap())
            )
            .unwrap(),
            *head
        );
    }
    f.change(&cancel(&f, &saved[0].0)).unwrap();
    boot(&f).unwrap();
    assert!(f.change(&saved[0].1).is_err());
    for (_, _, _, path, _) in saved {
        assert!(path.join("checkout/untracked").exists());
    }
}

#[test]
fn publication_boundary_recovery_never_resets_exposed_data_or_restarts_uncertain_builds() {
    for failure in [
        SessionBoundary::Anchored,
        SessionBoundary::Started,
        SessionBoundary::Built,
        SessionBoundary::Ready,
        SessionBoundary::Published,
        SessionBoundary::Synced,
    ] {
        let (f, project, session_id) = reserved_session();
        let request = prepare_request(&f, &project, session_id);
        let receipt = anchored_intent(&f, &request);
        {
            let store = Store::open(&f.root()).unwrap();
            let manifest = f.manifest();
            assert!(
                ensure(
                    &store,
                    &manifest,
                    &receipt,
                    session_id,
                    Instant::now() + Source::WORKER_TIMEOUT,
                    &mut |at| { if at == failure { Err(invalid()) } else { Ok(()) } }
                )
                .is_err()
            );
        }
        let path = root(&f, &project, session_id);
        if path.exists() {
            fs::write(path.join("checkout/committed"), "edit before acknowledgement").unwrap();
        }
        boot(&f).unwrap();
        assert!(f.change(&cancel(&f, &project)).is_err());
        let retry = f.change(&request);
        if matches!(failure, SessionBoundary::Started | SessionBoundary::Built) {
            assert!(retry.is_err());
            assert!(f.change(&request).is_err());
            assert_eq!(
                fs::read_dir(f.root())
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().starts_with(".session-"))
                    .count(),
                1
            );
        } else {
            retry.unwrap();
            if matches!(failure, SessionBoundary::Published | SessionBoundary::Synced) {
                assert_eq!(
                    fs::read(path.join("checkout/committed")).unwrap(),
                    b"edit before acknowledgement"
                );
            }
            f.change(&cancel(&f, &project)).unwrap();
        }
    }
}

#[test]
fn preparation_rejects_foreign_sessions_missing_capabilities_and_replaced_roots() {
    let (f, project, session_id) = reserved_session();
    let foreign = prepared(&f, "foreign", 8001);
    assert!(f.change(&prepare_request(&f, &foreign, session_id)).is_err());
    let request = prepare_request(&f, &project, session_id);
    let before = f.manifest();
    assert!(
        mutate(
            &Store::open(&f.root()).unwrap(),
            &f.runtime,
            &request,
            Action::PrepareProjectSession,
            |_| Err(invalid()),
            &mut |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(f.manifest(), before);
    f.change(&request).unwrap();
    let path = root(&f, &project, session_id);
    fs::rename(&path, path.with_extension("old")).unwrap();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(f.change(&request).is_err());
    assert!(boot(&f).is_err());
    assert!(f.change(&cancel(&f, &project)).is_err());
}

#[test]
fn changed_ready_staging_and_replaced_child_roots_are_never_acknowledged() {
    for change in ["ready-content", "home-symlink", "checkout-directory", "unanchored"] {
        let (f, project, session_id) = reserved_session();
        let request = prepare_request(&f, &project, session_id);
        if change == "unanchored" {
            fs::create_dir(f.root().join(format!(".session-{session_id}.next"))).unwrap();
            assert!(f.change(&request).is_err());
        } else if change == "ready-content" {
            let receipt = anchored_intent(&f, &request);
            let store = Store::open(&f.root()).unwrap();
            assert!(
                ensure(
                    &store,
                    &f.manifest(),
                    &receipt,
                    session_id,
                    Instant::now() + Source::WORKER_TIMEOUT,
                    &mut |at| {
                        if at == SessionBoundary::Ready {
                            Err(invalid())
                        } else {
                            Ok(())
                        }
                    }
                )
                .is_err()
            );
            drop(store);
            fs::write(
                f.root().join(format!(".session-{session_id}.next/checkout/committed")),
                "changed staged source",
            )
            .unwrap();
            assert!(f.change(&request).is_err());
            assert!(!root(&f, &project, session_id).exists());
        } else {
            f.change(&request).unwrap();
            let path = root(&f, &project, session_id).join(if change == "home-symlink" { "home" } else { "checkout" });
            fs::rename(&path, path.with_extension("saved")).unwrap();
            if change == "home-symlink" {
                symlink(path.with_extension("saved"), &path).unwrap();
            } else {
                fs::create_dir(&path).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            assert!(f.change(&request).is_err());
            assert!(boot(&f).is_err());
        }
        assert!(f.change(&cancel(&f, &project)).is_err());
    }
}

#[test]
fn ready_retry_sync_failure_prevents_exposure_and_expired_deadlines_do_no_work() {
    let (f, project, session_id) = reserved_session();
    let request = prepare_request(&f, &project, session_id);
    let receipt = anchored_intent(&f, &request);
    let store = Store::open(&f.root()).unwrap();
    let manifest = f.manifest();
    assert!(ensure(&store, &manifest, &receipt, session_id, Instant::now(), &mut |_| Ok(())).is_err());
    assert!(!f.root().join(format!(".session-{session_id}.next")).exists());
    assert!(
        ensure(
            &store,
            &manifest,
            &receipt,
            session_id,
            Instant::now() + Source::WORKER_TIMEOUT,
            &mut |at| {
                if at == SessionBoundary::Ready {
                    Err(invalid())
                } else {
                    Ok(())
                }
            }
        )
        .is_err()
    );
    store.fail_sync_after(0);
    assert!(
        ensure(
            &store,
            &manifest,
            &receipt,
            session_id,
            Instant::now() + Source::WORKER_TIMEOUT,
            &mut |_| Ok(())
        )
        .is_err()
    );
    assert!(!root(&f, &project, session_id).exists());
    drop(store);
    f.change(&request).unwrap();
}

#[test]
fn helper_enforces_budgets_before_writes_and_never_follows_committed_links() {
    let (f, project, session_id) = reserved_session();
    let source = project_root(&f, &project).join("repository/source");
    let destination = f.directory.path().join("helper-test");
    fs::create_dir(&destination).unwrap();
    let result = Command::new("/usr/bin/python3")
        .args([
            "-I",
            "-c",
            include_str!("sessions/helper.py"),
            include_str!("../../../../../sessions/checkout.py"),
        ])
        .arg(&destination)
        .arg(&f.manifest().members[0].sessions[0].revision)
        .arg(session_id.to_string())
        .arg(project.project_id().to_string())
        .current_dir(source)
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
}

#[test]
fn content_hashing_is_bounded_and_changed_source_before_publication_is_rejected() {
    let (f, project, session_id) = reserved_session();
    let request = prepare_request(&f, &project, session_id);
    let before = source::content_checks();
    f.change(&request).unwrap();
    assert!(
        source::content_checks() - before <= 3,
        "full source hashing must not run at each storage checkpoint"
    );
    let before = source::content_checks();
    f.change(&request).unwrap();
    assert!(source::content_checks() - before <= 2);

    let (f, project, session_id) = reserved_session();
    let request = prepare_request(&f, &project, session_id);
    let receipt = anchored_intent(&f, &request);
    let store = Store::open(&f.root()).unwrap();
    assert!(
        ensure(
            &store,
            &f.manifest(),
            &receipt,
            session_id,
            Instant::now() + Source::WORKER_TIMEOUT,
            &mut |at| {
                if at == SessionBoundary::Built {
                    fs::write(
                        project_root(&f, &project).join("repository/source/repository.git/HEAD"),
                        "changed immutable source",
                    )?;
                }
                Ok(())
            }
        )
        .is_err()
    );
    assert!(!root(&f, &project, session_id).exists());
}
