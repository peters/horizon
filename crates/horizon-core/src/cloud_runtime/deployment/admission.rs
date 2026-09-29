//! What a deployment would refuse on this machine, found while the person can still fix it.
use super::Request;
use crate::cloud_runtime::{
    Error, providers,
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
/// what the provider needs of the profile, the provider's account and credential, and the SSH key
/// the worker is reached with.
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
    // A missing or unusable account comes first; only with one does a refusal of the profile
    // itself (which no key can fix) stand on its own.
    match providers::compute(&request) {
        Err(error) => note("Provider account", Err(error)),
        Ok(_) => note(
            "Provider profile",
            providers::preflight(&request.cloud_id, profile, settings),
        ),
    }
    note("SSH key", ssh_key(settings));
    found
}

/// The identity the worker is reached with: a private key file and, beside it, the public key
/// deployment sends to the provider, which must be a supported Ed25519 key.
fn ssh_key(settings: &Settings) -> crate::cloud_runtime::Result<()> {
    validate_ssh_identity(&settings.ssh_identity_file)?;
    // Appended to the path itself: a name that is not valid UTF-8 keeps its own bytes.
    let mut public = settings.ssh_identity_file.clone().into_os_string();
    public.push(".pub");
    let key = std::fs::read_to_string(&public)?;
    if !horizon_cloud::valid_public_key(key.trim()) {
        return Err(Error::Invalid(
            "The SSH public key beside the identity is not a supported Ed25519 key; Cloud settings can make a new pair",
        ));
    }
    // The same test that decides whether a profile is ready to launch: the private key must be
    // readable without a passphrase and be the pair of the public key deployment sends.
    if crate::cloud_runtime::repository::launch::ssh_ready(&settings.ssh_identity_file) {
        Ok(())
    } else {
        Err(Error::Invalid(
            "The SSH private key is not the pair of the public key beside it, has a passphrase, or ssh-keygen is missing; Cloud settings can make a new pair",
        ))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn private_file(path: &std::path::Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// A real key pair, as Cloud settings makes one.
    fn keygen(path: &std::path::Path) {
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// A private identity file with its public key beside it.
    fn identity(root: &std::path::Path) {
        let _ = std::fs::remove_file(root.join("ssh"));
        keygen(&root.join("ssh"));
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
        identity(root.path());
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
        identity(root.path());
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

    #[test]
    fn an_identity_without_its_public_key_is_not_usable() {
        let root = tempfile::tempdir().unwrap();
        private_file(&root.path().join("runpod"), "key");
        private_file(&root.path().join("ssh"), "-----BEGIN-----");
        let found = problems(&profile(), &settings(root.path()));
        assert_eq!(
            found.iter().map(|problem| problem.what).collect::<Vec<_>>(),
            ["SSH key"]
        );
        std::fs::write(root.path().join("ssh.pub"), "ssh-rsa AAAA not-ed25519").unwrap();
        let found = problems(&profile(), &settings(root.path()));
        assert!(
            found
                .iter()
                .any(|problem| problem.what == "SSH key" && problem.reason.contains("Ed25519"))
        );
        identity(root.path());
        assert!(problems(&profile(), &settings(root.path())).is_empty());
    }

    #[test]
    fn a_private_key_that_is_not_the_pair_of_its_public_key_is_not_usable() {
        let root = tempfile::tempdir().unwrap();
        private_file(&root.path().join("runpod"), "key");
        identity(root.path());
        assert!(problems(&profile(), &settings(root.path())).is_empty());
        // A valid Ed25519 public key that belongs to another private key.
        let other = root.path().join("other");
        keygen(&other);
        std::fs::copy(root.path().join("other.pub"), root.path().join("ssh.pub")).unwrap();
        let found = problems(&profile(), &settings(root.path()));
        assert!(
            found
                .iter()
                .any(|problem| problem.what == "SSH key" && problem.reason.contains("not the pair")),
            "{found:?}"
        );
        // A private file that is not a key at all, beside a well-formed public key.
        private_file(&root.path().join("ssh"), "-----BEGIN-----");
        let found = problems(&profile(), &settings(root.path()));
        assert!(found.iter().any(|problem| problem.what == "SSH key"), "{found:?}");
    }

    #[cfg(unix)]
    #[test]
    fn an_identity_whose_name_is_not_utf8_finds_its_public_key() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        private_file(&root.path().join("runpod"), "key");
        let name = std::ffi::OsString::from_vec(b"id-\xff".to_vec());
        let path = root.path().join(name);
        keygen(&path);
        let mut settings = settings(root.path());
        settings.ssh_identity_file = path;
        assert!(problems(&profile(), &settings).is_empty());
    }
}
