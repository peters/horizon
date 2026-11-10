//! The machine key store, judged against the real `ssh-keygen`.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use super::*;

/// A test machine without `ssh-keygen` skips the store, with the reason.
fn ssh_keygen_available() -> bool {
    match Command::new("ssh-keygen").arg("-?").output() {
        Ok(_) => true,
        Err(error) => {
            eprintln!("skipping: this test machine has no ssh-keygen: {error}");
            false
        }
    }
}

fn store(temp: &tempfile::TempDir) -> MachineKeys {
    MachineKeys::new(temp.path().join("machine-keys"))
}

fn code(error: &MachineKeyError) -> &'static str {
    error.code()
}

#[test]
fn a_key_pair_is_private_and_a_second_call_returns_the_same_public_key() {
    if !ssh_keygen_available() {
        return;
    }
    let temp = tempfile::tempdir().expect("home");
    let keys = store(&temp);
    let made = keys
        .ensure("first", "build-1", block_keys::MAX_MACHINE_KEYS_PER_WORKSPACE)
        .expect("a new key pair");
    assert_eq!(made.name, "build-1");
    assert!(made.created, "the first call makes the pair");
    assert!(
        made.public_key.starts_with("ssh-ed25519 "),
        "the answer holds one public line: {}",
        made.public_key
    );
    assert!(
        made.public_key.contains("horizon-machine-build-1"),
        "the public key says which machine name it belongs to: {}",
        made.public_key
    );
    assert!(!made.public_key.contains('\n'), "the answer is one line");
    assert!(made.created_at_millis > 0, "the age of the pair is reported");

    let directory = keys.workspace_directory("first");
    let permissions = |path: &Path| std::fs::metadata(path).expect("the file exists").permissions().mode() & 0o777;
    assert_eq!(permissions(&directory), 0o700, "the key directory is private");
    assert_eq!(
        permissions(&directory.join("build-1")),
        0o600,
        "the private key is private"
    );
    assert!(
        !std::fs::read_to_string(directory.join("build-1"))
            .expect("the private key")
            .contains("horizon-machine-build-1"),
        "the private half is not the public line"
    );
    assert_eq!(
        made.private_key_path,
        directory.join("build-1").display().to_string(),
        "the answer names the private half, so the agent can run its own ssh"
    );
    assert_eq!(
        made.known_hosts_path,
        directory.join("build-1.known_hosts").display().to_string(),
        "the answer names the known-hosts file of that key"
    );
    assert!(
        !made.private_key_path.contains("BEGIN"),
        "a path is a path, not key material: {}",
        made.private_key_path
    );

    let again = keys
        .ensure("first", "build-1", block_keys::MAX_MACHINE_KEYS_PER_WORKSPACE)
        .expect("the same key pair");
    assert!(!again.created, "a second call keeps the pair it finds");
    assert_eq!(again.public_key, made.public_key);
    assert_eq!(again.private_key_path, made.private_key_path);
    assert_eq!(again.known_hosts_path, made.known_hosts_path);
    assert_eq!(again.created_at_millis, made.created_at_millis);

    let listed = keys.list("first").expect("the keys of the workspace");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "build-1");
    assert_eq!(listed[0].public_key, made.public_key);
    assert_eq!(listed[0].private_key_path, made.private_key_path);
    assert_eq!(listed[0].known_hosts_path, made.known_hosts_path);
    assert!(!listed[0].created);
}

