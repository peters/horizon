use super::*;
use std::path::PathBuf;

fn prepared(f: &Fixture) {
    let mut state = f.ready();
    state.worker = None;
    state.spec = None;
    state.operation = CreateState::Prepared;
    f.save(&state);
}

fn catalog(f: &Fixture) -> PathBuf {
    let path = f.root.path().join("tailnets.json");
    std::fs::write(&path, br#"{"tailnets":[{"id":"work","name":"Work"}]}"#).unwrap();
    path
}

fn disrupt_catalog(f: &Fixture, pending: bool) -> (PathBuf, Vec<u8>) {
    let path = f.root.path().join(if pending {
        "tailnets.pending.json"
    } else {
        "tailnets.json"
    });
    let bytes = b"retained uncertain catalog evidence".to_vec();
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

#[test]
fn fresh_no_network_requests_preserve_corrupt_or_pending_catalog_while_saved_networks_refuse() {
    for pending in [false, true] {
        for requested in [None, Some("none"), Some("work")] {
            let f = Fixture::new();
            prepared(&f);
            catalog(&f);
            let (path, bytes) = disrupt_catalog(&f, pending);
            let id = OperationId::generate();
            let result = submit_with_tailnet(&f.request(), Action::EnsureReady, id, requested);
            if requested == Some("work") {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().phase, Phase::Submitted);
                let target = f.root.path().join("target");
                assert!(
                    horizon_cloud::tailnet::Selection::load(&target)
                        .unwrap()
                        .tailnet
                        .is_none()
                );
                assert!(!target.join(format!("tailnet-commit-{id}.pending")).exists());
                assert_eq!(
                    std::fs::read(target.join(format!("tailnet-request-{id}.json"))).unwrap(),
                    br#"{"tailnet":null}"#
                );
            }
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

fn interrupted_request(f: &Fixture, requested: &str) -> (PathBuf, OperationId) {
    prepared(f);
    catalog(f);
    let target = f.root.path().join("target");
    let ownership = Store::lock(&target).unwrap();
    let catalog = crate::cloud_runtime::tailnet::store(f.root.path()).load().unwrap();
    horizon_cloud::tailnet::Selection::save(&target, Some("work"), &catalog).unwrap();
    let id = OperationId::generate();
    let choice = tailnet_choice::prepare(f.root.path(), &ownership, id, Some(requested)).unwrap();
    std::fs::remove_file(target.join("tailnet.json")).unwrap();
    std::fs::create_dir(target.join("tailnet.json")).unwrap();
    assert!(choice.commit(&ownership).is_err());
    std::fs::remove_dir(target.join("tailnet.json")).unwrap();
    horizon_cloud::tailnet::Selection::save(&target, Some("work"), &catalog).unwrap();
    (target, id)
}

#[test]
fn interrupted_none_recovery_retains_catalog_evidence_and_allocated_selection_fence() {
    for pending in [false, true] {
        for allocated in [false, true] {
            let f = Fixture::new();
            let (target, id) = interrupted_request(&f, "none");
            let request = target.join(format!("tailnet-request-{id}.json"));
            let saved = std::fs::read(&request).unwrap();
            let marker = target.join(format!("tailnet-commit-{id}.pending"));
            let (path, bytes) = disrupt_catalog(&f, pending);
            if allocated {
                f.save(&f.ready());
            }
            let ownership = Store::lock(&target).unwrap();
            let result = tailnet_choice::recover(&ownership, id);
            if allocated {
                assert!(result.is_err());
                assert_eq!(
                    horizon_cloud::tailnet::Selection::load(&target)
                        .unwrap()
                        .tailnet
                        .as_deref(),
                    Some("work")
                );
                assert_eq!(std::fs::read(&marker).unwrap(), b"1");
            } else {
                result.unwrap();
                assert!(
                    horizon_cloud::tailnet::Selection::load(&target)
                        .unwrap()
                        .tailnet
                        .is_none()
                );
                assert!(!marker.exists());
                crate::cloud_runtime::tailnet::validate_pending(&target).unwrap();
            }
            assert_eq!(std::fs::read(&request).unwrap(), saved);
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn no_network_recovery_refuses_malformed_marker_without_changing_selection_or_request() {
    for body in [b"0".as_slice(), b"11".as_slice(), b"".as_slice()] {
        let f = Fixture::new();
        let (target, id) = interrupted_request(&f, "none");
        let marker = target.join(format!("tailnet-commit-{id}.pending"));
        std::fs::write(&marker, body).unwrap();
        let request = target.join(format!("tailnet-request-{id}.json"));
        let original = std::fs::read(&request).unwrap();
        let (path, bytes) = disrupt_catalog(&f, true);
        let ownership = Store::lock(&target).unwrap();
        assert!(tailnet_choice::recover(&ownership, id).is_err());
        assert_eq!(
            horizon_cloud::tailnet::Selection::load(&target)
                .unwrap()
                .tailnet
                .as_deref(),
            Some("work")
        );
        assert_eq!(std::fs::read(&marker).unwrap(), body);
        assert_eq!(std::fs::read(&request).unwrap(), original);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn no_network_request_and_recovery_still_require_exclusive_catalog_ownership() {
    for recovering in [false, true] {
        let f = Fixture::new();
        let (target, id) = if recovering {
            interrupted_request(&f, "none")
        } else {
            prepared(&f);
            catalog(&f);
            (f.root.path().join("target"), OperationId::generate())
        };
        let prior = horizon_cloud::tailnet::Selection::load(&target).unwrap();
        let (path, bytes) = disrupt_catalog(&f, true);
        let held = crate::cloud_runtime::tailnet::store(f.root.path())
            .own_catalog()
            .unwrap();
        let worker_target = target.clone();
        let root = f.root.path().to_path_buf();
        let (entered, started) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let ownership = Store::lock(&worker_target).unwrap();
            entered.send(()).unwrap();
            let result = if recovering {
                tailnet_choice::recover(&ownership, id)
            } else {
                tailnet_choice::prepare(&root, &ownership, id, Some("none"))
                    .and_then(|choice| choice.commit(&ownership))
            };
            done.send(result).unwrap();
        });
        started.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let early = finished.recv_timeout(std::time::Duration::from_millis(150));
        let waited = matches!(early, Err(std::sync::mpsc::RecvTimeoutError::Timeout));
        assert_eq!(horizon_cloud::tailnet::Selection::load(&target).unwrap(), prior);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(held);
        let result = early.unwrap_or_else(|_| finished.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
        worker.join().unwrap();
        assert!(waited, "the production path must wait for catalog ownership");
        result.unwrap();
        assert!(
            horizon_cloud::tailnet::Selection::load(&target)
                .unwrap()
                .tailnet
                .is_none()
        );
        assert!(!target.join(format!("tailnet-commit-{id}.pending")).exists());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn no_network_recovery_refuses_linked_marker_without_changing_foreign_evidence() {
    let f = Fixture::new();
    let (target, id) = interrupted_request(&f, "none");
    let marker = target.join(format!("tailnet-commit-{id}.pending"));
    let foreign = f.root.path().join("foreign-marker");
    std::fs::write(&foreign, b"1").unwrap();
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(&foreign, &marker).unwrap();
    let request = target.join(format!("tailnet-request-{id}.json"));
    let saved = std::fs::read(&request).unwrap();
    let ownership = Store::lock(&target).unwrap();
    assert!(tailnet_choice::recover(&ownership, id).is_err());
    assert_eq!(std::fs::read_link(&marker).unwrap(), foreign);
    assert_eq!(std::fs::read(&foreign).unwrap(), b"1");
    assert_eq!(std::fs::read(&request).unwrap(), saved);
    assert_eq!(
        horizon_cloud::tailnet::Selection::load(&target)
            .unwrap()
            .tailnet
            .as_deref(),
        Some("work")
    );
}

#[test]
fn saved_network_recovery_still_refuses_corrupt_or_pending_catalog() {
    for pending in [false, true] {
        let f = Fixture::new();
        let (target, id) = interrupted_request(&f, "work");
        let request = target.join(format!("tailnet-request-{id}.json"));
        let saved = std::fs::read(&request).unwrap();
        let marker = target.join(format!("tailnet-commit-{id}.pending"));
        let (path, bytes) = disrupt_catalog(&f, pending);
        let ownership = Store::lock(&target).unwrap();
        assert!(tailnet_choice::recover(&ownership, id).is_err());
        assert_eq!(
            horizon_cloud::tailnet::Selection::load(&target)
                .unwrap()
                .tailnet
                .as_deref(),
            Some("work")
        );
        assert_eq!(std::fs::read(&request).unwrap(), saved);
        assert_eq!(std::fs::read(&marker).unwrap(), b"1");
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
