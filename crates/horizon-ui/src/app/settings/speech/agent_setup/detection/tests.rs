use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use horizon_core::{PanelKind, PanelResume, PresetConfig};

#[cfg(unix)]
use super::probe::login_shell::{
    BoundedCommandResult, LOGIN_SHELL_ENV_COMMAND, LoginShellProbe, MAX_LOGIN_SHELL_LINE_BYTES,
    MAX_LOGIN_SHELL_OUTPUT_BYTES, login_shell_path, login_shell_probe_args, probe_login_shell, read_login_shell_path,
    run_bounded,
};
use super::probe::{
    ProbeEnvironment, expand_explicit_path, is_explicit_path, path_candidates, probe_candidate, verify_preset_command,
    verify_preset_command_with_environment, windows_executable_name_candidates, windows_path_candidates,
    windows_path_is_helper_launchable,
};
use super::{SpeechSetupAgentAvailability, SpeechSetupProbeFailure};

#[test]
fn explicit_paths_are_classified_without_shell_parsing() {
    assert!(is_explicit_path("/opt/tools/codex"));
    assert!(is_explicit_path("./bin/codex"));
    assert!(is_explicit_path("tools\\claude.exe"));
    assert!(is_explicit_path("~/bin/codex"));
    assert!(!is_explicit_path("codex"));
}

#[test]
fn tilde_and_relative_explicit_paths_use_injected_roots() {
    let environment = ProbeEnvironment::for_test(
        None,
        Some(PathBuf::from("/bin/sh")),
        PathBuf::from("/users/alice"),
        PathBuf::from("/work/repo"),
    );
    assert_eq!(
        expand_explicit_path("~/bin/codex", &environment),
        PathBuf::from("/users/alice/bin/codex")
    );
    assert_eq!(
        expand_explicit_path("./tools/claude", &environment),
        PathBuf::from("/work/repo/./tools/claude")
    );
}

#[test]
fn windows_candidate_rules_use_pathext_case_insensitively() {
    let candidates = windows_executable_name_candidates("codex", Some(OsStr::new(".EXE;.CMD;.exe")));
    assert_eq!(candidates, ["codex.EXE", "codex.CMD"]);
    let explicit = windows_executable_name_candidates("CLAUDE.cmd", Some(OsStr::new(".EXE;.CMD")));
    assert_eq!(explicit, ["CLAUDE.cmd"]);
    let explicit_exe = windows_executable_name_candidates("CLAUDE.exe", Some(OsStr::new(".CMD")));
    assert_eq!(explicit_exe, ["CLAUDE.exe"]);
    let defaults = windows_executable_name_candidates("codex", None);
    assert_eq!(defaults, ["codex.COM", "codex.EXE", "codex.BAT", "codex.CMD"]);
}

#[test]
fn windows_candidates_preserve_path_then_pathext_precedence() {
    let first = PathBuf::from("first-bin");
    let second = PathBuf::from("second-bin");
    let path = std::env::join_paths([&first, &second]).expect("join paths");
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut environment =
        ProbeEnvironment::for_test(Some(PathBuf::from(path)), None, workspace.clone(), workspace.clone());
    environment.path_ext = Some(OsString::from(".CMD;.EXE"));
    let candidates = windows_path_candidates("codex", &environment);

    assert_eq!(
        candidates,
        [
            workspace.join(first).join("codex.CMD"),
            workspace.join("first-bin").join("codex.EXE"),
            workspace.join(second).join("codex.CMD"),
            workspace.join("second-bin").join("codex.EXE"),
        ]
    );
}

#[cfg(unix)]
#[test]
fn relative_and_empty_path_entries_are_rebased_to_workspace_cwd() {
    let path = std::env::join_paths([PathBuf::new(), PathBuf::from("tools")]).expect("join paths");
    let environment = ProbeEnvironment::for_test(
        Some(PathBuf::from(path)),
        None,
        PathBuf::from("/home/alice"),
        PathBuf::from("/workspace/project"),
    );

    assert_eq!(
        path_candidates("agent", &environment),
        [
            PathBuf::from("/workspace/project/agent"),
            PathBuf::from("/workspace/project/tools/agent"),
        ]
    );
}

