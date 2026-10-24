//! Stop, resume, check and delete through this crate's records against a loopback
//! ARM endpoint. The request sequences themselves are tested in
//! `horizon_cloud::azure::lifecycle` against recorded ARM answers.
use super::super::{check, client, delete, resume, retained, save, stop};
use super::*;
use crate::cloud_runtime::{
    Stage, deployment::hetzner::tests::provider::serve, lifecycle::ReconciledDeployment, mutation,
};
use horizon_cloud::{WorkerStatus, azure::provision::Journal, runpod::recovery::Outcome};

fn vm_in(power: &str) -> String {
    let mut body: serde_json::Value = serde_json::from_str(&vm()).unwrap();
    body["properties"]["instanceView"]["statuses"] = json!([{ "code": power }]);
    body.to_string()
}

/// One read of the bound worker: its VM, then the deployment's address.
fn read(power: &str) -> [(u16, String); 2] {
    [(200, vm_in(power)), (200, deployment("Succeeded"))]
}

fn script(parts: &[&[(u16, String)]]) -> Vec<(u16, String)> {
    parts.iter().flat_map(|part| part.iter().cloned()).collect()
}

fn bound() -> CreateState {
    CreateState::Bound {
        worker_id: INSTANCE.into(),
    }
}

/// A ready cloud with its bound VM and group.
fn ready(root: &std::path::Path) -> (Store, Deployment) {
    let (store, mut state) = record(root, &spec(), &bound());
    state.stage = Stage::Ready;
    store.save(&state).unwrap();
    save(
        &Journal {
            region: Some("northeurope".into()),
            group: CreateState::Bound { worker_id: group() },
            ..Journal::default()
        },
        root,
    )
    .unwrap();
    (store, state)
}

fn client_at(address: std::net::SocketAddr) -> horizon_cloud::azure::Azure {
    compute(address).client
}

/// The provider mutations each request line names, in order.
fn writes(requests: &[String]) -> Vec<String> {
    lines(requests)
        .into_iter()
        .filter(|line| !line.starts_with("GET "))
        .collect()
}

#[cfg(unix)]
#[test]
fn a_stop_deallocates_and_is_confirmed_as_a_kept_worker() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    let (address, requests, task) = serve(script(&[
        &read("PowerState/running"),
        &read("PowerState/running"),
        &[(200, String::new())],
        &read("PowerState/deallocated"),
        &read("PowerState/deallocated"),
        // The check afterwards.
        &read("PowerState/deallocated"),
    ]));
    let client = client_at(address);
    let seen = std::cell::RefCell::new(Vec::new());
    stop(&client, &store, &mut state, &Cancellation::default(), &|phase| {
        seen.borrow_mut().push(phase);
        if phase == Mutation::Pending {
            // The stop is recorded before the request is announced.
            let saved = store.load().unwrap().unwrap();
            assert!(saved.stop_requested && saved.stage == Stage::Stopping);
        }
        Ok(Mutation::Settled)
    })
    .unwrap();
    assert_eq!(*seen.borrow(), [Mutation::Pending, Mutation::Settled]);
    let saved = store.load().unwrap().unwrap();
    assert_eq!((saved.stage, saved.stop_requested), (Stage::Stopped, true));
    assert_eq!(saved.operation, bound(), "the same VM is kept");
    assert_eq!(saved.worker.as_ref().unwrap().status(), WorkerStatus::Stopped);
    let mut state = saved;
    let report = check(&client, &store, &mut state, None, &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert_eq!(
        report.outcome,
        Outcome::Inactive {
            worker_id: INSTANCE.into()
        }
    );
    assert!(ReconciledDeployment { state, report }.confirmed_stopped());
    assert_eq!(
        writes(&requests.lock().unwrap()),
        ["POST horizon-ws-azure-fixture/providers/Microsoft.Compute/virtualMachines/worker/deallocate"]
    );
    assert!(retained(root.path()).unwrap(), "the disks and the address stay");
}

