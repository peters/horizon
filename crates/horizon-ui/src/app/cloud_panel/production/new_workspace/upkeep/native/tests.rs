use super::jobs::{finished, save};
use super::*;
use horizon_core::upkeep::native::probe;

fn request() -> Request {
    Request {
        repository: "acme/web".into(),
        number: 7,
        head: "a".repeat(40),
        reason: String::new(),
    }
}

#[test]
fn a_test_ends_with_the_agents_outcome_or_at_its_time_limit() {
    let passed = probe::Outcome {
        state: Test::Passed,
        summary: "All scenarios passed.".into(),
    };
    let report = finished(
        &request(),
        Ok((Some(passed), "| ok |".into(), vec!["a.png".into()])),
        false,
    )
    .unwrap();
    assert_eq!(
        (report.state, report.message.as_str()),
        (Test::Passed, "All scenarios passed.")
    );
    assert_eq!(report.files, ["a.png"]);
    assert!(finished(&request(), Ok((None, String::new(), Vec::new())), false).is_none());
    assert!(
        finished(&request(), Err("ssh".into()), false).is_none(),
        "a failed look is tried again"
    );
    assert_eq!(
        finished(&request(), Err("ssh".into()), true).map(|report| report.state),
        Some(Test::Expired)
    );
    let expired = finished(&request(), Ok((None, "half".into(), vec!["b.png".into()])), true).unwrap();
    assert_eq!(
        (expired.state, expired.files.len()),
        (Test::Expired, 1),
        "evidence of an expired test is kept"
    );
}

#[test]
fn an_approved_test_cloud_starts_only_after_a_saved_ledger_names_it() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = egui::Context::default();
    let mut ledger = native::Ledger::default();
    ledger.take(vec![request()]);
    assert!(ledger.start_next().is_some());
    let approved = native::Approved {
        checkout: "/repo".into(),
        revision: "a".repeat(40),
        profile_name: "cpu".into(),
        profile: serde_json::from_value(
            serde_json::json!({"provider": "hetzner", "image": "web", "cpu": 4, "memory_gb": 8}),
        )
        .unwrap(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok((approved, "test-cloud".to_owned()))).unwrap();
    let mut native = Native {
        approving: Some(rx),
        ..Native::default()
    };
    approval(&mut ledger, &mut native);
    assert_eq!(
        ledger.current.as_ref().and_then(|current| current.cloud.as_deref()),
        Some("test-cloud"),
        "the ledger names the cloud before it exists"
    );
    assert!(native.dirty && native.launching.is_some());
    let clouds = app.cloud_prototype.groups.0.len();
    app.native_launch(&ctx, "upkeep", &mut ledger, &mut native, false);
    assert_eq!(
        app.cloud_prototype.groups.0.len(),
        clouds,
        "an unsaved ledger starts no cloud"
    );
    assert!(ledger.current.is_none() && native.launching.is_none());
    assert_eq!(ledger.reports[0].state, Test::Refused);
}

#[test]
fn only_a_board_with_the_upkeep_or_its_test_cloud_follows_the_tests() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let mut ledger = native::Ledger::default();
    ledger.take(vec![request()]);
    assert!(ledger.start_next().is_some());
    ledger.current.as_mut().unwrap().cloud = Some("test-cloud".into());
    assert!(
        !app.native_on_board("upkeep", &ledger),
        "the Upkeep of another session waits"
    );
    let workspace = app.board.create_workspace("Upkeep");
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        101,
        "Test web #7".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        "/repo".into(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        id: "test-cloud".into(),
        revision: "a".repeat(40),
        profile_name: "cpu".into(),
        profile: serde_json::from_value(
            serde_json::json!({"provider": "hetzner", "image": "web", "cpu": 4, "memory_gb": 8}),
        )
        .unwrap(),
        placement: Placement::default(),
        deployment_started: true,
    });
    app.cloud_prototype.groups.0.push(group);
    assert!(
        app.native_on_board("upkeep", &ledger),
        "a test whose Upkeep was closed still ends"
    );
}

#[test]
fn the_task_counts_as_given_only_when_the_enter_reaches_the_agent_panel() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let mut ledger = native::Ledger::default();
    ledger.take(vec![request()]);
    assert!(ledger.start_next().is_some());
    let now = Instant::now();
    let mut native = Native {
        // A panel that a session switch or a close took away.
        agent: Some((horizon_core::PanelId(u64::MAX), now)),
        submit_at: Some(now),
        ..Native::default()
    };
    assert!(!app.submit_task("upkeep", &mut ledger, &mut native, now));
    assert!(!ledger.current.as_ref().unwrap().prompted, "the task is given again");
    assert!(native.submit_at.is_none());
    assert_eq!(ledger.reports[0].state, Test::Starting);
}

#[test]
fn a_test_cloud_whose_close_failed_holds_back_the_next_test() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("Upkeep");
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        101,
        "Test web #7".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        "/repo".into(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        id: "test-cloud".into(),
        revision: "a".repeat(40),
        profile_name: "cpu".into(),
        profile: serde_json::from_value(
            serde_json::json!({"provider": "hetzner", "image": "web", "cpu": 4, "memory_gb": 8}),
        )
        .unwrap(),
        placement: Placement::default(),
        deployment_started: true,
    });
    app.cloud_prototype.groups.0.push(group);
    let mut ledger = native::Ledger {
        closing: vec!["test-cloud".into()],
        ..native::Ledger::default()
    };
    // Its close was tried in this run and stopped short, so nothing deletes it now.
    let mut native = Native::default();
    native.close_tried.insert("test-cloud".into());
    let ctx = egui::Context::default();
    assert!(app.native_closing(&ctx, &mut ledger, &mut native));
    app.cloud_prototype.groups.0.clear();
    assert!(!app.native_closing(&ctx, &mut ledger, &mut native), "its card is gone");
    assert!(ledger.closing.is_empty());
}

#[test]
fn a_failed_save_of_the_ledger_is_tried_again_after_a_while() {
    let ctx = egui::Context::default();
    let ledger = native::Ledger::default();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(false).unwrap();
    let mut native = Native {
        saving: Some(rx),
        stored: true,
        ..Native::default()
    };
    let now = Instant::now();
    assert_eq!(
        save(&ctx, "upkeep", &ledger, &mut native, now),
        Some(false),
        "a launch that this ledger names is refused"
    );
    assert!(native.saving.is_none() && !native.undelivered);
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(false).unwrap();
    native.saving = Some(rx);
    // A change after the failed save started: its launch waits for the next save.
    native.dirty = true;
    assert_eq!(save(&ctx, "upkeep", &ledger, &mut native, now), None);
    assert_eq!(save(&ctx, "upkeep", &ledger, &mut native, now + RETRY / 2), None);
    assert!(native.saving.is_none() && native.dirty, "the next save waits");
    assert_eq!(native.save_after, Some(now + RETRY));
}
