//! Explicit opt-in lifecycle of two clouds on this computer's Docker engine with the
//! public worker base image. It needs a running Docker engine and network access to
//! pull the image, and removes everything it created, also when it fails.
#![cfg(all(unix, feature = "cloud-workspaces"))]
use horizon_core::{
    cloud_panel::CloudConfig,
    cloud_runtime::{Cancellation, deployment, lifecycle, repository, settings::Settings},
};
use std::{path::Path, process::Command};

/// The public base image this Horizon version pins, with the capabilities it is built with.
fn config() -> String {
    format!(
        "version: 1
default: local
profiles:
  local:
    provider: docker-host
    image: {}
    cpu: 2
    memory_gb: 2
    gpu: false
    storage:
      container_gb: 20
      volume_gb: 20
    bootstrap:
      readiness_seconds: 600
    capabilities:
      agents: [claude, codex]
      browsers: [chromium]
      desktop: true
",
        repository::launch::quick_start::IMAGE
    )
}

fn run(program: &str, args: &[&str], directory: &Path) -> String {
    let output = Command::new(program)
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Removes the test clouds' containers and volumes, whatever state the test left.
struct Cleanup(Vec<String>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for id in &self.0 {
            let filter = format!("label=io.horizon.cloud={id}");
            let listed = Command::new("docker").args(["ps", "-aq", "--filter", &filter]).output();
            for container in listed
                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
                .unwrap_or_default()
                .lines()
            {
                let _ = Command::new("docker").args(["rm", "--force", container]).output();
            }
            // Only the test cloud's own labelled volume, never a foreign one of that name.
            let name = format!("horizon-cloud-{id}-workspace");
            let volumes = Command::new("docker")
                .args(["volume", "ls", "-q", "--filter", &filter])
                .output();
            if volumes.is_ok_and(|output| String::from_utf8_lossy(&output.stdout).lines().any(|line| line == name)) {
                let _ = Command::new("docker").args(["volume", "rm", &name]).output();
            }
        }
    }
}

fn labelled(kind: &str, id: &str) -> usize {
    let filter = format!("label=io.horizon.cloud={id}");
    let args: &[&str] = if kind == "volume" {
        &["volume", "ls", "-q", "--filter", &filter]
    } else {
        &["ps", "-aq", "--filter", &filter]
    };
    run("docker", args, Path::new("/")).lines().count()
}

#[test]
#[ignore = "runs two clouds on this computer's Docker engine and pulls the public worker image"]
fn two_clouds_run_stop_resume_and_delete_on_this_computers_docker() {
    let root = tempfile::tempdir().unwrap();
    let suffix = std::process::id();
    let ids = [format!("dockerlive{suffix}a"), format!("dockerlive{suffix}b")];
    let _cleanup = Cleanup(ids.to_vec());
    let repo = root.path().join("repo");
    std::fs::create_dir_all(repo.join(".horizon")).unwrap();
    std::fs::write(repo.join(".horizon/cloud.yml"), config()).unwrap();
    run("git", &["init", "-q"], &repo);
    run("git", &["add", "-A"], &repo);
    run(
        "git",
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
        &repo,
    );
    let key = root.path().join("worker_ed25519");
    run(
        "ssh-keygen",
        &["-q", "-t", "ed25519", "-N", "", "-f", key.to_str().unwrap()],
        root.path(),
    );
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file": root.path().join("absent"), "ssh_identity_file": key,
        "docker_config": root.path().join("docker-config"), "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [],
        "docker_hosts": [{"name": "this-pc", "access": "this-pc"}]
    }))
    .unwrap();
    let profile = CloudConfig::parse(&config()).unwrap().profiles["local"].clone();
    let revision = repository::resolve(&repo, "HEAD").unwrap();
    let cancel = Cancellation::default();
    let mut ports = Vec::new();
    for id in &ids {
        let request = deployment::Request::new(
            id.clone(),
            repo.clone(),
            revision.clone(),
            profile.clone(),
            root.path().join(id),
            settings.clone(),
        );
        let ready = deployment::deploy(&request, &cancel, &|_| {}).unwrap();
        ports.push(ready.worker.unwrap().ssh_endpoint().unwrap().port());
    }
    assert_ne!(ports[0], ports[1], "each cloud has its own SSH port");
    let first = root.path().join(&ids[0]);
    lifecycle::stop(&first, &settings, &cancel).unwrap();
    assert!(
        lifecycle::reconcile(&first, &settings, None, &cancel)
            .unwrap()
            .confirmed_stopped()
    );
    lifecycle::resume(&first, &settings, &cancel).unwrap();
    let resumed = deployment::reconnect(&first, settings.clone(), &cancel, &|_| {}).unwrap();
    assert_eq!(
        resumed.worker.unwrap().ssh_endpoint().unwrap().port(),
        ports[0],
        "the port stays pinned"
    );
    for id in &ids {
        deployment::terminate(&root.path().join(id), &settings, &cancel, &|_| {}).unwrap();
        assert_eq!((labelled("container", id), labelled("volume", id)), (0, 0));
    }
}