#[cfg(unix)]
#[test]
fn an_uncertain_stop_stays_recorded_and_a_retry_settles_it_without_a_second_request() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    let (address, requests, task) = serve(script(&[
        &read("PowerState/running"),
        &read("PowerState/running"),
        &[(503, String::new())],
    ]));
    let seen = std::cell::RefCell::new(Vec::new());
    let observe = |phase| {
        seen.borrow_mut().push(phase);
        Ok(Mutation::Settled)
    };
    let error = stop(
        &client_at(address),
        &store,
        &mut state,
        &Cancellation::default(),
        &observe,
    )
    .unwrap_err();
    task.join().unwrap();
    assert!(error.to_string().contains("transport"), "{error}");
    let saved = store.load().unwrap().unwrap();
    assert_eq!((saved.stage, saved.stop_requested), (Stage::Stopping, true));
    assert_eq!(*seen.borrow(), [Mutation::Pending], "the request stays pending");
    assert_eq!(writes(&requests.lock().unwrap()).len(), 1);
    // The retry finds the VM deallocated and sends nothing.
    let mut state = saved;
    let (address, requests, task) = serve(script(&[
        &read("PowerState/deallocated"),
        &read("PowerState/deallocated"),
    ]));
    stop(
        &client_at(address),
        &store,
        &mut state,
        &Cancellation::default(),
        &observe,
    )
    .unwrap();
    task.join().unwrap();
    assert!(writes(&requests.lock().unwrap()).is_empty());
    assert_eq!(store.load().unwrap().unwrap().stage, Stage::Stopped);
    assert_eq!(seen.borrow().last(), Some(&Mutation::Settled));
}

#[cfg(unix)]
#[test]
fn a_refused_stop_rolls_the_stop_back() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    let (address, _, task) = serve(script(&[
        &read("PowerState/running"),
        &read("PowerState/running"),
        &[(
            403,
            json!({"error": {"code": "AuthorizationFailed", "message": "no"}}).to_string(),
        )],
    ]));
    let error = stop(
        &client_at(address),
        &store,
        &mut state,
        &Cancellation::default(),
        &|_| Ok(Mutation::Settled),
    )
    .unwrap_err();
    task.join().unwrap();
    assert!(error.to_string().to_lowercase().contains("credential"), "{error}");
    let saved = store.load().unwrap().unwrap();
    assert_eq!((saved.stage, saved.stop_requested), (Stage::Ready, false));
}

#[cfg(unix)]
#[test]
fn a_resume_starts_the_same_vm_and_waits_for_readiness() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    state.stage = Stage::Stopped;
    state.stop_requested = true;
    store.save(&state).unwrap();
    let (address, requests, task) = serve(script(&[
        &read("PowerState/deallocated"),
        &read("PowerState/deallocated"),
        &[(200, String::new())],
        &read("PowerState/running"),
    ]));
    let seen = std::cell::RefCell::new(Vec::new());
    resume(
        &client_at(address),
        &store,
        &mut state,
        &Cancellation::default(),
        &|phase| {
            seen.borrow_mut().push(phase);
            Ok(Mutation::Settled)
        },
    )
    .unwrap();
    task.join().unwrap();
    assert_eq!(*seen.borrow(), [Mutation::Pending, Mutation::Settled]);
    let saved = store.load().unwrap().unwrap();
    assert_eq!((saved.stage, saved.stop_requested), (Stage::Readiness, false));
    assert_eq!(saved.operation, bound(), "no new VM");
    assert!(saved.timeline.is_some());
    assert_eq!(
        writes(&requests.lock().unwrap()),
        ["POST horizon-ws-azure-fixture/providers/Microsoft.Compute/virtualMachines/worker/start"]
    );
}

#[test]
fn a_cloud_being_deleted_or_without_a_bound_vm_is_neither_stopped_nor_resumed() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    let mut journal = super::super::load(root.path()).unwrap();
    journal.deleting = true;
    save(&journal, root.path()).unwrap();
    let client = client_at(closed());
    let error = resume(&client, &store, &mut state, &Cancellation::default(), mutation::IGNORE).unwrap_err();
    assert!(error.to_string().contains("being deleted"), "{error}");
    state.operation = CreateState::Requested;
    let error = stop(&client, &store, &mut state, &Cancellation::default(), mutation::IGNORE).unwrap_err();
    assert_eq!(error.to_string(), "Reconcile a bound worker before stopping it");
    let error = resume(&client, &store, &mut state, &Cancellation::default(), mutation::IGNORE).unwrap_err();
    assert_eq!(error.to_string(), "No existing worker to resume");
    assert_eq!(store.load().unwrap().unwrap().operation, bound(), "nothing is saved");
}

