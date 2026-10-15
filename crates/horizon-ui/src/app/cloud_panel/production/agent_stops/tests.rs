use super::*;
use std::cell::RefCell;

fn stop() -> AgentStop {
    serde_json::from_value(serde_json::json!({
        "at": 1_792_017_958_386_u64, "reason": "PR 12 merged", "session": "agent-a", "agent": "claude"
    }))
    .unwrap()
}

/// What `run` sends for the reads `answers`; the card stops listening once they are
/// all read.
fn sent(answers: Vec<cloud_runtime::Result<Requested>>) -> Vec<String> {
    let count = answers.len();
    let answers = RefCell::new(answers.into_iter());
    let found = RefCell::new(Vec::new());
    let checks = RefCell::new(0);
    let send = |event: Found| {
        match event {
            Found::Watching => {
                *checks.borrow_mut() += 1;
                return *checks.borrow() <= count;
            }
            Found::Stop(stop) => found.borrow_mut().push(format!("stop: {}", stop.reason)),
            Found::Log(line) => found.borrow_mut().push(line),
        }
        true
    };
    run(Duration::ZERO, || answers.borrow_mut().next().unwrap(), &send);
    found.into_inner()
}

#[test]
fn a_taken_request_asks_the_card_to_stop_the_cloud_for_the_agent() {
    let found = sent(vec![
        Ok(Requested::Nothing),
        Ok(Requested::Waiting("An agent session produced output".into())),
        Ok(Requested::Stop(stop())),
        // Until the stop starts, the worker gives the same request again.
        Ok(Requested::Stop(stop())),
    ]);
    assert_eq!(found, ["stop: PR 12 merged", "stop: PR 12 merged"]);
}

#[test]
fn a_dropped_request_is_logged_and_a_failed_read_once_until_it_changes() {
    let failed = || Err(Error::Invalid("The worker's stop request is malformed"));
    let found = sent(vec![
        Ok(Requested::Refused(
            "The stop request lapsed before the worker stopped.".into(),
        )),
        failed(),
        failed(),
        // Another operation holds the cloud: no failure.
        Err(Error::Busy),
        failed(),
        Ok(Requested::Nothing),
        failed(),
    ]);
    assert_eq!(
        found,
        [
            "An agent's stop request was dropped: The stop request lapsed before the worker stopped.",
            "Reading the agent's stop request failed: The worker's stop request is malformed",
            "Reading the agent's stop request failed: The worker's stop request is malformed",
        ]
    );
}

#[test]
fn the_reading_ends_once_the_card_stops_listening() {
    let reads = RefCell::new(0);
    run(
        Duration::ZERO,
        || {
            *reads.borrow_mut() += 1;
            Ok(Requested::Nothing)
        },
        &|found| !matches!(found, Found::Watching),
    );
    assert_eq!(*reads.borrow(), 0, "nothing is read for a card that is gone");
}

// Cloud records need a Unix host's durable directory updates.
#[cfg(unix)]
#[test]
fn a_stop_request_starts_a_stop_for_that_agent() {
    use crate::app::test_support::test_app;
    use horizon_core::cloud_panel::{CloudGroup, CloudLaunch};
    use std::time::{Duration, Instant};
    let (temp, mut app) = test_app();
    let root = temp.path().join("clouds");
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    // A cloud that is no longer one whose agents Horizon stops, so the stop for the
    // agent refuses before any provider or worker request.
    let state: cloud_runtime::state::Deployment = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": temp.path(), "revision": "a".repeat(40),
        "profile": {"provider": "runpod", "image": "registry.example/worker", "cpu": 2, "memory_gb": 4},
        "stage": "Ready", "operation": {"state": "bound", "worker_id": "42"}, "spec": null, "worker": null,
        "sessions": []
    }))
    .unwrap();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: state.profile.clone(),
        placement: horizon_core::cloud_panel::Placement::default(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(root.clone());
    std::fs::create_dir_all(&root).unwrap();
    let settings = serde_json::json!({
        "runpod_key_file": root.join("missing-key"), "ssh_identity_file": root.join("ssh"),
        "docker_config": root.join("docker"), "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": []
    });
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    cloud_runtime::state::Store::lock(&root.join("fixture"))
        .unwrap()
        .save(&state)
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.stage = Some(cloud_runtime::Stage::Ready);
    runtime.state = Some(state);
    runtime.agent_stops = Some(receiver);
    sender
        .send(Found::Log("Reading the agent's stop request failed: once".into()))
        .unwrap();
    sender.send(Found::Stop(stop())).unwrap();
    app.prepare_production_clouds(&ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.operation, Some(Action::Stop));
    assert!(runtime.agent_stop.is_none(), "the stop took the request");
    assert!(
        runtime
            .logs
            .iter()
            .any(|line| line.text == "claude asked to stop this cloud: PR 12 merged"),
        "the card names the agent"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.cloud_prototype.production.runtimes[&1].error.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        app.prepare_production_clouds(&ctx);
    }
    assert_eq!(
        app.cloud_prototype.production.runtimes[&1].error.as_deref(),
        Some("The cloud is no longer ready on the worker that asked, so Horizon did not stop it for the agent")
    );
}
