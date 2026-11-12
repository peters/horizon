//! The command line that a Claude panel of the fixture stand-in starts with.
//!
//! `scripts/device-smoke/attention_panel.json` is the panel entry that the
//! device fixture writes for `--attention-agent`, and this test reads the same
//! file, so the two sides cannot drift apart. The entry names no command: the
//! launch resolves the agent program on the `PATH` of the panel, which is the
//! stand-in itself, and Horizon inserts its own arguments after that program.
//! The entry also asks for work continuation, which is what attaches the work
//! ledger and gives the panel the five `HORIZON_WORK_*` variables that the
//! stand-in sends its events with.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::AgentLaunchContext;
use super::{agent_env, resolve_launch_command};
use crate::agent_work::{WorkLaunch, WorkStore};
use crate::browser::manifest;
use crate::config::{TerminalConfig, WorkspaceConfig};
use crate::horizon_home::HorizonHome;
use crate::panel::{PanelKind, PanelOptions, PanelResume};
use crate::runtime_state::PanelState;

/// The panel entry of the device fixture, which `attention_agent` of
/// `scripts/device-smoke/serve.py` writes into the config of the fixture.
const PANEL_ENTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/attention_panel.json"
));

const CHILD_ENV: &str = "HORIZON_TEST_ATTENTION_STANDIN_CHILD";
const SESSION: &str = "fixture-session";
/// The program that the `PATH` of the fixture gives the name `claude`. It takes
/// the launch arguments without an answer, as the stand-in does.
const STAND_IN: &str = "#!/bin/sh\nexit 0\n";

#[test]
fn a_claude_panel_of_the_fixture_stand_in_keeps_the_arguments_and_the_work_environment() {
    if std::env::var_os(CHILD_ENV).is_some() {
        exercise_launch();
        return;
    }
    // A child process isolates HOME, PATH and SHELL from the other tests of this
    // process, and only that child resolves the program of the panel on a `PATH`
    // that names the fixture.
    let home = tempfile::tempdir().expect("isolated home");
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("fixture bin");
    std::fs::write(home.path().join(".bashrc"), "").expect("isolated shell config");
    crate::util::stand_in_program_in(&bin, "claude", STAND_IN).expect("stand-in program");
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            concat!(
                module_path!(),
                "::a_claude_panel_of_the_fixture_stand_in_keeps_the_arguments_and_the_work_environment"
            )
            .trim_start_matches("horizon_core::"),
            "--nocapture",
        ])
        .env_clear()
        .env(CHILD_ENV, "1")
        .env("HOME", home.path())
        .env("SHELL", "/bin/bash")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .current_dir(home.path())
        .output()
        .expect("isolated test process");
    assert!(
        output.status.success(),
        "isolated launch failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn exercise_launch() {
    let home = std::env::var_os("HOME").map(PathBuf::from).expect("fixture home");
    let horizon_home = HorizonHome::resolve();
    let host = manifest::host_instance();
    // A start of Horizon installs both plugins before it opens a panel.
    let work_plugin = horizon_home.agent_work_plugin_dir_for_host(host);
    std::fs::create_dir_all(work_plugin.join("hooks")).expect("work plugin");
    std::fs::write(work_plugin.join("hooks/hooks.json"), "{}").expect("work hooks");
    std::fs::create_dir_all(horizon_home.claude_plugin_dir_for_host(host)).expect("claude plugin");

    let options = panel_options(&home);
    let (program, args, env) = launch(&home, &options);

    let stand_in = home.join("bin").join("claude");
    assert_eq!(program, "/bin/bash", "a Claude panel starts in the default shell");
    assert_eq!(args.len(), 2);
    assert_eq!(args[0], "-ic");
    let command = &args[1];
    let launch = format!("exec '{}'", stand_in.display());
    assert!(
        command.starts_with(&launch),
        "the program of the panel is the stand-in that the fixture put first on PATH: {command}"
    );
    let after = &command[launch.len()..];
    let session = after.find("--session-id").expect("the panel names its conversation");
    let ledger = after
        .find(&format!("--plugin-dir '{}'", work_plugin.display()))
        .expect("the work ledger plugin follows the program");
    assert!(
        session < ledger,
        "Horizon puts its arguments between the program and the last one: {command}"
    );

    assert_eq!(
        env.get(crate::agent_work::WORK_ROOT_ENV),
        Some(&horizon_home.root().display().to_string())
    );
    assert_eq!(env.get(crate::agent_work::WORK_PANEL_ENV), options.local_id.as_ref());
    assert_eq!(
        env.get(crate::agent_work::WORK_KIND_ENV).map(String::as_str),
        Some("claude")
    );
    assert_eq!(
        env.get(crate::agent_work::WORK_EXECUTABLE_ENV).map(String::as_str),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.to_str().map(str::to_owned))
            .as_deref()
    );
    assert!(
        env.get(crate::agent_work::WORK_OWNER_ENV)
            .is_some_and(|owner| !owner.is_empty())
    );
    let record = WorkStore::new(horizon_home.root())
        .read(options.local_id.as_deref().unwrap_or_default())
        .expect("read")
        .expect("the launch registered the work record of the panel");
    assert_eq!(record.ledger.session_id, SESSION);
}

/// The panel options of the entry that the fixture writes.
fn panel_options(home: &Path) -> PanelOptions {
    let panel: TerminalConfig = serde_json::from_str(PANEL_ENTRY).expect("the panel entry of the fixture");
    let workspace = WorkspaceConfig {
        name: "Attention".to_string(),
        color: None,
        cwd: Some(home.display().to_string()),
        position: None,
        terminals: vec![panel.clone()],
    };
    let state = PanelState::from_config(0, "Attention", 0, &workspace, [0.0, 0.0], &panel);
    let options = state.to_panel_options(&crate::browser::BrowserConfig::default());
    assert!(
        options.work_resume.enabled,
        "the entry turns the work ledger of the panel on"
    );
    assert!(
        options.command.is_none(),
        "the launch arguments only stay with the default command"
    );
    options
}

fn launch(home: &Path, options: &PanelOptions) -> (String, Vec<String>, HashMap<String, String>) {
    let policy = options.work_resume.clone();
    let resume = PanelResume::Fresh;
    let (program, mut args) = resolve_launch_command(
        options.command.clone(),
        options.args.clone(),
        None,
        options.kind,
        AgentLaunchContext {
            resume: &resume,
            session_binding: None,
            should_resume_binding: false,
            is_restore: false,
        },
    );
    let local_id = options.local_id.clone().unwrap_or_default();
    let mut env = agent_env(options.kind, &local_id, options.command.is_none());
    // The terminal of a panel takes HOME from the user database of this
    // computer, so the probe shell reads the shell configuration of this child.
    env.insert("HOME".to_string(), home.display().to_string());
    let plan = WorkLaunch {
        panel: &local_id,
        kind: PanelKind::Claude,
        policy: &policy,
        cwd: options.cwd.as_deref(),
        session_id: Some(SESSION),
        default_command: options.command.is_none(),
    }
    .start(&program, false, true, &mut args, &mut env);
    assert!(plan.owner.is_some(), "the launch attaches the work ledger of the panel");
    (program, args, env)
}
