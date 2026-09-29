//! What a deployment would refuse on this machine, found while the person can still fix it.
use super::Request;
use crate::cloud_runtime::{
    providers,
    settings::{Settings, validate_ssh_identity},
};
use std::path::PathBuf;

/// One thing deployment would refuse, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Problem {
    pub what: &'static str,
    pub reason: String,
}

/// Runs the checks deployment starts with, against nothing but this machine's settings:
/// the provider's account and credential, and the SSH key the worker is reached with.
#[must_use]
pub fn problems(profile: &horizon_cloud::Profile, settings: &Settings) -> Vec<Problem> {
    let request = Request::new(
        "readiness-check".into(),
        PathBuf::new(),
        String::new(),
        profile.clone(),
        PathBuf::new(),
        settings.clone(),
    );
    let mut found = Vec::new();
    let mut note = |what, result: crate::cloud_runtime::Result<()>| {
        if let Err(error) = result {
            found.push(Problem {
                what,
                reason: error.to_string(),
            });
        }
    };
    note(
        "Provider account",
        providers::preflight(&request.cloud_id, profile, settings)
            .and_then(|()| providers::compute(&request).map(drop)),
    );
    note("SSH key", validate_ssh_identity(&settings.ssh_identity_file));
    found
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn private_file(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn settings(root: &std::path::Path) -> Settings {
        serde_json::from_value(serde_json::json!({
            "runpod_key_file": root.join("runpod"),
            "ssh_identity_file": root.join("ssh"),
            "docker_config": root.join("docker"),
            "registry_pull_auth_id": null,
            "cpu_flavors": ["cpu3c"],
            "gpu_types": [],
        }))
        .unwrap()
    }

    fn profile() -> horizon_cloud::Profile {
        let config = horizon_cloud::CloudConfig::parse(
            "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: ghcr.io/demo-org/dev-image\n    cpu: 4\n    memory_gb: 16\n    gpu: false\n    storage:\n      container_gb: 20\n      volume_gb: 60\n",
        )
        .unwrap();
        config.profiles["cpu"].clone()
    }

    #[test]
    fn a_missing_key_is_reported_before_anything_starts() {
        let root = tempfile::tempdir().unwrap();
        private_file(&root.path().join("runpod"), "key");
        let found = problems(&profile(), &settings(root.path()));
        assert_eq!(
            found.iter().map(|problem| problem.what).collect::<Vec<_>>(),
            ["SSH key"]
        );
        private_file(&root.path().join("ssh"), "-----BEGIN-----");
        assert!(problems(&profile(), &settings(root.path())).is_empty());
        std::fs::remove_file(root.path().join("runpod")).unwrap();
        let found = problems(&profile(), &settings(root.path()));
        assert_eq!(
            found.iter().map(|problem| problem.what).collect::<Vec<_>>(),
            ["Provider account"]
        );
    }

    #[test]
    fn a_hetzner_profile_without_a_binding_is_reported_as_the_provider_account() {
        let root = tempfile::tempdir().unwrap();
        private_file(&root.path().join("runpod"), "key");
        private_file(&root.path().join("ssh"), "-----BEGIN-----");
        let config = horizon_cloud::CloudConfig::parse(
            "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: hetzner\n    image: ghcr.io/demo-org/dev-image\n    cpu: 4\n    memory_gb: 16\n    gpu: false\n    storage:\n      container_gb: 20\n      volume_gb: 60\n",
        )
        .unwrap();
        let found = problems(&config.profiles["cpu"], &settings(root.path()));
        assert_eq!(
            found.iter().map(|problem| problem.what).collect::<Vec<_>>(),
            ["Provider account"]
        );
    }
}
