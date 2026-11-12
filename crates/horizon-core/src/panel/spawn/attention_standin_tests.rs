//! The command line that a Claude panel of the fixture stand-in starts with.
//!
//! `scripts/device-smoke/attention_panel.json` is the panel entry that the
//! device fixture writes for `--attention-agent`, and this test reads the same
//! file, so the two sides cannot drift apart. The entry names no command: the
//! launch resolves the agent program on the `PATH` of the panel, which is the
//! stand-in itself, and Horizon inserts its own arguments after that program.
//! The entry also asks for work continuation. That attaches the work ledger of
//! the launch and gives the panel the five `HORIZON_WORK_*` variables that the
//! stand-in sends its events with. The report of a panel needs no single system,
//! so this test states the rule of each Unix system; a system that is no Unix
//! attaches nothing.

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

/// The second shape of the panel entry: a preset with a command of its own,
/// which is the stand-in, and the work continuation off.
const PANEL_COMMAND_ENTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/attention_panel_command.json"
));

const CHILD_ENV: &str = "HORIZON_TEST_ATTENTION_STANDIN_CHILD";
const SESSION: &str = "fixture-session";
/// The conversation of the panel of a system that attaches no ledger.
const SESSION_ID: &str = "6f1d0f9a-2b7c-4a1e-9a58-1c0f7f4a2d31";
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
    let (program, args, env, _plan) = launch(&home, &options);

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
    if cfg!(unix) {
        assert_work_ledger(&options, &horizon_home, &work_plugin, after, session, &env);
    } else {
        assert_without_work_ledger(&options, &horizon_home, &work_plugin, after, session, &env);
    }
    assert_own_command_launch(&home, &horizon_home, &work_plugin);
}

/// The launch of the second shape of the fixture entry: a Claude panel whose
/// preset has a command of its own and whose work continuation is off. The
/// sidebar follows its state all the same, and no resume or card comes of it.
fn assert_own_command_launch(home: &Path, horizon_home: &HorizonHome, work_plugin: &Path) {
    let options = panel_options_of(home, PANEL_COMMAND_ENTRY);
    let command = options
        .command
        .clone()
        .expect("the entry names the command of its preset");
    assert_eq!(
        Path::new(&command).file_name().and_then(std::ffi::OsStr::to_str),
        Some("claude"),
        "the command of the entry is the stand-in that the fixture put on the PATH"
    );
    assert!(
        !options.work_resume.enabled,
        "the entry of the second shape continues no work after a restart"
    );
    let (program, args, env, plan) = launch(&home, &options);
    assert_eq!(
        program, "/bin/bash",
        "the command of a preset runs in the default shell"
    );
    let after = &args[1][command.len()..];
    assert!(
        after.contains("--session-id"),
        "Horizon gives this launch the identity of its conversation: {after}"
    );
    if cfg!(unix) {
        assert!(
            plan.owner.is_some(),
            "the launch of a command of its own reports its state through the work ledger"
        );
        let session = after.find("--session-id").expect("the conversation of the panel");
        assert_work_ledger(&options, horizon_home, work_plugin, after, session, &env);
    } else {
        assert!(plan.owner.is_none(), "this system attaches no ledger");
    }
    // The continuation keeps its own gate: no offer of a resume, and no prompt
    // that Horizon writes into the launch of a panel that only reports.
    assert_eq!(
        plan.state.pending(),
        None,
        "a panel that only reports shows no card of a continuation"
    );
    assert!(
        !args[1].contains("Horizon restored this conversation"),
        "a panel that only reports receives no seed: {}",
        args[1]
    );
}