#[test]
fn windows_helper_support_is_limited_to_native_and_batch_targets() {
    for supported in ["agent", "agent.com", "agent.EXE", "agent.bat", "agent.CmD"] {
        assert!(windows_path_is_helper_launchable(PathBuf::from(supported).as_path()));
    }
    for unsupported in ["agent.ps1", "agent.js", "agent.vbs", "agent.wsf"] {
        assert!(!windows_path_is_helper_launchable(PathBuf::from(unsupported).as_path()));
    }
}

#[cfg(windows)]
#[test]
fn windows_probe_accepts_batch_shims_and_stops_at_unsupported_precedence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let script = temp.path().join("setup-agent.ps1");
    let batch = temp.path().join("setup-agent.cmd");
    fs::write(&script, "Write-Output unsupported").expect("write PowerShell shim");
    fs::write(&batch, "@echo off\r\n").expect("write batch shim");
    let path = std::env::join_paths([temp.path()]).expect("join PATH");
    let mut environment = ProbeEnvironment::for_test(
        Some(PathBuf::from(path)),
        None,
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );
    environment.path_ext = Some(OsString::from(".PS1;.CMD;.EXE"));

    assert!(matches!(
        probe_candidate("setup-agent", &environment, Duration::ZERO),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
    fs::remove_file(script).expect("remove unsupported shim");
    assert_eq!(
        probe_candidate("setup-agent", &environment, Duration::ZERO),
        SpeechSetupAgentAvailability::Available {
            executable: batch.display().to_string()
        }
    );
}

#[cfg(windows)]
#[test]
fn windows_explicit_suffix_is_checked_even_when_pathext_omits_it() {
    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("setup-agent.exe");
    fs::write(&executable, b"not executed by this probe").expect("write executable candidate");
    let mut environment = ProbeEnvironment::for_test(None, None, temp.path().to_path_buf(), temp.path().to_path_buf());
    environment.path_ext = Some(OsString::from(".CMD"));

    assert_eq!(
        probe_candidate(executable.to_string_lossy().as_ref(), &environment, Duration::ZERO),
        SpeechSetupAgentAvailability::Available {
            executable: executable.display().to_string()
        }
    );
}

#[cfg(unix)]
#[test]
fn path_probe_requires_an_executable_file() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("codex-test-probe");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );
    assert_eq!(
        probe_candidate("codex-test-probe", &environment, Duration::from_millis(100)),
        SpeechSetupAgentAvailability::Available {
            executable: executable.display().to_string()
        }
    );

    fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).expect("remove executable bit");
    assert_eq!(
        probe_candidate(
            executable.to_string_lossy().as_ref(),
            &environment,
            Duration::from_millis(100)
        ),
        SpeechSetupAgentAvailability::Missing
    );
}

#[cfg(unix)]
#[test]
fn executable_for_a_different_permission_class_is_not_available() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("other-users-only");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write candidate");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o001)).expect("set other-only execute bit");
    let environment = ProbeEnvironment::for_test(
        None,
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    assert_eq!(
        probe_candidate(
            executable.to_string_lossy().as_ref(),
            &environment,
            Duration::from_millis(100)
        ),
        SpeechSetupAgentAvailability::Missing
    );
}

#[cfg(unix)]
#[test]
fn tilde_command_resolves_to_the_exact_launch_path() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create bin");
    let executable = bin.join("codex");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let environment = ProbeEnvironment::for_test(
        None,
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    assert_eq!(
        probe_candidate("~/bin/codex", &environment, Duration::from_millis(100)),
        SpeechSetupAgentAvailability::Available {
            executable: executable.display().to_string()
        }
    );
}

