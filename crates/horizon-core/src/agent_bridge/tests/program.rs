//! The program of the orchestrator of one workspace: its arguments, its identity
//! and the environment that it shares with an agent panel.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::PanelKind;
use crate::agent_bridge::{
    BridgeIdentity, ENV_REMOVE_NAMES, ENV_REMOVE_PREFIXES, SessionChoice, actor, claude, claude_for_front_desk,
    claude_for_workspace,
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

/// The identity of the front desk of the test.
fn front_desk_identity(plugin_dir: Option<&str>) -> BridgeIdentity {
    BridgeIdentity {
        actor: actor::FRONT_DESK.to_owned(),
        host_instance: crate::browser::manifest::host_instance().to_string(),
        home: PathBuf::from(HOME),
        executable: PathBuf::from(EXECUTABLE),
        plugin_dir: plugin_dir.map(PathBuf::from),
    }
}

/// The three additions of the front desk that follow the line of an orchestrator.
fn front_desk_additions(model: &str) -> Vec<String> {
    vec![
        "--tools".to_owned(),
        crate::agent_bridge::NO_TOOLS.to_owned(),
        "--model".to_owned(),
        model.to_owned(),
        "--append-system-prompt".to_owned(),
        crate::agent_bridge::FRONT_DESK_PROMPT.to_owned(),
    ]
}

#[test]
fn the_program_of_the_front_desk_is_an_orchestrator_with_no_tool_and_its_own_prompt() {
    let spec = claude_for_front_desk(
        Path::new("/home/person/.horizon/front-desk"),
        SessionChoice::New(SESSION.to_owned()),
        &front_desk_identity(Some("/tmp/horizon-home/runtime/agent-plugins/host/claude-code")),
    );

    // The line of an orchestrator of a workspace, and then the three additions.
    let mut expected = arguments(&claude_for_workspace(
        Path::new("/home/person/.horizon/front-desk"),
        SessionChoice::New(SESSION.to_owned()),
        &front_desk_identity(Some("/tmp/horizon-home/runtime/agent-plugins/host/claude-code")),
    ));
    assert!(expected.iter().any(|argument| argument == "--plugin-dir"));
    // A test process names no model, so the front desk takes the model of this
    // line of Horizon.
    expected.extend(front_desk_additions(crate::agent_bridge::FRONT_DESK_MODEL));
    assert_eq!(arguments(&spec), expected);
    assert_eq!(spec.cwd, PathBuf::from("/home/person/.horizon/front-desk"));
    assert_eq!(
        environment(&spec).get("HORIZON_BROWSER_ACTOR"),
        Some(&actor::FRONT_DESK.to_owned()),
        "the front desk speaks as its own actor"
    );
    assert_eq!(
        environment(&spec).get("HORIZON_BROWSER_ACTOR"),
        Some(&crate::agent_bridge::actor::FRONT_DESK.to_owned()),
        "the actor of the front desk is the one constant of the bridge"
    );
}

#[test]
fn the_front_desk_names_no_tool_and_writes_about_a_hundred_and_twenty_words() {
    assert_eq!(crate::agent_bridge::NO_TOOLS, "", "the empty word leaves no tool");
    let prompt = crate::agent_bridge::FRONT_DESK_PROMPT;
    let words = prompt.split_whitespace().count();
    assert!(
        (90..=150).contains(&words),
        "the system text of the front desk is about 120 words, and it is {words}: {prompt}"
    );
    assert!(prompt.contains("front desk of Horizon"));
    assert!(prompt.contains("work item"));
    assert!(prompt.contains("no tool"));
}

#[test]
fn the_model_of_the_front_desk_holds_a_name_and_falls_back_to_haiku() {
    let model = crate::agent_bridge::front_desk_model;
    assert_eq!(crate::agent_bridge::FRONT_DESK_MODEL, "haiku");
    assert_eq!(model(None), "haiku");
    assert_eq!(model(Some("sonnet")), "sonnet");
    assert_eq!(model(Some("claude-sonnet-5.0_x")), "claude-sonnet-5.0_x");
    assert_eq!(model(Some("5-sonnet")), "5-sonnet");
    assert_eq!(
        model(Some(&"a".repeat(64))),
        "a".repeat(64),
        "the longest name that a model of the front desk holds"
    );
    // A value that is not a name of a model: another model of this line of
    // Horizon answers instead, so no other argument of the program can arrive.
    for refused in [
        "",
        " ",
        // A name holds no slash, no space and no control character, and it does
        // not start with a hyphen: the value can never be read as another option.
        "--dangerously-skip-permissions",
        "-5",
        "sonnet 5",
        "sonnet\n",
        "/tmp/model",
        "a;rm",
        "mod\u{e8}le",
        &"a".repeat(65),
        "../../etc/passwd",
    ] {
        assert_eq!(
            model(Some(refused)),
            "haiku",
            "the value {refused:?} is no model of the front desk"
        );
    }
}

#[test]
fn the_front_desk_keeps_the_rule_of_the_session_id_of_an_orchestrator() {
    // An id that is not of the form of a UUID never reaches the command line.
    for id in ["--dangerously-skip-permissions", "a-session", &"a".repeat(4096)] {
        let spec = claude_for_front_desk(
            Path::new("/tmp/front-desk"),
            SessionChoice::Resume(id.to_owned()),
            &front_desk_identity(None),
        );
        let arguments = arguments(&spec);
        assert!(
            !arguments.iter().any(|argument| argument == id),
            "the id {id:?} is on the command line: {arguments:?}"
        );
        assert!(
            !arguments.iter().any(|argument| argument == "--resume"),
            "a resume that cannot name a session starts a new one: {arguments:?}"
        );
        let at = arguments
            .iter()
            .position(|argument| argument == "--session-id")
            .expect("the new session of the start");
        assert_eq!(
            arguments.get(at + 1).map(String::len),
            Some(36),
            "the new session carries a uuid: {arguments:?}"
        );
    }
    let spec = claude_for_front_desk(
        Path::new("/tmp/front-desk"),
        SessionChoice::Resume(SESSION.to_owned()),
        &front_desk_identity(None),
    );
    let arguments = arguments(&spec);
    let at = arguments
        .iter()
        .position(|argument| argument == "--resume")
        .expect("the resumed session of the front desk");
    assert_eq!(arguments.get(at + 1), Some(&SESSION.to_owned()));
}
