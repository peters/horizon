use super::*;

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct TraceCapture(Arc<Mutex<Vec<u8>>>);

impl Write for TraceCapture {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for TraceCapture {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn initial_agent_prompt_is_rejected_for_non_agent_kinds() {
    for kind in [PanelKind::Shell, PanelKind::Ssh, PanelKind::Command, PanelKind::Editor] {
        let options = PanelOptions {
            kind,
            initial_agent_prompt: Some("configure speech input".to_string()),
            ..PanelOptions::default()
        };

        let error = validate_agent_launch_options(&options).expect_err("non-agent prompt must be rejected");
        assert!(error.to_string().contains("only valid for agent panel kinds"));
    }

    let agent_options = PanelOptions {
        kind: PanelKind::Codex,
        initial_agent_prompt: Some("configure speech input".to_string()),
        ..PanelOptions::default()
    };
    assert!(validate_agent_launch_options(&agent_options).is_ok());

    let invalid_shell_mode = PanelOptions {
        kind: PanelKind::Shell,
        agent_login_shell: true,
        ..PanelOptions::default()
    };
    let error = validate_agent_launch_options(&invalid_shell_mode)
        .expect_err("agent login shell must be rejected for non-agent panels");
    assert!(error.to_string().contains("only valid for agent panel kinds"));

    let invalid_spawn = PanelOptions {
        kind: PanelKind::Editor,
        initial_agent_prompt: Some("configure speech input".to_string()),
        ..PanelOptions::default()
    };
    let Err(error) = spawn_panel(PanelId(1), WorkspaceId(1), invalid_spawn) else {
        panic!("non-agent panel spawn unexpectedly accepted an initial agent prompt");
    };
    assert!(error.to_string().contains("only valid for agent panel kinds"));
}

#[test]
fn initial_agent_prompt_rejects_preset_session_directives() {
    let cases = [
        (PanelKind::Codex, vec!["--model", "gpt-5", "resume", "old-session"]),
        (PanelKind::Claude, vec!["--resume=old-session"]),
        (PanelKind::Claude, vec!["--session-id", "old-session"]),
        (PanelKind::Claude, vec!["--continue"]),
        (PanelKind::Claude, vec!["--continue=true"]),
        (PanelKind::Claude, vec!["-r", "old-session"]),
        (PanelKind::Claude, vec!["-rold-session"]),
        (PanelKind::Claude, vec!["-cold-session"]),
    ];

    for (kind, args) in cases {
        let options = PanelOptions {
            kind,
            args: args.into_iter().map(str::to_string).collect(),
            initial_agent_prompt: Some("configure speech input".to_string()),
            ..PanelOptions::default()
        };

        let error = validate_agent_launch_options(&options).expect_err("session directive must be rejected");
        assert!(error.to_string().contains("session or resume arguments"));
    }
}

#[test]
fn codex_initial_prompt_is_the_final_positional_argument() {
    let prompt = "Set up speech; preserve $(pwd) and don't overwrite /tmp/model.gguf";
    let executable = absolute_agent_command();
    let (program, args) = resolve_launch_command(
        Some(executable.clone()),
        vec!["--no-alt-screen".to_string()],
        None,
        PanelKind::Codex,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: None,
            should_resume_binding: false,
            initial_agent_prompt: Some(prompt),
            agent_login_shell: true,
            is_restore: false,
        },
    );

