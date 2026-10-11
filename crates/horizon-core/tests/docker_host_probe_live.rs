//! Explicit opt-in read-only probe of an authorized Tailscale SSH host.
#![cfg(all(unix, feature = "cloud-workspaces"))]
use horizon_core::cloud_runtime::{Cancellation, docker_host};
#[test]
#[ignore = "requires an explicitly authorized read-only Tailscale SSH host"]
fn read_only_tailscale_host_probe() {
    let binding_path = std::env::var("HORIZON_DOCKER_READ_ONLY_HOST").expect("explicitly authorized host binding");
    let host: docker_host::Binding = serde_json::from_slice(&std::fs::read(binding_path).unwrap()).unwrap();
    assert_eq!(
        host.ssh.as_ref().unwrap().authentication,
        docker_host::SshAuthentication::Tailscale
    );
    let report = docker_host::probe(&host, None, &Cancellation::default()).unwrap();
    assert!(!report.checks.is_empty(), "missing prerequisites must produce evidence");
    println!("READ_ONLY_PROBE {}", serde_json::to_string(&report).unwrap());
}
