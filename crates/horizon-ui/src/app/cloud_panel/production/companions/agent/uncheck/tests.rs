//! An agent's uncheck against the card's own state.
use super::super::super::State;
use super::*;
use horizon_core::cloud_runtime::companions::{Catalog, Companion, Row, Snapshot, Status};

fn groups() -> horizon_core::cloud_panel::CloudGroups {
    use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudLaunch};
    let config = CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 32\n",
    )
    .unwrap();
    let mut group = CloudGroup::new(1, "Source".into(), "workspace1".into(), "/synthetic".into(), [0.0, 0.0]);
    group.remote = Some(CloudLaunch {
        publishing_selection: None,
        deployment_started: true,
        id: "source".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles["dev"].clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    horizon_core::cloud_panel::CloudGroups(vec![group])
}

fn snapshot(selected: bool) -> Snapshot {
    Snapshot {
        catalog: Catalog {
            version: 1,
            source_cloud_id: "source".into(),
            observed_at: 1,
            companions: vec![],
        },
        rows: vec![Row {
            companion: Companion {
                alias: "app".into(),
                repository: "example/app".into(),
                profile: "cpu".into(),
                selected,
                status: if selected { Status::Ready } else { Status::Unselected },
                target_cloud_id: selected.then(|| "target".into()),
                access: None,
            },
            candidates: vec![],
            error: None,
        }],
        publication_error: None,
        notice: None,
    }
}

#[test]
fn an_agent_unchecks_a_checked_companion_as_the_card_does() {
    let mut state = State::default();
    state.sync(Some("session"), &groups());
    let entry = state.entries.get_mut("source").unwrap();
    entry.snapshot = Some(snapshot(true));
    assert_eq!(
        uncheck(entry, "source", "app"),
        Ok(None),
        "checked: its deselection is saved next"
    );
    assert!(entry.clearing.is_empty(), "nothing is queued before the save");
    entry.clearing.insert("app".into());
    assert!(
        uncheck(entry, "source", "app")
            .unwrap_err()
            .starts_with("cloud_companion_busy"),
        "a removal still under way is not reported as done again"
    );
    entry.clearing.clear();
    entry.snapshot = Some(snapshot(false));
    let done = uncheck(entry, "source", "app").unwrap().unwrap();
    assert_eq!(done["changed"], false);
    assert_eq!(done["status"], "unselected");
    let mut pending = snapshot(false);
    pending.rows[0].companion.status = Status::RevocationPending;
    entry.snapshot = Some(pending);
    let answer = uncheck(entry, "source", "app").unwrap().unwrap();
    assert_eq!(answer["status"], "revocation_pending");
    assert!(answer["message"].as_str().unwrap().contains("not done"), "{answer}");
    entry.snapshot = Some(snapshot(false));
    entry.selecting.insert("other".into());
    assert!(
        uncheck(entry, "source", "app")
            .unwrap_err()
            .starts_with("cloud_companion_busy"),
        "a check the owner is making, of any companion, is not dropped or misreported"
    );
    entry.selecting.clear();
    assert!(
        uncheck(entry, "source", "other")
            .unwrap_err()
            .starts_with("cloud_companion_unknown_alias")
    );
    entry.snapshot = None;
    assert!(
        uncheck(entry, "source", "app")
            .unwrap_err()
            .starts_with("cloud_companion_unavailable")
    );
}

#[test]
fn a_pending_refresh_does_not_make_the_uncheck_busy() {
    let mut state = State::default();
    state.sync(Some("session"), &groups());
    let entry = state.entries.get_mut("source").unwrap();
    entry.snapshot = Some(snapshot(true));
    entry.pending = Some(Action::Refresh);
    assert_eq!(
        uncheck(entry, "source", "app"),
        Ok(None),
        "a refresh is no change in progress"
    );
    entry.pending = Some(Action::Select {
        alias: "app".into(),
        target_cloud_id: "target".into(),
    });
    assert!(
        uncheck(entry, "source", "app")
            .unwrap_err()
            .starts_with("cloud_companion_busy"),
        "a pending check is still a change in progress"
    );
}

#[test]
fn the_agent_hears_of_an_uncheck_only_once_it_is_saved() {
    let saved = answer("source", "app", Ok(true)).unwrap();
    assert_eq!(saved["queued"], true);
    assert!(saved["message"].as_str().unwrap().contains("saved"));
    assert!(
        answer("source", "app", Ok(false))
            .unwrap_err()
            .starts_with("cloud_companion_expired")
    );
    // A failed save is answered as save found it: unchanged, or not known.
    assert_eq!(answer("source", "app", Err("why".into())), Err("why".to_owned()));
    // A save after the agent stopped waiting writes nothing.
    let root = tempfile::tempdir().unwrap();
    let owner = Owner {
        scope: horizon_core::cloud_runtime::companions::Scope {
            session_id: "session".into(),
            workspace_id: "workspace1".into(),
        },
        cloud_id: "source".into(),
    };
    assert_eq!(save(root.path(), &owner, "app", 0), Ok(false));
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}
