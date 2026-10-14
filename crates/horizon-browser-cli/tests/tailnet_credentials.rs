//! Real CLI route regression with a synthetic host queue; no native-owner proof.
use horizon_browser_control::manifest::provider_usage::{CredentialOperation, UsageQueue};
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
        .env("HORIZON_BROWSER_ACTOR", "horizon:synthetic-credential-agent")
        .env("HORIZON_BROWSER_HOST_INSTANCE", "00000000-0000-0000-0000-000000000001")
        .env("RUST_LOG", "off")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn cli_owner_request_reaches_the_shared_queue_and_returns_only_pending_metadata() {
    let root = tempfile::tempdir().unwrap();
    let plan = root.path().join("plan.json");
    let request_id = "00000000-0000-0000-0000-000000000002";
    let arguments = json!({"operation":"remove","tailnet_id":"synthetic-saved", "request_id":request_id});
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({"version":1,"steps":[{
            "id":"setup","tool":"tailnet_api_credential","arguments":arguments
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
            assert_eq!(request.actor, "horizon:synthetic-credential-agent");
            let input = request.tailnet_credential.as_ref().unwrap();
            assert_eq!(input.operation, CredentialOperation::Remove);
            assert_eq!(input.tailnet_id, "synthetic-saved");
            assert_eq!(input.request_id, request_id);
            let mut result = request.result(Vec::new(), None);
            result.tailnet_credential = Some(json!({"state":"pending_owner","request_id":request_id}));
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
    assert_eq!(report["steps"][0]["result"]["state"], "pending_owner");
    assert_eq!(report["steps"][0]["result"]["request_id"], request_id);
    assert!(
        !root.path().join(".horizon/cloud").exists(),
        "Metadata request cannot create a credential store"
    );
}

#[test]
fn cli_refuses_secret_arguments_before_queuing_an_owner_request() {
    let root = tempfile::tempdir().unwrap();
    let plan = root.path().join("plan.json");
    std::fs::write(
        &plan,
        serde_json::to_vec(&json!({"version":1,"steps":[{
            "id":"setup","tool":"tailnet_api_credential","arguments":{
                "operation":"remove","tailnet_id":"synthetic-saved",
                "request_id":"00000000-0000-0000-0000-000000000002","token":"synthetic-forbidden"
            }
        }]}))
        .unwrap(),
    )
    .unwrap();
    let output = command(root.path(), &plan).output().unwrap();
    assert!(!output.status.success());
    let queue = UsageQueue::new(root.path().join(".horizon"));
    assert!(queue.claim("00000000-0000-0000-0000-000000000001").unwrap().is_empty());
    assert!(!root.path().join(".horizon/cloud").exists());
}

#[test]
fn cli_refuses_entry_operations_without_native_owner_entry() {
    let root = tempfile::tempdir().unwrap();
    let plan = root.path().join("plan.json");
    for operation in ["set_personal_token", "set_oauth"] {
        std::fs::write(&plan, serde_json::to_vec(&json!({"version":1,"steps":[{"id":"remove","tool":"tailnet_api_credential","arguments":{"operation":operation,"tailnet_id":"synthetic-saved","request_id":"00000000-0000-0000-0000-000000000002"}}]})).unwrap()).unwrap();
        assert!(!command(root.path(), &plan).output().unwrap().status.success());
        let queue = UsageQueue::new(root.path().join(".horizon"));
        assert!(queue.claim("00000000-0000-0000-0000-000000000001").unwrap().is_empty());
        assert!(!root.path().join(".horizon/cloud").exists());
    }
}
