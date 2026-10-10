use super::*;
use std::path::PathBuf;
fn host() -> Binding {
    Binding {
        id: "test".into(),
        name: "Test".into(),
        ssh: Some(Ssh {
            authentication: SshAuthentication::Key,
            host: "example.test".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: std::env::temp_dir().join("horizon-probe-missing-key"),
            known_hosts: std::env::temp_dir().join("horizon-probe-missing-hosts"),
        }),
        context: None,
        allow_emulation: false,
    }
}
#[test]
fn bindings_reject_shell_options_and_ambiguous_credentials() {
    let mut binding = host();
    assert!(binding.validate().is_ok());
    binding.ssh.as_mut().unwrap().host = "-oProxyCommand=touch /tmp/bad".into();
    assert!(binding.validate().is_err());
    binding = host();
    binding.ssh.as_mut().unwrap().identity_file = "relative".into();
    assert!(binding.validate().is_err());
    binding = host();
    binding.context = Some("default; whoami".into());
    assert!(binding.validate().is_err());
}

#[test]
fn docker_context_names_follow_engine_rules() {
    let mut binding = host();
    for name in ["default", "build.prod", "Stage_1+cpu-test"] {
        binding.context = Some(name.into());
        assert!(binding.validate().is_ok(), "{name}");
    }
    for name in ["", "a", ".prod", "-config", "prod/blue", "prod blue", "default; whoami"] {
        binding.context = Some(name.into());
        assert!(binding.validate().is_err(), "{name}");
    }
}
#[test]
fn missing_requirements_are_blockers_without_attempting_docker() {
    let report = probe(&host(), None, &crate::cloud_runtime::Cancellation::default()).unwrap();
    assert!(!report.ready());
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.checks[0].state, CheckState::Blocked);
    assert_eq!(
        report.checks[0].name,
        if cfg!(windows) {
            "Controller platform"
        } else {
            "SSH identity"
        }
    );
    assert!(report.checks[0].remedy.is_some());
}

#[test]
fn storage_rejects_relative_and_option_paths_before_execution() {
    let binding = host();
    let cancel = crate::cloud_runtime::Cancellation::default();
    let runner = crate::cloud_runtime::command::Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    for path in ["", "relative", "--total", "/ambiguous\npath", "/invalid\0path"] {
        assert!(matches!(
            Transport(&binding).disk_free(&runner, path),
            Err(Error::Invalid("Docker storage needs an absolute Linux path"))
        ));
    }
}

#[test]
fn cancellation_stays_an_error_instead_of_a_host_problem() {
    let cancel = crate::cloud_runtime::Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        probe(&host(), None, &cancel),
        Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
    ));
}

#[test]
// These paths exercise literal Unix OpenSSH arguments.
#[cfg(unix)]
fn credential_paths_are_literal_in_ssh_options() {
    let mut binding = host();
    let ssh = binding.ssh.as_mut().unwrap();
    ssh.identity_file = "/private/key%h".into();
    ssh.known_hosts = "/private/hosts%p".into();
    let command = Transport(&binding).ssh().unwrap();
    assert!(command.get_args().any(|arg| arg == "UpdateHostKeys=no"));
    assert!(command.get_args().any(|arg| arg == "/private/key%%h"));
    assert!(
        command
            .get_args()
            .any(|arg| arg == "UserKnownHostsFile=\"/private/hosts%%p\"")
    );
}

// The Unix controller uses the system OpenSSH configuration parser.
#[cfg(unix)]
#[test]
fn ssh_parses_one_literal_known_hosts_path_with_spaces_quotes_and_percent() {
    let mut binding = host();
    let path = "/private/space name/quote\"hosts%p";
    binding.ssh.as_mut().unwrap().known_hosts = path.into();
    let source = Transport(&binding).ssh().unwrap();
    let output = std::process::Command::new("ssh")
        .arg("-G")
        .args(source.get_args())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == format!("userknownhostsfile {path}"))
    );
}

#[test]
fn old_saved_hosts_keep_key_authentication_and_tailscale_needs_no_key_paths() {
    let old = r#"{"host":"example.test","user":"deploy","port":22,"identity_file":"/private/key","known_hosts":"/private/hosts"}"#;
    let ssh: Ssh = serde_json::from_str(old).unwrap();
    assert_eq!(ssh.authentication, SshAuthentication::Key);
    let mut binding = host();
    binding.ssh = Some(
        serde_json::from_str(r#"{"host":"build","user":"deploy","port":22,"authentication":"tailscale"}"#).unwrap(),
    );
    assert!(binding.validate().is_ok());
    binding.ssh.as_mut().unwrap().port = 2222;
    assert!(binding.validate().is_err());
    binding.ssh.as_mut().unwrap().port = 22;
    binding.ssh.as_mut().unwrap().user = "-options".into();
    assert!(binding.validate().is_err());
}

#[test]
fn tailscale_commands_use_tailnet_host_trust() {
    let mut binding = host();
    let ssh = binding.ssh.as_mut().unwrap();
    ssh.authentication = SshAuthentication::Tailscale;
    ssh.identity_file = PathBuf::new();
    ssh.known_hosts = PathBuf::new();
    let command = Transport(&binding).ssh().unwrap();
    assert_eq!(command.get_program(), "tailscale");
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(&args[..2], &["ssh", "deploy@example.test"]);
    assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
    assert!(args.iter().any(|arg| arg == "PreferredAuthentications=none"));
    assert!(
        !args
            .iter()
            .any(|arg| arg == "-i" || arg.starts_with("UserKnownHostsFile="))
    );
}
