//! The launch line of a pi panel, and the report that it names.
//!
//! A pi panel is a launch that reports its state: Horizon starts the shell with
//! the command of the panel, appends the report extension with `-e`, and puts
//! the five `HORIZON_WORK_*` variables of that one process into the environment.
//! pi stops at a path that names no file, so a launch whose report is missing on
//! disk attaches nothing: it keeps its command line and the screen rule.
//!
//! The test reads the report of `HorizonHome`, which is the home of the child it
//! starts, so the paths below are the same paths the launch writes.

use std::path::{Path, PathBuf};

use super::{AgentLaunchContext, agent_env, resolve_launch_command};
use crate::agent_work::{WorkLaunch, WorkStore};
use crate::browser::manifest;
use crate::horizon_home::HorizonHome;
use crate::panel::{PanelKind, PanelOptions, PanelResume};

const CHILD_ENV: &str = "HORIZON_TEST_PI_ATTENTION_CHILD";
/// The program that the `PATH` of the child gives the name `pi`.
const STAND_IN: &str = "#!/bin/sh\nexit 0\n";

/// The panel that a person opens with the pi preset.
fn pi_panel(local_id: &str) -> PanelOptions {
    PanelOptions {
        kind: PanelKind::Pi,
        local_id: Some(local_id.to_string()),
        ..PanelOptions::default()
    }
}

/// The launch of the panel, resolved the way the panel start resolves it.
fn launch(
    options: &PanelOptions,
    session_id: Option<&str>,
) -> (
    String,
    Vec<String>,
    std::collections::HashMap<String, String>,
    crate::agent_work::StartupPlan,
) {
    let (program, mut args) = resolve_launch_command(
        options.command.clone(),
        options.args.clone(),
        None,
        options.kind,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: None,
            should_resume_binding: false,
            is_restore: false,
        },
    );
    let local_id = options.local_id.clone().unwrap_or_default();
    let mut env = agent_env(options.kind, &local_id, options.command.is_none());
    let plan = WorkLaunch {
        panel: &local_id,
        kind: options.kind,
        policy: &options.work_resume,
        cwd: options.cwd.as_deref(),
        session_id,
        default_command: options.command.is_none(),
    }
    .start(&program, false, true, &mut args, &mut env);
    (program, args, env, plan)
}

#[test]
fn a_pi_panel_loads_the_report_beside_the_extensions_of_the_person() {
    if std::env::var_os(CHILD_ENV).is_some() {
        exercise_launch();
        return;
    }
    // A child isolates HOME, PATH and SHELL from the other tests of this process:
    // only that child resolves the program of the panel on a `PATH` that names the
    // stand-in, and only it writes the work record of the panel.
    let home = tempfile::tempdir().expect("isolated home");
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).expect("fixture bin");
    std::fs::write(home.path().join(".bashrc"), "").expect("isolated shell config");
    crate::util::stand_in_program_in(&bin, "pi", STAND_IN).expect("stand-in program");
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            concat!(
                module_path!(),
                "::a_pi_panel_loads_the_report_beside_the_extensions_of_the_person"
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
    let report = horizon_home.agent_work_extension_path_for_host(manifest::host_instance());

    // A Horizon home without the report attaches nothing: no `-e` ever names a
    // file that pi cannot load, and no record is registered.
    assert!(!report.exists(), "the fixture starts without the report");
    assert_without_report(home.as_path(), &horizon_home, &report);

    std::fs::create_dir_all(report.parent().expect("report directory")).expect("report directory");
    std::fs::write(&report, "export default function activate() {}\n").expect("report file");
    assert_with_report(home.as_path(), &horizon_home, &report);
}

/// The launch of a pi panel whose report is a file on disk.
fn assert_with_report(home: &Path, horizon_home: &HorizonHome, report: &Path) {
    let options = pi_panel("pi-panel");
    let (program, args, env, plan) = launch(&options, None);
    assert_eq!(program, "/bin/bash", "a pi panel starts in the default shell");
    assert_eq!(args.len(), 2, "the launch is one command in a login shell");
    assert_eq!(args[0], "-ic");
    let command = &args[1];
    let expected = format!("-e '{}'", report.display());
    assert!(
        command.ends_with(&expected),
        "the report follows the command of the panel: {command}"
    );
    assert!(
        command.starts_with("pi"),
        "the program of the preset stays the program of the panel: {command}"
    );
    assert!(plan.owner.is_some(), "the launch attaches the work record of the panel");
    for (key, value) in [
        (
            crate::agent_work::WORK_ROOT_ENV,
            horizon_home.root().display().to_string(),
        ),
        (
            crate::agent_work::WORK_PANEL_ENV,
            options.local_id.clone().unwrap_or_default(),
        ),
        (crate::agent_work::WORK_KIND_ENV, "pi".to_string()),
    ] {
        assert_eq!(env.get(key), Some(&value), "the launch sets {key}");
    }
    assert_eq!(
        env.get(crate::agent_work::WORK_EXECUTABLE_ENV).map(String::as_str),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.to_str().map(str::to_owned))
            .as_deref(),
        "the report starts the executable of this Horizon"
    );
    assert!(
        env.get(crate::agent_work::WORK_OWNER_ENV)
            .is_some_and(|owner| !owner.is_empty()),
        "the launch holds a token of its own"
    );
    let record = WorkStore::new(horizon_home.root())
        .read("pi-panel")
        .expect("read")
        .expect("the launch registered the work record of the panel");
    assert_eq!(record.kind, PanelKind::Pi);
    assert!(
        record.ledger.session_id.is_empty(),
        "pi names no conversation at the launch, and the report names its own"
    );

    // A command of the person's own is the same launch, as it is for Claude.
    let own = PanelOptions {
        command: Some(home.join("bin").join("pi").display().to_string()),
        local_id: Some("pi-panel-own-command".to_string()),
        ..pi_panel("pi-panel-own-command")
    };
    let (_program, args, env, plan) = launch(&own, None);
    assert!(plan.owner.is_some(), "a command of its own reports its state too");
    assert!(
        args[1].ends_with(&expected),
        "the report follows the command of the person: {}",
        args[1]
    );
    assert_eq!(
        env.get(crate::agent_work::WORK_KIND_ENV).map(String::as_str),
        Some("pi")
    );
}

/// The launch of a pi panel whose report is not on disk.
fn assert_without_report(home: &Path, horizon_home: &HorizonHome, report: &Path) {
    let options = pi_panel("pi-panel-without-a-report");
    let (program, args, env, plan) = launch(&options, None);
    assert_eq!(program, "/bin/bash");
    assert!(
        !args[1].contains("-e"),
        "a launch without the report names no extension: {}",
        args[1]
    );
    assert!(
        !args[1].contains(&report.display().to_string()),
        "the command line names no file that pi could not load: {}",
        args[1]
    );
    assert!(plan.owner.is_none(), "a launch that cannot report keeps no record");
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
            "a launch without a report sets no work identity: {key}"
        );
    }
    let record = WorkStore::new(horizon_home.root())
        .read(&options.local_id.clone().unwrap_or_default())
        .expect("read");
    assert!(record.is_none(), "no record is registered");
    let _ = home;
}