#[cfg(unix)]
#[test]
fn a_check_binds_a_found_vm_and_reports_a_lost_one() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    state.operation = CreateState::Requested;
    store.save(&state).unwrap();
    let (address, _, task) = serve(script(&[
        &[(200, created_group()), (200, vm())],
        &read("PowerState/running"),
        // The bound VM is gone.
        &[(404, json!({"error": {"code": "ResourceNotFound"}}).to_string())],
    ]));
    let client = client_at(address);
    let report = check(&client, &store, &mut state, None, &Cancellation::default()).unwrap();
    assert_eq!(
        report.outcome,
        Outcome::Found {
            worker_id: INSTANCE.into()
        }
    );
    assert_eq!(store.load().unwrap().unwrap().operation, bound());
    let report = check(&client, &store, &mut state, None, &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert_eq!(
        report.outcome,
        Outcome::Missing {
            worker_id: INSTANCE.into()
        }
    );
    assert!(report.worker.is_none());
    let error = check(&client, &store, &mut state, Some("another"), &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("identity"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_delete_proves_the_group_absent_before_the_vm_fence_ends() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    let (address, requests, task) = serve(vec![
        (200, created_group()),
        (202, String::new()),
        (404, json!({"error": {"code": "ResourceGroupNotFound"}}).to_string()),
        (200, json!({"value": []}).to_string()),
    ]);
    delete(&client_at(address), &store, &mut state, &Cancellation::default()).unwrap();
    task.join().unwrap();
    assert_eq!(
        lines(&requests.lock().unwrap()),
        [
            "GET horizon-ws-azure-fixture",
            "DELETE horizon-ws-azure-fixture",
            "GET horizon-ws-azure-fixture",
            &format!("GET /subscriptions/{SUB}/resources"),
        ]
    );
    let journal = super::super::load(root.path()).unwrap();
    assert!(journal.proves_absent("azure-fixture"));
    assert!(!retained(root.path()).unwrap());
    assert_eq!(
        store.load().unwrap().unwrap().operation,
        CreateState::Terminated {
            worker_id: INSTANCE.into()
        }
    );
}

#[test]
fn a_failed_delete_keeps_the_cloud_retained_and_resumable() {
    let root = tempfile::tempdir().unwrap();
    let (store, mut state) = ready(root.path());
    let error = delete(&client_at(closed()), &store, &mut state, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("transport"), "{error}");
    let journal = super::super::load(root.path()).unwrap();
    assert!(journal.deleting && !journal.proves_absent("azure-fixture"));
    assert!(retained(root.path()).unwrap());
    assert_eq!(store.load().unwrap().unwrap().operation, bound());
}

#[test]
fn the_settings_must_name_the_subscription_of_the_cloud() {
    let root = tempfile::tempdir().unwrap();
    let (store, _) = ready(root.path());
    let token = root.path().join("arm-token");
    std::fs::write(&token, "synthetic-arm-token").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let settings = |subscription: &str| -> crate::cloud_runtime::settings::Settings {
        serde_json::from_value(json!({
            "runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused",
            "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],"data_centers":[],
            "azure": {"token_file": token, "subscription_id": subscription,
                "vm_sizes": ["Standard_D4s_v3"], "regions": ["northeurope"]}
        }))
        .unwrap()
    };
    assert!(client(&settings(SUB), &store).is_ok());
    let error = client(&settings("00000000-0000-4000-8000-000000000002"), &store)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("another subscription"), "{error}");
    // A group whose request answer was lost is checked against the subscription the
    // request went to: a 404 in another one would prove nothing.
    let other = "00000000-0000-4000-8000-000000000002";
    let requested = |subscription: Option<&str>| {
        save(
            &Journal {
                region: Some("northeurope".into()),
                subscription: subscription.map(str::to_owned),
                group: CreateState::Requested,
                ..Journal::default()
            },
            root.path(),
        )
        .unwrap();
    };
    requested(Some(SUB));
    assert!(client(&settings(SUB), &store).is_ok());
    let error = client(&settings(other), &store).err().unwrap().to_string();
    assert!(error.contains("another subscription"), "{error}");
    // An older journal without the subscription is unresolved, never permission to delete.
    requested(None);
    for subscription in [SUB, other] {
        let error = client(&settings(subscription), &store).err().unwrap().to_string();
        assert!(error.contains("did not record which Azure subscription"), "{error}");
    }
}