#[cfg(unix)]
#[test]
fn relative_workspace_and_relative_command_resolve_once_to_an_absolute_path() {
    use std::os::unix::fs::PermissionsExt;

    let process_cwd = std::env::current_dir().expect("process cwd");
    let temp = tempfile::tempdir_in(&process_cwd).expect("temp dir inside process cwd");
    let relative_cwd = temp.path().strip_prefix(&process_cwd).expect("relative temp cwd");
    let tools = temp.path().join("tools");
    fs::create_dir(&tools).expect("create tools dir");
    let executable = tools.join("setup-agent");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let environment = ProbeEnvironment::for_test(
        None,
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        relative_cwd.to_path_buf(),
    );

    let SpeechSetupAgentAvailability::Available { executable: resolved } =
        probe_candidate("./tools/setup-agent", &environment, Duration::from_millis(100))
    else {
        panic!("relative executable should be available");
    };
    assert!(Path::new(&resolved).is_absolute());
    assert_eq!(
        Path::new(&resolved)
            .canonicalize()
            .expect("resolved path canonicalizes"),
        executable.canonicalize().expect("expected path canonicalizes")
    );
}

#[cfg(unix)]
#[test]
fn missing_explicit_path_is_definitively_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );
    assert_eq!(
        probe_candidate(
            temp.path().join("missing-codex").to_string_lossy().as_ref(),
            &environment,
            Duration::from_millis(100)
        ),
        SpeechSetupAgentAvailability::Missing
    );
}

#[cfg(unix)]
#[test]
fn launch_preflight_detects_an_executable_removed_after_the_cached_scan() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("codex");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let preset = PresetConfig {
        name: "Codex".to_string(),
        alias: None,
        kind: PanelKind::Codex,
        command: Some(executable.display().to_string()),
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    };

    assert_eq!(
        verify_preset_command(&preset, Some(temp.path())),
        SpeechSetupAgentAvailability::Available {
            executable: executable.display().to_string()
        }
    );
    fs::remove_file(&executable).expect("remove executable after detection");
    assert_eq!(
        verify_preset_command(&preset, Some(temp.path())),
        SpeechSetupAgentAvailability::Missing
    );
}

#[cfg(unix)]
#[test]
fn launch_preflight_checks_effective_access_without_starting_a_helper() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("setup-agent");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o001)).expect("set metadata execute bit");
    let preset = PresetConfig {
        name: "Setup agent".to_string(),
        alias: None,
        kind: PanelKind::Claude,
        command: Some(executable.display().to_string()),
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    };

    assert_eq!(
        verify_preset_command(&preset, Some(temp.path())),
        SpeechSetupAgentAvailability::Missing
    );
}

