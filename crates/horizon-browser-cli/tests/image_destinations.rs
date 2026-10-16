//! Actual public CLI-plan routing with a synthetic broker; desktop policy has separate tests.
use horizon_browser_control::manifest::provider_usage::UsageQueue;
use serde_json::{Value, json};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn command(root: &std::path::Path, plan: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_horizon-browser"));
    command
        .arg("run")
        .arg(plan)
        .env_remove("HORIZON")
        .env_remove("HORIZON_HOME")
        .env("HOME", root)
        .env("HORIZON_BROWSER_ROOT", root.join(".horizon"))
        .env("HORIZON_BROWSER_ACTOR", "horizon:synthetic-destination-agent")
        .env("HORIZON_BROWSER_HOST_INSTANCE", "00000000-0000-0000-0000-000000000001")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn receipt(operation: &str) {
    let root = tempfile::tempdir().unwrap();
    let plan = root.path().join("plan.json");
    let request_id = "00000000-0000-0000-0000-000000000002";
    let profile = (!matches!(operation, "status" | "cancel")).then_some("cpu");
    let arguments = json!({"operation_id":request_id,"profile":profile,"action":{"operation":operation}});
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({"version":1,"steps":[{
            "id":"setup","tool":"cloud_image_destination","arguments":arguments
        }]}))
        .unwrap(),
    )
    .unwrap();
    let mut process = OwnedChild(Some(command(root.path(), &plan).spawn().unwrap()));
    let queue = UsageQueue::new(root.path().join(".horizon"));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let requests = queue.claim("00000000-0000-0000-0000-000000000001").unwrap();
        if let Some(request) = requests.first() {
            assert_eq!(request.actor, "horizon:synthetic-destination-agent");
            let input = request.image_destination.as_ref().unwrap();
            assert_eq!(serde_json::to_value(&input.action).unwrap()["operation"], operation);
            assert_eq!(input.profile.as_deref(), profile);
            assert_eq!(input.operation_id, request_id);
            let mut result = request.result(Vec::new(), None);
            result.image_destination = Some(
                json!({"done":false,"operation_id":request_id,"sign_in":(operation=="authenticate").then(||json!({"user_code":"SYNTHETIC-CODE","verification_uri":"https://github.com/login/device","expires_at_unix":123}))}),
            );
            queue.complete(&result).unwrap();
            break;
        }
        assert!(
            process.0.as_mut().unwrap().try_wait().unwrap().is_none(),
            "CLI exited before routing"
        );
        assert!(Instant::now() < deadline, "CLI did not route the owner request");
        std::thread::sleep(Duration::from_millis(10));
    }
    while process.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "CLI did not consume the metadata reply");
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = process.0.take().unwrap().wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["steps"][0]["result"]["done"], false);
    assert_eq!(report["steps"][0]["result"]["operation_id"], request_id);
    if operation == "authenticate" {
        assert_eq!(report["steps"][0]["result"]["sign_in"]["user_code"], "SYNTHETIC-CODE");
    }
    assert!(
        !root.path().join(".horizon/cloud").exists(),
        "Metadata request cannot create a credential store"
    );
}

#[test]
fn public_plan_routes_start_and_receipt_operations_through_the_host_queue() {
    for operation in ["list", "authenticate", "status", "cancel"] {
        receipt(operation);
    }
}

#[test]
fn public_plan_refuses_secret_and_authority_fields_before_queuing() {
    let root = tempfile::tempdir().unwrap();
    let plan = root.path().join("plan.json");
    for field in ["root", "repository", "token", "credential_reference"] {
        let mut arguments = json!({"operation_id":"00000000-0000-0000-0000-000000000002","profile":"cpu","action":{"operation":"list"}});
        arguments[field] = "synthetic-forbidden".into();
        std::fs::write(&plan,serde_json::to_vec(&json!({"version":1,"steps":[{"id":"destination","tool":"cloud_image_destination","arguments":arguments}]})).unwrap()).unwrap();
        let output = command(root.path(), &plan).output().unwrap();
        assert!(!output.status.success());
        assert!(
            UsageQueue::new(root.path().join(".horizon"))
                .claim("00000000-0000-0000-0000-000000000001")
                .unwrap()
                .is_empty()
        );
    }
    assert!(!root.path().join(".horizon/cloud").exists());
}
