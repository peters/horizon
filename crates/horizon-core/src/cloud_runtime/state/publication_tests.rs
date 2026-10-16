use super::*;
use crate::cloud_runtime::{github::publish::publication::Account, publishing::Publication};
use horizon_cloud::image_publishing::{Destination, Publisher};

fn record(root: &Path) -> Deployment {
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"publishing-record","repository":root.join("source"),"revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4,
            "build":{"context":".","dockerfile":"Dockerfile"}},
        "stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    state.publication = Some(Box::new(Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination: Destination::bound("registry.example/team/worker".into(), "synthetic-grant".into()).unwrap(),
        account: None,
    }));
    state
}

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn persisted_publication_cannot_be_downgraded_or_lose_its_version_fence() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let state = record(root.path());
    store.save(&state).unwrap();
    let path = root.path().join("deployment.json");
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], PUBLISHING_RECORD_VERSION);
    assert_eq!(store.load().unwrap().unwrap().publication, state.publication);
    for missing in [false, true] {
        let mut changed = saved.clone();
        if missing {
            changed.as_object_mut().unwrap().remove("publication");
        } else {
            changed["version"] = 1.into();
        }
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(store.load().is_err());
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn an_invalid_publication_is_refused_on_read_and_cannot_replace_a_saved_cloud() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let state = record(root.path());
    store.save(&state).unwrap();
    let path = root.path().join("deployment.json");
    let before = std::fs::read(&path).unwrap();
    for case in 0..4 {
        let mut changed = state.clone();
        match case {
            0 => changed.profile.image = "registry.example/another/worker".into(),
            1 => changed.profile.build = None,
            2 => changed.publication.as_mut().unwrap().profile = "../another-profile".into(),
            _ => {
                changed.publication.as_mut().unwrap().account = Some(Account {
                    id: 42,
                    login: "publisher".into(),
                });
            }
        }
        assert!(store.save(&changed).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        changed.version = PUBLISHING_RECORD_VERSION;
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(store.load().is_err());
        std::fs::write(&path, &before).unwrap();
        assert!(store.load().unwrap().is_some());
    }
}

#[test]
fn personal_publication_requires_its_exact_nonsecret_account_metadata() {
    let root = tempfile::tempdir().unwrap();
    let mut state = record(root.path());
    state.version = PUBLISHING_RECORD_VERSION;
    state.profile.image = "ghcr.io/publisher/worker".into();
    let publication = state.publication.as_mut().unwrap();
    publication.destination = Destination {
        repository: state.profile.image.clone(),
        publisher: Publisher::GitHub {
            login: "publisher".into(),
        },
    };
    publication.account = Some(Account {
        id: 42,
        login: "publisher".into(),
    });
    assert!(publication.validate(&state.profile).is_ok());
    for account in [
        None,
        Some(Account {
            id: 42,
            login: "other-account".into(),
        }),
        Some(Account {
            id: 0,
            login: "publisher".into(),
        }),
    ] {
        state.publication.as_mut().unwrap().account = account;
        assert!(state.publication.as_ref().unwrap().validate(&state.profile).is_err());
    }
}
