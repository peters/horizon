//! The program of the orchestrator of one workspace: its arguments, its identity
//! and the environment that it shares with an agent panel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::PanelKind;
use crate::agent_bridge::{
    BridgeIdentity, ENV_REMOVE_NAMES, ENV_REMOVE_PREFIXES, SessionChoice, actor, claude, claude_for_workspace,
};
use crate::panel::spawn::agent_env;

/// A local id of the form that `crate::new_local_id` makes.
const WORKSPACE: &str = "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e";
/// The session of every test.
const SESSION: &str = "00000000-0000-4000-8000-000000000001";
/// The home of the Horizon of a test.
const HOME: &str = "/tmp/horizon-home";
/// The program that runs that Horizon.
const EXECUTABLE: &str = "/opt/horizon";

/// The identity of the workspace of the test, with `plugin_dir` when it names one.
fn identity(plugin_dir: Option<&str>) -> BridgeIdentity {
    BridgeIdentity {
        actor: actor::of_workspace(WORKSPACE).expect("the actor of the workspace"),
        host_instance: crate::browser::manifest::host_instance().to_string(),
        home: PathBuf::from(HOME),
        executable: PathBuf::from(EXECUTABLE),
        plugin_dir: plugin_dir.map(PathBuf::from),
    }
}

/// The environment of one program, as text.
fn environment(spec: &crate::agent_bridge::ProgramSpec) -> BTreeMap<String, String> {
    spec.env
        .iter()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

/// The arguments of one program, as text.
fn arguments(spec: &crate::agent_bridge::ProgramSpec) -> Vec<String> {
    spec.args
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

/// The flags of the line format that every program of the bridge carries.
fn line_format() -> Vec<String> {
    arguments(&claude(Path::new("/tmp/work"), SessionChoice::New(SESSION.to_owned())))
}

#[test]
fn the_program_of_a_workspace_adds_the_plugin_dir_of_claude_code() {
    let spec = claude_for_workspace(
        Path::new("/tmp/work"),
        SessionChoice::New(SESSION.to_owned()),
        &identity(Some("/tmp/horizon-home/runtime/agent-plugins/host/claude-code")),
    );
    let mut expected = line_format();
    expected.push("--plugin-dir".to_owned());
    expected.push("/tmp/horizon-home/runtime/agent-plugins/host/claude-code".to_owned());
    assert_eq!(arguments(&spec), expected);
    assert_eq!(spec.program, PathBuf::from("claude"));
    assert_eq!(spec.cwd, PathBuf::from("/tmp/work"));
}

#[test]
fn a_start_without_a_plugin_dir_names_no_plugin() {
    let spec = claude_for_workspace(
        Path::new("/tmp/work"),
        SessionChoice::New(SESSION.to_owned()),
        &identity(None),
    );
    assert_eq!(arguments(&spec), line_format());
}

#[test]
fn the_identity_of_a_workspace_names_the_same_host_as_an_agent_panel() {
    let identity = identity(None);
    let spec = claude_for_workspace(
        Path::new("/tmp/work"),
        SessionChoice::New(SESSION.to_owned()),
        &identity,
    );
    let bridge = environment(&spec);
    let panel = agent_env(PanelKind::Claude, WORKSPACE, true);
    // The names of the identity come from the code that an agent panel uses, so
    // the two callers cannot drift apart.
    let names: Vec<&str> = crate::panel::spawn::agent_identity_vars(String::new(), "", "")
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        names,
        [
            "HORIZON",
            "HORIZON_BROWSER_ACTOR",
            crate::browser::manifest::HOST_INSTANCE_ENV,
            "HORIZON_EXECUTABLE",
        ]
    );
    assert_eq!(bridge.get("HORIZON"), Some(&"1".to_owned()));
    assert_eq!(bridge.get("HORIZON"), panel.get("HORIZON"));
    assert_eq!(
        bridge.get(crate::browser::manifest::HOST_INSTANCE_ENV),
        panel.get(crate::browser::manifest::HOST_INSTANCE_ENV),
        "one host instance for the panel and for the bridge"
    );
    assert_eq!(
        bridge.get("HORIZON_EXECUTABLE"),
        Some(&EXECUTABLE.to_owned()),
        "the bridge names the program of the horizon that started it"
    );
    assert_eq!(
        bridge.get(horizon_browser_control::paths::HOME_ENV),
        None,
        "the child inherits HORIZON_HOME as a panel does, so this identity writes no home"
    );
    assert_eq!(
        bridge.get("HORIZON_BROWSER_ACTOR"),
        Some(&identity.actor),
        "the bridge speaks as the orchestrator of its workspace, not as a panel"
    );
    assert_eq!(
        panel.get("HORIZON_BROWSER_ACTOR"),
        Some(&crate::browser_actor(WORKSPACE))
    );
}

#[test]
fn the_identity_of_a_workspace_holds_no_work_variable() {
    let spec = claude_for_workspace(
        Path::new("/tmp/work"),
        SessionChoice::New(SESSION.to_owned()),
        &identity(None),
    );
    let bridge = environment(&spec);
    assert_eq!(
        bridge.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![
            "HORIZON",
            "HORIZON_BROWSER_ACTOR",
            crate::browser::manifest::HOST_INSTANCE_ENV,
            "HORIZON_EXECUTABLE",
        ],
        "the identity is exactly the four names that an agent panel gets, and no secret"
    );
    let panel = agent_env(PanelKind::Claude, WORKSPACE, true);
    for key in crate::agent_work::WORK_ENV_KEYS {
        assert!(
            !bridge.contains_key(key),
            "the bridge of a workspace holds no work variable, but it holds {key}"
        );
        assert!(panel.contains_key(key), "the panel overrides the inherited {key}");
    }
    for prefix in ENV_REMOVE_PREFIXES {
        assert!(
            !bridge.keys().any(|key| key.starts_with(prefix)),
            "the program removes every inherited {prefix} name"
        );
    }
    assert_eq!(
        spec.env_remove,
        ENV_REMOVE_NAMES
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
        "a nested program does not speak as the agent that started it"
    );
}

#[test]
fn a_local_id_that_is_not_of_the_actor_form_gets_no_identity() {
    assert!(BridgeIdentity::for_workspace(WORKSPACE).is_some());
    for refused in ["", "workspace", "8f14e45f/ceea", ".."] {
        assert!(
            BridgeIdentity::for_workspace(refused).is_none(),
            "the local id {refused:?} names no orchestrator"
        );
    }
}

#[test]
fn the_identity_of_a_workspace_names_the_plugin_dir_only_while_it_is_a_directory() {
    let identity = BridgeIdentity::for_workspace(WORKSPACE).expect("the identity of the workspace");
    let plugin_dir =
        crate::HorizonHome::resolve().claude_plugin_dir_for_host(crate::browser::manifest::host_instance());
    assert_eq!(
        identity.plugin_dir.is_some(),
        plugin_dir.is_dir(),
        "the identity names the plugin directory only while it is a directory of this computer"
    );
    assert_eq!(
        identity.actor,
        actor::of_workspace(WORKSPACE).expect("the actor of the workspace")
    );
    assert_eq!(identity.host_instance, crate::browser::manifest::host_instance());
    assert_eq!(identity.home, crate::HorizonHome::resolve().root());
}

#[test]
fn a_session_id_that_is_not_of_the_form_of_a_uuid_never_reaches_the_command_line() {
    for id in ["--dangerously-skip-permissions", "a-session", &"a".repeat(4096)] {
        let spec = claude(Path::new("/tmp/work"), SessionChoice::Resume(id.to_owned()));
        let arguments = arguments(&spec);
        assert!(
            !arguments.iter().any(|argument| argument == id),
            "the id {id:?} is on the command line: {arguments:?}"
        );
        assert!(
            !arguments.iter().any(|argument| argument == "--resume"),
            "a resume that cannot name a session starts a new one: {arguments:?}"
        );
        assert!(
            arguments.iter().any(|argument| argument == "--session-id"),
            "{arguments:?}"
        );
        assert_eq!(
            arguments.last().map(String::len),
            Some(36),
            "the new session carries a uuid: {arguments:?}"
        );
    }
    let spec = claude(Path::new("/tmp/work"), SessionChoice::Resume(SESSION.to_owned()));
    assert_eq!(arguments(&spec).last(), Some(&SESSION.to_owned()));
}

#[test]
fn a_start_without_an_identity_removes_the_identity_of_a_nested_agent() {
    let spec = claude(Path::new("/tmp/work"), SessionChoice::New(SESSION.to_owned()));
    let removed: Vec<String> = spec
        .env_remove
        .iter()
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    assert!(removed.iter().any(|name| name == "CLAUDECODE"), "{removed:?}");
    for name in [
        "HORIZON",
        "HORIZON_BROWSER_ACTOR",
        crate::browser::manifest::HOST_INSTANCE_ENV,
        "HORIZON_EXECUTABLE",
    ] {
        assert!(
            removed.iter().any(|removed| removed == name),
            "a start without an identity removes {name}: {removed:?}"
        );
    }
    assert_eq!(
        spec.env,
        Vec::new(),
        "a start without an identity sets no identity of its own"
    );
}