#[cfg(unix)]
#[test]
fn direct_candidate_is_unknown_when_the_exact_launch_shell_cannot_start() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("setup-agent");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(temp.path().join("missing-shell")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    assert!(matches!(
        probe_candidate("setup-agent", &environment, Duration::from_millis(100)),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn tilde_launch_shell_stays_unknown_even_when_the_expanded_file_exists() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create bin");
    let shell = bin.join("custom-shell");
    let executable = temp.path().join("setup-agent");
    fs::write(&shell, "#!/bin/sh\nexit 0\n").expect("write shell");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).expect("make shell executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(PathBuf::from("~/bin/custom-shell")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );
    let preset = PresetConfig {
        name: "Setup agent".to_string(),
        alias: None,
        kind: PanelKind::Claude,
        command: Some(executable.display().to_string()),
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    };

    assert!(matches!(
        probe_candidate(
            executable.to_string_lossy().as_ref(),
            &environment,
            Duration::from_millis(100)
        ),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
    assert!(matches!(
        verify_preset_command_with_environment(&preset, &environment),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn launch_preflight_rechecks_the_exact_launch_shell() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let executable = temp.path().join("setup-agent");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make executable");
    let preset = PresetConfig {
        name: "Setup agent".to_string(),
        alias: None,
        kind: PanelKind::Claude,
        command: Some(executable.display().to_string()),
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: None,
    };
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(temp.path().join("missing-shell")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    assert!(matches!(
        verify_preset_command_with_environment(&preset, &environment),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn login_shell_candidate_is_passed_as_data_not_source() {
    let temp = tempfile::tempdir().expect("temp dir");
    let candidate = "missing-agent; touch SHELL_INJECTION";
    let args = login_shell_probe_args();
    assert_eq!(args[0], "-lic");
    assert_eq!(args[1], LOGIN_SHELL_ENV_COMMAND);
    assert!(args.iter().all(|argument| argument != candidate));

    let mut probe_paths = vec![temp.path().to_path_buf()];
    if let Some(path) = std::env::var_os("PATH") {
        probe_paths.extend(std::env::split_paths(&path));
    }
    let probe_path = std::env::join_paths(probe_paths).expect("join probe PATH");
    let environment = ProbeEnvironment::for_test(
        Some(PathBuf::from(probe_path)),
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );
    let result = probe_login_shell(
        PathBuf::from("/bin/sh").as_path(),
        candidate,
        &environment,
        Duration::from_millis(500),
    );
    assert!(matches!(result, LoginShellProbe::Missing));
    assert!(!temp.path().join("SHELL_INJECTION").exists());
}

#[cfg(unix)]
#[test]
fn login_shell_probe_returns_an_exact_executable_path() {
    let temp = tempfile::tempdir().expect("temp dir");
    let environment = ProbeEnvironment::for_test(
        None,
        Some(PathBuf::from("/bin/sh")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    let result = probe_login_shell(Path::new("/bin/sh"), "sh", &environment, Duration::from_millis(500));
    let LoginShellProbe::Available(executable) = result else {
        panic!("expected an exact executable path, got {result:?}");
    };
    assert!(Path::new(&executable).is_absolute());
    assert_eq!(Path::new(&executable).file_name(), Some(OsStr::new("sh")));
}

#[cfg(unix)]
#[test]
fn login_shell_path_uses_the_final_environment_value() {
    assert_eq!(
        login_shell_path(b"PATH=/startup/noise\nSHELL=/bin/sh\nPATH=/actual/bin\n"),
        Some(OsString::from("/actual/bin"))
    );
}

#[cfg(unix)]
#[test]
fn login_shell_output_capture_is_memory_bounded_and_ignores_large_noise() {
    let mut output = vec![b'x'; MAX_LOGIN_SHELL_LINE_BYTES + 1];
    output.extend_from_slice(b"\nPATH=/safe/bin\n");

    assert_eq!(
        read_login_shell_path(output.as_slice()).expect("bounded noisy output"),
        Some(OsString::from("/safe/bin"))
    );

    let oversized = vec![b'\n'; MAX_LOGIN_SHELL_OUTPUT_BYTES + 1];
    let error = read_login_shell_path(oversized.as_slice()).expect_err("total output limit must be enforced");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[cfg(target_os = "linux")]
#[test]
fn login_shell_capture_times_out_when_an_escaped_child_holds_stdout() {
    use std::os::unix::fs::PermissionsExt;

    if Command::new("setsid").arg("--version").output().is_err() {
        return;
    }

    let temp = tempfile::tempdir().expect("temp dir");
    let escaped_pid = temp.path().join("escaped.pid");
    let shell = temp.path().join("stdout-holder-shell");
    fs::write(
        &shell,
        format!(
            "#!/bin/sh\nsetsid /bin/sh -c 'echo $$ > \"{}\"; sleep 5' &\nprintf 'PATH=/bin\\n'\nexit 0\n",
            escaped_pid.display()
        ),
    )
    .expect("write shell");
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).expect("make shell executable");
    let environment = ProbeEnvironment::for_test(
        None,
        Some(shell.clone()),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    let started = Instant::now();
    let result = probe_login_shell(&shell, "missing-agent", &environment, Duration::from_millis(300));
    assert!(matches!(result, LoginShellProbe::Timeout));
    assert!(started.elapsed() < Duration::from_secs(1));

    if let Ok(pid) = fs::read_to_string(&escaped_pid)
        && let Ok(raw_pid) = pid.trim().parse::<i32>()
        && let Some(pid) = rustix::process::Pid::from_raw(raw_pid)
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
}

#[cfg(unix)]
#[test]
fn login_shell_probe_uses_a_shell_agnostic_environment_command() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let bin = temp.path().join("profile-bin");
    fs::create_dir(&bin).expect("create profile bin");
    let executable = bin.join("agent-from-profile");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write candidate");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("make candidate executable");

    let shell = temp.path().join("non-posix-profile-shell");
    fs::write(
        &shell,
        format!(
            "#!/bin/sh\n[ \"$1\" = -lic ] || exit 90\n[ \"$2\" = env ] || exit 91\nPATH='{}':\"$PATH\" exec env\n",
            bin.display()
        ),
    )
    .expect("write shell");
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).expect("make shell executable");
    let environment =
        ProbeEnvironment::for_test(None, Some(shell), temp.path().to_path_buf(), temp.path().to_path_buf());

    assert_eq!(
        probe_candidate("agent-from-profile", &environment, Duration::from_millis(500)),
        SpeechSetupAgentAvailability::Available {
            executable: executable.display().to_string()
        }
    );
}

#[cfg(unix)]
#[test]
fn missing_login_shell_environment_utility_stays_unknown() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let shell = temp.path().join("shell-without-env");
    fs::write(&shell, "#!/bin/sh\nPATH=/no-such-directory exec env\n").expect("write shell");
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).expect("make shell executable");
    let environment =
        ProbeEnvironment::for_test(None, Some(shell), temp.path().to_path_buf(), temp.path().to_path_buf());

    assert!(matches!(
        probe_candidate("not-on-path", &environment, Duration::from_millis(500)),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn bounded_process_probe_kills_a_hung_child() {
    let temp = tempfile::tempdir().expect("temp dir");
    let pid_file = temp.path().join("child.pid");
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            "sleep 5 & child=$!; printf '%s' \"$child\" > \"$1\"; wait \"$child\"",
            "probe-test",
        ])
        .arg(&pid_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let started = Instant::now();
    let result = run_bounded(&mut command, Duration::from_millis(300));
    assert!(matches!(result, BoundedCommandResult::Timeout));
    assert!(started.elapsed() < Duration::from_secs(1));

    let child_pid = fs::read_to_string(&pid_file).expect("profile child pid");
    let reaped_deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < reaped_deadline && process_is_running(&child_pid) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_is_running(&child_pid),
        "timed-out probe left child process {child_pid} alive"
    );
}

#[cfg(unix)]
fn process_is_running(pid: &str) -> bool {
    Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", pid])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|output| {
            output.status.success() && !String::from_utf8_lossy(&output.stdout).trim_start().starts_with('Z')
        })
}

#[cfg(unix)]
#[test]
fn broken_login_shell_is_unknown_instead_of_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(PathBuf::from("/bin/false")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    assert!(matches!(
        probe_candidate("not-on-path", &environment, Duration::from_millis(100)),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Failed(_))
    ));
}

#[cfg(unix)]
#[test]
fn login_shell_timeout_stays_unknown() {
    let temp = tempfile::tempdir().expect("temp dir");
    fs::write(temp.path().join(".bash_profile"), "/bin/sleep 2\n").expect("write slow login profile");
    let environment = ProbeEnvironment::for_test(
        Some(temp.path().to_path_buf()),
        Some(PathBuf::from("/bin/bash")),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    );

    assert_eq!(
        probe_candidate("not-on-path", &environment, Duration::from_millis(30)),
        SpeechSetupAgentAvailability::Unknown(SpeechSetupProbeFailure::Timeout)
    );
}
