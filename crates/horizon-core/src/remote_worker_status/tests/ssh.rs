use super::*;
use crate::remote_worker_ssh as ssh;
use std::{
    ffi::OsString,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    path::Path,
    process::Command,
};

#[test]
fn isolated_options_disable_ambient_authentication_trust_and_command_hooks() {
    let fixture = Fixture::new();
    let endpoint = fixture
        .recovered
        .observation()
        .expect("observation")
        .ssh
        .as_ref()
        .expect("SSH");
    let known_hosts = tempfile::NamedTempFile::new().expect("known hosts");
    let command = ssh::prepared_command(
        fixture.recovered.identity().private_key_path(),
        known_hosts.path(),
        endpoint,
    )
    .expect("command");
    assert_eq!(
        known_hosts.as_file().metadata().expect("mode").permissions().mode() & 0o077,
        0
    );
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().expect("UTF8")).collect();
    assert_eq!(&args[..5], ["-F", "none", "-S", "none", "-T"]);
    assert_eq!(args.last(), Some(&"/usr/local/bin/horizon-panel-session request"));
    let output = Command::new("ssh")
        .arg("-G")
        .args(command.get_args())
        .output()
        .expect("SSH config parser");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let effective = String::from_utf8(output.stdout).expect("config");
    for expected in [
        "batchmode yes",
        "identitiesonly yes",
        "identityagent none",
        "stricthostkeychecking true",
        "updatehostkeys false",
        "clearallforwardings yes",
        "permitlocalcommand no",
        "forwardagent no",
        "hostkeyalias horizon-retained-worker",
        "hostkeyalgorithms ssh-ed25519",
    ] {
        assert!(effective.lines().any(|line| line == expected), "missing {expected}");
    }
}

#[test]
fn path_options_preserve_config_quoting_and_percent_tokens_without_environment_expansion() {
    let fixture = Fixture::new();
    let endpoint = fixture
        .recovered
        .observation()
        .expect("observation")
        .ssh
        .as_ref()
        .expect("SSH");
    let path = Path::new("/tmp/synthetic space/percent%h/quote\"back\\slash");
    let command = ssh::prepared_command(path, path, endpoint).expect("literal path");
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().expect("UTF8")).collect();
    assert!(args.contains(&"IdentityFile=\"/tmp/synthetic space/percent%%h/quote\\\"back\\\\slash\""));
    let parsed = Command::new("ssh")
        .arg("-G")
        .args(command.get_args())
        .output()
        .expect("parser");
    assert!(parsed.status.success());
    for path in ["relative", "/tmp/${PRIVATE}/key", "/tmp/new\nline", "/tmp/null\0byte"] {
        assert_eq!(
            ssh::prepared_command(Path::new(path), Path::new("/tmp/known"), endpoint).expect_err("path"),
            RemotePanelStatusError::UnsupportedPath
        );
    }
    let invalid = OsString::from_vec(vec![b'/', 255]);
    assert_eq!(
        ssh::prepared_command(Path::new(&invalid), Path::new("/tmp/known"), endpoint).expect_err("UTF8"),
        RemotePanelStatusError::UnsupportedPath
    );
}