/// The work ledger that a Unix launch attaches to the panel.
fn assert_work_ledger(
    options: &PanelOptions,
    horizon_home: &HorizonHome,
    work_plugin: &Path,
    after: &str,
    session: usize,
    env: &HashMap<String, String>,
) {
    let ledger = after
        .find(&format!("--plugin-dir '{}'", work_plugin.display()))
        .expect("the work ledger plugin follows the program");
    assert!(
        session < ledger,
        "Horizon puts its arguments between the program and the last one: {after}"
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

/// The launch of a system that attaches no ledger, and of a panel kind that
/// Horizon gives no arguments of its own.
fn assert_without_work_ledger(
    options: &PanelOptions,
    horizon_home: &HorizonHome,
    work_plugin: &Path,
    after: &str,
    session: usize,
    env: &HashMap<String, String>,
) {
    // The plugin of Horizon's browser tools stays on every system, so the
    // command names one plugin folder and never the work ledger.
    assert!(
        !after.contains(&work_plugin.display().to_string()),
        "a panel of this system starts without the work ledger plugin: {after}"
    );
    let named = &after[session..];
    assert_eq!(
        named.split(' ').next(),
        Some("--session-id"),
        "the panel keeps the conversation that Horizon names: {after}"
    );
    assert!(
        named
            .split(' ')
            .nth(1)
            .is_some_and(|id| id.len() == 36 && id.matches('-').count() == 4),
        "the conversation follows its flag: {after}"
    );
    // The work environment of a nested agent is cleared before the launch, so
    // the five variables reach the panel empty and never carry a token.
    for key in [
        crate::agent_work::WORK_ROOT_ENV,
        crate::agent_work::WORK_PANEL_ENV,
        crate::agent_work::WORK_OWNER_ENV,
        crate::agent_work::WORK_KIND_ENV,
        crate::agent_work::WORK_EXECUTABLE_ENV,
    ] {
        assert_eq!(
            env.get(key).map(String::as_str),
            Some(""),
            "a panel of this system starts without the work environment"
        );
    }
    let record = WorkStore::new(horizon_home.root())
        .read(options.local_id.as_deref().unwrap_or_default())
        .expect("read");
    assert!(record.is_none(), "a panel of this system registers no work record");
}

/// The assertions of a system that attaches no work ledger are the same on
/// every platform, but no Unix run takes that branch of the launch. This test
/// gives the helper the command and the environment of such a system, so the
/// branch itself is checked here and not on a system that is no Unix only.
#[test]
fn the_assertions_of_a_system_without_the_work_ledger_hold_for_its_command() {
    let horizon_home = HorizonHome::resolve();
    let host = manifest::host_instance();
    let work_plugin = horizon_home.agent_work_plugin_dir_for_host(host);
    let browser_plugin = horizon_home.claude_plugin_dir_for_host(host);
    let options = PanelOptions {
        local_id: Some("panel-without-a-ledger".to_string()),
        ..PanelOptions::default()
    };
    let after = format!(" --plugin-dir {} --session-id {SESSION_ID}", browser_plugin.display());
    let mut env = HashMap::new();
    for key in crate::agent_work::WORK_ENV_KEYS {
        env.insert(key.to_string(), String::new());
    }
    let session = after.find("--session-id").expect("the flag of the conversation");
    assert_without_work_ledger(&options, &horizon_home, &work_plugin, &after, session, &env);

    // The helper answers no silence: a ledger plugin, a token or a record of
    // the panel is a launch that attaches the ledger, and it is refused.
    let named_ledger = format!(
        " --plugin-dir {} --plugin-dir '{}' --session-id {SESSION_ID}",
        browser_plugin.display(),
        work_plugin.display()
    );
    let mut carried = env.clone();
    carried.insert(crate::agent_work::WORK_OWNER_ENV.to_string(), "a-token".to_string());
    for (command, environment) in [(&named_ledger, &env), (&after, &carried)] {
        let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_without_work_ledger(
                &options,
                &horizon_home,
                &work_plugin,
                command,
                command.find("--session-id").expect("the flag of the conversation"),
                environment,
            );
        }));
        assert!(refused.is_err(), "the helper refuses a launch that attaches the ledger");
    }
}

/// The panel options of the entry that the fixture writes.
fn panel_options(home: &Path) -> PanelOptions {
    panel_options_of(home, PANEL_ENTRY)
}

fn panel_options_of(home: &Path, entry: &str) -> PanelOptions {
    let panel: TerminalConfig = serde_json::from_str(entry).expect("the panel entry of the fixture");
    let workspace = WorkspaceConfig {
        name: "Attention".to_string(),
        color: None,
        cwd: Some(home.display().to_string()),
        position: None,
        terminals: vec![panel.clone()],
    };
    let state = PanelState::from_config(0, "Attention", 0, &workspace, [0.0, 0.0], &panel);
    let mut options = state.to_panel_options(&crate::browser::BrowserConfig::default());
    if let Some(command) = options.command.take() {
        // The fixture names the program of a preset that it copied into its
        // state folder; this child puts the same stand-in on its own PATH.
        assert_eq!(command, "agent_attention_standin.py", "the program of the entry");
        options.command = Some(home.join("bin").join("claude").display().to_string());
        options.work_resume.enabled = false;
        return options;
    }
    assert!(
        options.work_resume.enabled,
        "the entry turns the work ledger of the panel on"
    );
    options
}

fn launch(
    home: &Path,
    options: &PanelOptions,
) -> (
    String,
    Vec<String>,
    HashMap<String, String>,
    crate::agent_work::StartupPlan,
) {
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
    if cfg!(unix) {
        assert!(plan.owner.is_some(), "the launch attaches the work ledger of the panel");
    } else {
        assert!(plan.owner.is_none(), "a system that is no Unix attaches no work ledger");
    }
    (program, args, env, plan)
}