    if cfg!(windows) {
        assert!(program.starts_with('"') && program.ends_with('"'));
        assert_eq!(
            args,
            [AGENT_LAUNCH_HELPER_ARG, executable.as_str(), "--no-alt-screen", prompt]
        );
    } else {
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-lic");
        assert_eq!(
            args[1],
            format!("{} --no-alt-screen {}", shell_escape(&executable), shell_escape(prompt))
        );
    }
}

#[test]
fn claude_initial_prompt_follows_session_and_preset_flags() {
    let binding = AgentSessionBinding::new(
        PanelKind::Claude,
        "11111111-2222-3333-4444-555555555555".to_string(),
        None,
        None,
        None,
    );
    let prompt = "Set up speech input safely";
    let executable = absolute_agent_command();
    let (program, args) = resolve_launch_command(
        Some(executable.clone()),
        vec!["--permission-mode".to_string(), "auto".to_string()],
        None,
        PanelKind::Claude,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: Some(&binding),
            should_resume_binding: false,
            initial_agent_prompt: Some(prompt),
            agent_login_shell: true,
            is_restore: false,
        },
    );

    if cfg!(windows) {
        assert!(program.starts_with('"') && program.ends_with('"'));
        assert_eq!(args[0], AGENT_LAUNCH_HELPER_ARG);
        assert_eq!(args[1], executable);
        assert_eq!(args.last().map(String::as_str), Some(prompt));
        let session_position = args
            .iter()
            .position(|argument| argument == "--session-id")
            .expect("session flag");
        let preset_position = args
            .iter()
            .position(|argument| argument == "--permission-mode")
            .expect("preset flag");
        assert!(session_position < args.len() - 1);
        assert!(preset_position < args.len() - 1);
    } else {
        let command = &args[1];
        assert_eq!(args[0], "-lic");
        let session_position = command.find("--session-id").expect("fresh session flag");
        let preset_position = command.find("--permission-mode auto").expect("preset flags");
        let prompt_position = command.rfind(&shell_escape(prompt)).expect("initial prompt");
        assert!(session_position < prompt_position, "{command}");
        assert!(preset_position < prompt_position, "{command}");
        assert!(command.ends_with(&shell_escape(prompt)), "{command}");
    }
}

#[test]
fn restored_and_restarted_launch_contexts_do_not_repeat_initial_prompt() {
    let prompt = "one-shot speech setup prompt";
    let saved_args = vec!["--no-alt-screen".to_string()];
    let executable = absolute_agent_command();
    let (_, initial_args) = resolve_launch_command(
        Some(executable.clone()),
        saved_args.clone(),
        None,
        PanelKind::Codex,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: None,
            should_resume_binding: false,
            initial_agent_prompt: Some(prompt),
            agent_login_shell: true,
            is_restore: false,
        },
    );
    let (_, restored_args) = resolve_launch_command(
        Some(executable.clone()),
        saved_args.clone(),
        None,
        PanelKind::Codex,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: None,
            should_resume_binding: false,
            initial_agent_prompt: None,
            agent_login_shell: true,
            is_restore: true,
        },
    );
    let (_, restarted_args) = resolve_launch_command(
        Some(executable),
        saved_args,
        None,
        PanelKind::Codex,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: None,
            should_resume_binding: false,
            initial_agent_prompt: None,
            agent_login_shell: true,
            is_restore: true,
        },
    );

    assert!(initial_args.join(" ").contains(prompt));
    assert!(!restored_args.join(" ").contains(prompt));
    assert!(!restarted_args.join(" ").contains(prompt));
    if cfg!(unix) {
        assert_eq!(initial_args[0], "-lic");
        assert_eq!(restored_args[0], "-lic");
        assert_eq!(restarted_args[0], "-lic");
    }
}

#[test]
fn setup_login_shell_mode_is_explicit_and_stable() {
    let executable = absolute_agent_command();
    let (regular_program, regular_args) = wrap_agent_command(executable.clone(), Vec::new(), false);
    let (setup_program, setup_args) = wrap_agent_command(executable.clone(), Vec::new(), true);

    if cfg!(windows) {
        assert_eq!(regular_program, default_shell());
        assert_eq!(regular_args[0], "-ic");
        assert!(setup_program.starts_with('"') && setup_program.ends_with('"'));
        assert_eq!(setup_args, [AGENT_LAUNCH_HELPER_ARG, executable.as_str()]);
    } else {
        assert_eq!(regular_program, default_shell());
        assert_eq!(regular_args[0], "-ic");
        assert_eq!(setup_program, default_shell());
        assert_eq!(setup_args[0], "-lic");
    }
}