#[test]
fn a_key_belongs_to_one_workspace_and_delete_removes_its_known_hosts_file() {
    if !ssh_keygen_available() {
        return;
    }
    let temp = tempfile::tempdir().expect("home");
    let keys = store(&temp);
    let made = keys.ensure("first", "build-1", 8).expect("a new key pair");
    let binding = keys.require("first", "build-1").expect("the key is there");
    assert_eq!(binding.private_key(), keys.workspace_directory("first").join("build-1"));
    assert_eq!(
        binding.known_hosts(),
        keys.workspace_directory("first").join("build-1.known_hosts"),
        "the known-hosts file of one key never shares the person's file"
    );
    // ssh makes this file on first contact; a delete removes it too.
    std::fs::write(binding.known_hosts(), "203.0.113.10 ssh-ed25519 AAAA\n").expect("a known host");

    assert!(
        keys.list("second").expect("another workspace").is_empty(),
        "another workspace holds no key of this one"
    );
    assert!(
        keys.require("second", "build-1").is_err(),
        "another workspace cannot use this key"
    );
    let refused = keys.delete("second", "build-1").expect_err("another workspace");
    assert_eq!(code(&refused), block_keys::KEY_UNAVAILABLE);
    assert_eq!(keys.list("first").expect("the keys").len(), 1, "nothing was removed");

    keys.delete("first", "build-1").expect("the key is removed");
    assert!(!binding.private_key().exists(), "the private key is gone");
    assert!(!binding.known_hosts().exists(), "the known-hosts file is gone");
    assert!(keys.list("first").expect("the keys").is_empty());
    assert_eq!(
        code(&keys.require("first", "build-1").expect_err("gone")),
        block_keys::KEY_UNAVAILABLE
    );
    assert_eq!(
        code(&keys.delete("first", "build-1").expect_err("gone")),
        block_keys::KEY_UNAVAILABLE
    );
    assert!(
        made.public_key.starts_with("ssh-ed25519 "),
        "the public key that the machine holds never changed"
    );
}

#[test]
fn the_limit_is_a_parameter_and_a_deleted_key_frees_its_place() {
    if !ssh_keygen_available() {
        return;
    }
    let temp = tempfile::tempdir().expect("home");
    let keys = store(&temp);
    let limit = 2;
    keys.ensure("first", "build-1", limit).expect("the first key");
    keys.ensure("first", "build-2", limit).expect("the second key");
    let refused = keys.ensure("first", "build-3", limit).expect_err("the limit");
    assert_eq!(code(&refused), block_keys::TOO_MANY_KEYS);
    assert!(
        refused.to_string().contains("delete_machine_key"),
        "the refusal names the way out: {refused}"
    );
    assert!(
        !keys
            .ensure("first", "build-1", limit)
            .expect("a key that is there")
            .created,
        "a key that is there is not refused by the limit"
    );
    keys.delete("first", "build-2").expect("the second key");
    keys.ensure("first", "build-3", limit).expect("the freed place");
    assert_eq!(keys.list("first").expect("the keys").len(), limit);
}

#[test]
fn a_name_that_breaks_the_rule_never_reaches_the_file_system() {
    let temp = tempfile::tempdir().expect("home");
    let keys = store(&temp);
    for name in ["../build", "/tmp/build", "Build", "build 1", ""] {
        let refused = keys.ensure("first", name, 8).expect_err("a refused name");
        assert_eq!(code(&refused), block_keys::INVALID_KEY_NAME, "{name:?}");
        assert_eq!(
            code(&keys.delete("first", name).expect_err("a refused name")),
            block_keys::INVALID_KEY_NAME,
            "{name:?}"
        );
        assert_eq!(
            code(&keys.require("first", name).expect_err("a refused name")),
            block_keys::INVALID_KEY_NAME,
            "{name:?}"
        );
    }
    assert!(
        !temp.path().join("machine-keys").exists(),
        "a refused name creates no directory"
    );
}

#[test]
fn a_panel_that_uses_a_key_offers_that_key_alone() {
    let binding = MachineKeyRef {
        directory: "/home/peters/.horizon/machine-keys/%6669".to_string(),
        name: "build-1".to_string(),
    };
    assert_eq!(
        binding.ssh_option_args(),
        vec![
            "-i".to_string(),
            "/home/peters/.horizon/machine-keys/%6669/build-1".to_string(),
            "-o".to_string(),
            "IdentitiesOnly=yes".to_string(),
            "-o".to_string(),
            "IdentityAgent=none".to_string(),
            "-o".to_string(),
            "UserKnownHostsFile=/home/peters/.horizon/machine-keys/%6669/build-1.known_hosts".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
        ],
        "the person's agent is not offered and the host key is pinned for this key"
    );
}
