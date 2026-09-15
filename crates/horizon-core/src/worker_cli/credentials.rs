//! Explicit per-attempt disclosure; Git/task claims and remote work stay untouched.

use super::{Error, storage::Context};
use horizon_core::{
    remote_git_setup::{self as git, ConfiguredRemoteGitSetupRequest},
    remote_github_credential::{RemoteCredentialInstallation, RepositoryPat},
    remote_ssh_identity::RemoteSshIdentityStore,
};
use serde_json::{Value, json};
use std::{fs::File, io::Read, os::fd::AsFd, path::Path};

pub(super) fn unbuffered_stdin() -> Result<File, Error> {
    // Global buffered stdin can retain token fragments outside our zeroizing storage.
    std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .map_err(|_| Error::Input)
}

pub(super) fn operation_id(value: &std::ffi::OsStr) -> Result<uuid::Uuid, Error> {
    let id = uuid::Uuid::parse_str(value.to_str().ok_or(Error::Input)?).map_err(|_| Error::Input)?;
    if id.is_nil() {
        return Err(Error::Input);
    }
    Ok(id)
}

pub(super) fn install(context: &Context, operation: uuid::Uuid, input: impl Read) -> Result<Value, Error> {
    let claim = format!("credential-{operation}");
    require_unclaimed(&context.receipt.root.join(format!("{claim}.claimed")))?;
    let store = context.store()?;
    let expected = context.saved()?.environment_summary();
    let config = &context.receipt.intent.config;
    let request = ConfiguredRemoteGitSetupRequest {
        expected: &expected,
        client_session_id: &context.receipt.session,
    };
    let prepared = git::prepare_configured_remote_git_credential(&store, config, request)
        .map_err(|error| Error::Remote(error.to_string()))?;
    let bytes = read_secret(input)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Input)?;
    let token = RepositoryPat::new(text.trim_end_matches(['\r', '\n'])).map_err(|_| Error::Input)?;
    context.claim(&claim)?;
    let result = git::install_configured_remote_git_credential(
        &store,
        &RemoteSshIdentityStore::new(&context.home),
        config,
        request,
        &prepared,
        &token,
    )
    .map_err(|error| Error::Remote(error.to_string()))?;
    Ok(json!({"operation_id": operation, "credential": status(result)}))
}

fn require_unclaimed(path: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Error::Claimed),
    }
}

fn read_secret(input: impl Read) -> Result<zeroize::Zeroizing<Vec<u8>>, Error> {
    // Allocate inside Zeroizing so partial reads and oversized input are erased too.
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(16_387));
    input.take(16_387).read_to_end(&mut bytes).map_err(|_| Error::Input)?;
    if bytes.len() > 16_386 {
        return Err(Error::Input);
    }
    Ok(bytes)
}

fn status(result: RemoteCredentialInstallation) -> &'static str {
    match result {
        RemoteCredentialInstallation::Installed => "installed",
        RemoteCredentialInstallation::Present => "present",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_cannot_escape_the_claim_namespace_or_use_nil() {
        for value in ["", "../git", "00000000-0000-0000-0000-000000000000", "not-an-id"] {
            assert!(operation_id(value.as_ref()).is_err());
        }
        let id = uuid::Uuid::new_v4();
        assert_eq!(operation_id(id.to_string().as_ref()).unwrap(), id);
    }

    #[test]
    fn existing_or_symlinked_claims_are_refused_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credential.claimed");
        require_unclaimed(&path).unwrap();
        std::fs::write(&path, b"original").unwrap();
        assert!(matches!(require_unclaimed(&path), Err(Error::Claimed)));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let link = root.path().join("dangling.claimed");
        std::os::unix::fs::symlink(root.path().join("missing"), &link).unwrap();
        assert!(matches!(require_unclaimed(&link), Err(Error::Claimed)));
    }

    #[test]
    fn secret_input_is_bounded_and_read_errors_do_not_echo_partial_bytes() {
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic_secret"))
            }
        }
        assert!(matches!(read_secret(&vec![b'a'; 16_387][..]), Err(Error::Input)));
        let error = read_secret(FailedRead).unwrap_err();
        assert!(!error.to_string().contains("synthetic_secret"));
        let bytes = read_secret(&b"synthetic_token\r\n"[..]).unwrap();
        assert!(RepositoryPat::new(std::str::from_utf8(&bytes).unwrap().trim_end_matches(['\r', '\n'])).is_ok());
    }

    #[test]
    fn reopened_attempt_refuses_input_and_preserves_original_git_and_task_claims() {
        struct Unread;
        impl Read for Unread {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("replayed disclosure read stdin")
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("task");
        let request = json!({
            "config": {"local_docker": [{"name": "fixture", "docker_host": format!("unix://{}", directory.path().join("missing.sock").display())}]},
            "target": {"provider": "local_docker", "profile": "fixture", "image": format!("registry.example/worker@sha256:{}", "a".repeat(64)), "disk_gib": 20, "lifetime": "persistent"},
            "repository": {"repository": "fixture/repository", "commit": "a".repeat(40), "branch": "test/fixture"},
            "command": {"program": "/bin/false", "args": []}, "working_directory": ".",
            "setup_expires_at_millis": (time::OffsetDateTime::now_utc().unix_timestamp() + 300) * 1000,
            "issue": "fixture"
        });
        assert!(super::super::operations::create(&root, serde_json::from_value(request).unwrap()).is_err());
        let context = Context::open(&root).unwrap();
        context.claim("git").unwrap();
        context.claim("start-original").unwrap();
        let operation = uuid::Uuid::new_v4();
        context.claim(&format!("credential-{operation}")).unwrap();
        let before: Vec<_> = [
            "receipt.json".to_owned(),
            "git.claimed".into(),
            "start-original.claimed".into(),
            format!("credential-{operation}.claimed"),
        ]
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(root.join(&name)).unwrap();
            (name, bytes)
        })
        .collect();
        drop(context);
        let reopened = Context::open(&root).unwrap();
        assert!(matches!(install(&reopened, operation, Unread), Err(Error::Claimed)));
        for (name, bytes) in before {
            assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
        }
    }
}