#[test]
fn empty_or_unreadable_shell_values_use_the_platform_fallback() {
    let expected = platform_default_shell().to_string();

    assert_eq!(resolve_default_shell(Ok(String::new())), expected);
    assert_eq!(resolve_default_shell(Err(std::env::VarError::NotPresent)), expected);
}

#[test]
fn windows_setup_wrapper_uses_native_helper_for_paths_and_batch_shims() {
    let helper = std::path::Path::new(r"C:\Program Files\Horizon\horizon.exe");
    let target = r"C:\Users\Alice\AppData\Roaming\npm\claude.cmd".to_string();
    let prompt = "set up speech & keep %PATH% unchanged".to_string();

    let (program, args) = windows_setup_agent_command(
        target.clone(),
        vec!["--permission-mode".to_string(), "auto".to_string(), prompt.clone()],
        helper,
    );

    assert_eq!(program, r#""C:\Program Files\Horizon\horizon.exe""#);
    assert_eq!(
        args,
        [
            AGENT_LAUNCH_HELPER_ARG,
            target.as_str(),
            "--permission-mode",
            "auto",
            prompt.as_str(),
        ]
    );
}

#[test]
fn initial_agent_prompt_redacts_trace_command_and_local_paths() {
    let private_cwd = "/home/user/private/source";
    let private_program = "/home/user/private/bin/codex";
    let private_prompt = "inspect /home/user/private/model.gguf";
    let details = terminal_launch_details(Some(private_cwd), private_program, &[private_prompt.to_string()], true);

    assert_eq!(details, TerminalLaunchDetails::Redacted);
    let rendered = format!("{details:?}");
    assert!(!rendered.contains(private_cwd));
    assert!(!rendered.contains(private_program));
    assert!(!rendered.contains(private_prompt));
}

#[test]
fn captured_setup_launch_trace_contains_no_command_prompt_or_local_path() {
    let private_cwd = "/home/user/private/source";
    let private_program = "/home/user/private/bin/setup-agent";
    let private_prompt = "inspect /home/user/private/model.gguf";
    let capture = TraceCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(capture.clone())
        .finish();
    let resume = PanelResume::Fresh;
    let trace = TerminalLaunchTrace {
        kind: PanelKind::Codex,
        resume: &resume,
        session_binding: None,
        should_resume_binding: false,
        details: terminal_launch_details(Some(private_cwd), private_program, &[private_prompt.to_string()], true),
    };

    tracing::subscriber::with_default(subscriber, || log_terminal_launch(PanelId(7), &trace));
    let bytes = capture
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let output = String::from_utf8(bytes).expect("trace output is UTF-8");

    assert!(output.contains("command details redacted"), "{output}");
    assert!(!output.contains(private_cwd), "{output}");
    assert!(!output.contains(private_program), "{output}");
    assert!(!output.contains(private_prompt), "{output}");
}

#[test]
fn shell_escape_quotes_all_shell_control_syntax() {
    assert_eq!(shell_escape("plain-argument_1/path"), "plain-argument_1/path");
    assert_eq!(shell_escape("run; touch /tmp/pwned"), "'run; touch /tmp/pwned'");
    assert_eq!(shell_escape("$(touch /tmp/pwned)"), "'$(touch /tmp/pwned)'");
    assert_eq!(shell_escape("`touch /tmp/pwned`"), "'`touch /tmp/pwned`'");
    assert_eq!(shell_escape("don't expand $HOME"), "'don'\\''t expand $HOME'");
    assert_eq!(
        shell_escape("*?[abc] & value | other # comment"),
        "'*?[abc] & value | other # comment'"
    );
}
