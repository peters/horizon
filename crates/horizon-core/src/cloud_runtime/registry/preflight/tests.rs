use super::*;
use crate::cloud_runtime::{Cancellation, registry::tests::fixture};

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn selected_pull_is_checked_before_publisher_and_failure_stops_preflight() {
    let (_root, settings) = fixture();
    let prepared = Prepared::for_image(&settings, "registry.example/team/worker", None, true)
        .unwrap()
        .unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: vec![],
    };
    for fail_pull in [true, false] {
        let mut seen = Vec::new();
        let result = prepared.preflight_with(&runner, |auth, material, purpose| {
            seen.push((auth.username.clone(), material.secret.to_string()));
            if fail_pull || matches!(purpose, Purpose::Publish) {
                Err(purpose.failure())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(seen[0], ("reader".into(), "synthetic-pull".into()));
        assert_eq!(seen.len(), if fail_pull { 1 } else { 2 });
        if !fail_pull {
            assert_eq!(seen[1], ("writer".into(), "synthetic-push".into()));
        }
        assert!(!prepared.verified);
        assert!(prepared.journal.validation().is_none());
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn pull_only_checks_ignore_publisher_and_never_authorize_provider_transfer() {
    let (_root, settings) = fixture();
    let prepared = Prepared::for_image(&settings, "registry.example/team/worker", None, false)
        .unwrap()
        .unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: vec![],
    };
    let mut seen = 0;
    prepared
        .preflight_with(&runner, |_, _, purpose| {
            assert!(matches!(purpose, Purpose::Pull));
            seen += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, 1);
    assert!(!prepared.verified);
    cancel.cancel();
    assert!(matches!(
        prepared.preflight_with(&runner, |_, _, _| panic!("cancelled")),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
}

#[cfg(unix)]
#[test]
fn login_sends_only_loaded_secret_on_stdin_and_never_logs_a_reply() {
    use std::os::unix::fs::PermissionsExt as _;
    let (root, settings) = fixture();
    let prepared = Prepared::for_image(&settings, "registry.example/team/worker", None, true)
        .unwrap()
        .unwrap();
    let script = root.path().join("docker");
    std::fs::write(&script, r#"#!/bin/sh
[ "$1" = --config ] && [ "$3" = --host ] && [ "$4" = unix:///synthetic/docker.sock ] || exit 2
[ "$(cat "$2/config.json")" = '{"auths":{"registry.example":{}}}' ] || exit 3
shift 4
[ "$1" = login ] && [ "$2" = --username ] && [ "$3" = reader ] && [ "$4" = --password-stdin ] && [ "$5" = registry.example ] || exit 2
[ "$(cat)" = synthetic-pull ] || exit 4
printf 'synthetic-secret-reply'
printf 'synthetic-secret-error' >&2
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| panic!("private login must not emit"),
        secrets: vec![],
    };
    let before = std::fs::read(prepared.pull.config.path().join("config.json")).unwrap();
    login(
        &prepared.binding.pull,
        &prepared.pull,
        &prepared.binding.repository,
        &runner,
        Some("unix:///synthetic/docker.sock"),
        Command::new(&script),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(prepared.pull.config.path().join("config.json")).unwrap(),
        before
    );
}
