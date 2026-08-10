use super::*;

#[test]
fn shell_launch_args_adds_login_flag_when_requested() {
    assert_eq!(shell_launch_args(Vec::new(), true), vec!["-l".to_string()]);
}

#[test]
fn disconnected_snapshot_launch_command_exits_without_reconnecting() {
    let (program, args) = disconnected_snapshot_launch_command();

    if cfg!(windows) {
        assert_eq!(program, "cmd.exe");
        assert_eq!(args, vec!["/C".to_string(), "exit".to_string()]);
    } else {
        assert_eq!(program, default_shell());
        assert_eq!(args, vec!["-c".to_string(), "exit".to_string()]);
    }
}

#[test]
fn resolve_launch_command_preserves_custom_shell_without_args() {
    let (program, args) = resolve_launch_command(
        Some("/usr/local/bin/custom-shell".to_string()),
        Vec::new(),
        None,
        PanelKind::Shell,
        fresh_launch_context(&PanelResume::Fresh),
    );

    assert_eq!(program, "/usr/local/bin/custom-shell");
    assert!(args.is_empty());
}

#[test]
fn resolve_launch_command_adds_login_flag_only_for_default_shell() {
    let (program, args) = resolve_launch_command(
        None,
        Vec::new(),
        None,
        PanelKind::Shell,
        fresh_launch_context(&PanelResume::Fresh),
    );

    assert_eq!(program, default_shell());
    if PLATFORM_USES_LOGIN_SHELL {
        assert_eq!(args, vec!["-l".to_string()]);
    } else {
        assert!(args.is_empty());
    }
}

#[test]
fn resolve_launch_command_prefers_structured_ssh_connection() {
    let connection = SshConnection {
        host: "prod-api".to_string(),
        user: Some("deploy".to_string()),
        port: Some(2222),
        ..SshConnection::default()
    };

    let (program, args) = resolve_launch_command(
        Some("custom-ignored".to_string()),
        vec!["--ignored".to_string()],
        Some(connection),
        PanelKind::Ssh,
        fresh_launch_context(&PanelResume::Fresh),
    );

    assert_eq!(program, "ssh");
    assert_eq!(
        args,
        vec![
            "-p".to_string(),
            "2222".to_string(),
            "-o".to_string(),
            "ServerAliveInterval=15".to_string(),
            "-o".to_string(),
            "ServerAliveCountMax=1".to_string(),
            "deploy@prod-api".to_string(),
        ]
    );
}
