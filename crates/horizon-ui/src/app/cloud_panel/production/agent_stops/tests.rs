use super::*;
use std::cell::RefCell;

fn stop() -> AgentStop {
    serde_json::from_value(serde_json::json!({
        "at": 1_792_017_958_386_u64, "reason": "PR 12 merged", "session": "agent-a", "agent": "claude"
    }))
    .unwrap()
}

fn settings() -> Settings {
    serde_json::from_value(serde_json::json!({
        "runpod_key_file": "/unused", "ssh_identity_file": "/unused", "docker_config": "/unused",
        "registry_pull_auth_id": null, "cpu_flavors": [], "gpu_types": []
    }))
    .unwrap()
}

fn order() -> Order {
    Order {
        stop: stop(),
        settings: settings(),
    }
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
            Found::Stop(order) => found.borrow_mut().push(format!("stop: {}", order.stop.reason)),
            Found::Log(line) => found.borrow_mut().push(line),
        }
        true
    };
    run(
        Duration::ZERO,
        || answers.borrow_mut().next().unwrap().map(|answer| (answer, settings())),
        &send,
    );
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
            Ok((Requested::Nothing, settings()))
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
    // A ready Hetzner cloud; the request names no worker of it, so the stop for the
    // agent refuses before any provider or worker request.
    let state: cloud_runtime::state::Deployment = serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "fixture", "repository": temp.path(), "revision": "a".repeat(40),
        "profile": {"provider": "hetzner", "image": "registry.example/worker", "cpu": 2, "memory_gb": 4},
        "stage": "Ready", "operation": {"state": "bound", "worker_id": "42"}, "spec": null,
        "worker": {"id": "42", "name": "horizon-cloud-fixture", "imageName": "registry.example/worker@sha256:1",
                   "desiredStatus": "RUNNING", "publicIp": "192.0.2.10", "portMappings": {"22": 22}},
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
    sender.send(Found::Stop(Box::new(order()))).unwrap();
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

#[test]
fn a_cloud_with_an_open_dialog_gives_the_request_back_instead_of_stopping() {
    use crate::app::test_support::test_app;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.stage = Some(cloud_runtime::Stage::Ready);
    runtime.confirmation = Confirmation::Delete;
    app.stop_clouds_for_agents(vec![(1, order())], &ctx);
    // Found again later, as a worker that kept it gives it, it is still not carried out.
    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .confirmation = Confirmation::None;
    app.stop_clouds_for_agents(vec![(1, order())], &ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.operation, None, "no Stop starts");
    assert!(runtime.agent_stop.is_none());
    let given_back = runtime
        .logs
        .iter()
        .filter(|line| {
            line.text
                .starts_with("claude asked to stop this cloud while it was not ready")
        })
        .count();
    assert_eq!(given_back, 1, "logged once");
}

#[test]
fn a_request_found_again_while_its_stop_runs_is_left_alone() {
    use crate::app::test_support::test_app;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.stage = Some(cloud_runtime::Stage::Stopping);
    runtime.operation = Some(Action::Stop);
    app.stop_clouds_for_agents(vec![(1, order())], &ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.given_back, None, "not given back");
    assert!(runtime.logs.is_empty());
}

#[test]
fn a_cloud_whose_close_dialog_is_open_gives_the_request_back() {
    use crate::app::test_support::test_app;
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    app.cloud_prototype.production.runtimes.entry(1).or_default().stage = Some(cloud_runtime::Stage::Ready);
    app.request_cloud_close(1);
    app.stop_clouds_for_agents(vec![(1, order())], &ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.operation, None, "no Stop starts");
    assert_eq!(runtime.given_back, Some(stop().at));
}
