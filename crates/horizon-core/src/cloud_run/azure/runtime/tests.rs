#[cfg(target_os = "linux")]
mod shell;

use super::*;
use crate::cloud_run::azure::{
    AzureDeploymentPlan,
    tests::{ed25519_key, profile, target},
};
use crate::cloud_run::{CloudJobId, CloudWorkflowId, interactive_worker::InteractiveWorkerRequest};

fn cloud_init(runtime: AzureContainerRuntime) -> String {
    let mut profile = profile();
    profile.container_runtime = runtime;
    let request = InteractiveWorkerRequest {
        workflow_id: CloudWorkflowId::new(),
        job_id: CloudJobId::new(),
        target: target(),
        ssh_public_key: ed25519_key(7, ""),
    };
    let plan = AzureDeploymentPlan::new(&profile, &request).expect("plan");
    String::from_utf8(
        STANDARD
            .decode(plan.parameters["customData"]["value"].as_str().expect("data"))
            .expect("base64"),
    )
    .expect("utf8")
}

#[test]
fn missing_operator_selection_preserves_default_and_unknown_selection_is_refused() {
    let mut value = serde_json::to_value(profile()).expect("profile");
    value.as_object_mut().expect("object").remove("container_runtime");
    let decoded: crate::cloud_run::azure::AzureProfile = serde_json::from_value(value.clone()).expect("legacy profile");
    assert_eq!(decoded.container_runtime, AzureContainerRuntime::Default);
    value["container_runtime"] = serde_json::json!("unconfined");
    assert!(serde_json::from_value::<crate::cloud_run::azure::AzureProfile>(value).is_err());
    let selected = AzureContainerRuntime::WorkspaceSandboxV1;
    assert_eq!(serde_json::to_value(selected).expect("serialize"), selected.as_str());
}

#[test]
fn default_bootstrap_has_no_runtime_override_or_extra_prerequisite() {
    let script = cloud_init(AzureContainerRuntime::Default);
    assert!(script.contains("packages: [docker.io, iptables-persistent]"));
    assert!(!script.contains("--security-opt"));
    assert!(!script.contains("apparmor.service"));
    assert!(!script.contains("horizon-agent-sandbox-smoke"));
    assert!(script.contains("docker create --name horizon-worker --restart unless-stopped -p"));
}

#[test]
fn explicit_runtime_qualifies_before_worker_creation_without_privileged_fallback() {
    let script = cloud_init(AzureContainerRuntime::WorkspaceSandboxV1);
    let prepare = script
        .find("/usr/local/sbin/horizon-worker-runtime-preflight\n")
        .expect("version guard");
    let qualify = script.find("for uid in 0 1000").expect("both users");
    let create = script.find("docker create --name horizon-worker").expect("create");
    assert!(prepare < qualify && qualify < create);
    assert!(script.contains("Requires=apparmor.service\n      After=apparmor.service"));
    assert!(script.contains("ExecStartPre=/usr/local/sbin/horizon-worker-runtime-preflight"));
    assert!(AzureContainerRuntime::preflight().contains("timeout --kill-after=5s 30 apparmor_parser --replace"));
    assert!(script.contains("--network none --user \"$uid:$uid\""));
    assert!(script.contains("assert p[\"passed\"] is True"));
    for forbidden in [
        "--privileged",
        "--cap-add",
        "seccomp=unconfined",
        "apparmor=unconfined",
        "sysctl -w",
    ] {
        assert!(!script.contains(forbidden), "{forbidden}");
    }
    let worker = script
        .lines()
        .find(|line| line.contains("docker create --name horizon-worker"))
        .expect("worker");
    assert!(worker.contains(AzureContainerRuntime::WorkspaceSandboxV1.flags()));
    assert!(worker.contains("--mount type=bind,src="));
    assert!(worker.contains("--restart unless-stopped"));
}

#[test]
fn scoped_policy_retains_denials_and_adds_only_the_qualified_namespace_operations() {
    let policy: serde_json::Value =
        serde_json::from_str(include_str!("workspace-v1.seccomp.json")).expect("seccomp JSON");
    assert_eq!(policy["defaultAction"], "SCMP_ACT_ERRNO");
    let rules = policy["syscalls"].as_array().expect("rules");
    assert_eq!(
        rules.last().expect("extension"),
        &serde_json::json!({
            "names": ["clone", "unshare", "setns", "mount", "umount2", "pivot_root"],
            "action": "SCMP_ACT_ALLOW"
        })
    );
    assert!(rules.iter().any(|rule| {
        rule["names"]
            .as_array()
            .is_some_and(|names| names.contains(&serde_json::json!("clone3")))
            && rule["action"] == "SCMP_ACT_ERRNO"
            && rule["errnoRet"] == 38
    }));
    let apparmor = include_str!("workspace-v1.apparmor");
    for denial in [
        "deny /sys/kernel/security/**",
        "deny @{PROC}/sysrq-trigger",
        "deny @{PROC}/kcore",
    ] {
        assert!(apparmor.contains(denial));
    }
    assert!(apparmor.contains("profile horizon-workspace-sandbox-v1 "));
    assert!(!apparmor.contains("profile docker-default"));
}

#[cfg(unix)]
#[test]
fn qualification_shell_is_syntactically_valid() {
    use std::io::Write;
    let mut command = std::process::Command::new("bash")
        .arg("-n")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("bash");
    command
        .stdin
        .take()
        .expect("stdin")
        .write_all(
            AzureContainerRuntime::WorkspaceSandboxV1
                .qualify("synthetic.azurecr.io/worker@sha256:aaaa")
                .as_bytes(),
        )
        .expect("script");
    assert!(command.wait().expect("bash status").success());
}
