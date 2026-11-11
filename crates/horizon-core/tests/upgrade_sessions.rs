//! The saved sessions of release v0.2.7 and of `main` open in this build.
//!
//! The fixtures of `tests/fixtures/upgrade/` hold a session of each build. A
//! default run of that home must find the profile that the build wrote, resume
//! its session, leave the panels of the removed kinds out with one notice, and
//! keep the board that it read beside the board that it writes.

use std::fs;
use std::path::{Path, PathBuf};

use horizon_core::{
    Config, HorizonHome, PanelKind, RetiredPanels, SessionOpenDisposition, SessionStore, StartupDecision,
};
use tempfile::TempDir;

/// The profile id that release v0.2.7 and `main` wrote in their fixtures. It is
/// the hash of the settings path of a home at `/home/user/project`.
const FIXTURE_PROFILE_ID: &str = "fb3eb95f246d8861";
/// The settings path of that home, as the fixtures name it.
const FIXTURE_CONFIG_PATH: &str = "/home/user/project/.horizon/config.yaml";

fn fixture(version: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/upgrade")
        .join(version)
        .join(name)
}

/// The profile id of a home: the hash of `<home>/config.yaml`, which is what
/// release v0.2.7 and `main` hashed for the settings path of a default run.
fn profile_of(home: &HorizonHome) -> String {
    let root = fs::canonicalize(home.root()).expect("the home");
    fnv1a64(&root.join("config.yaml").to_string_lossy())
}

/// The hash that the profile id is made of, as both builds compute it.
fn fnv1a64(value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A private home that holds the session of `version`, with the profile id of
/// that home. The id is the hash of the settings path, so a copy of a session at
/// another path names another profile; every other byte is the file of the old
/// build.
fn home_with_the_session(version: &str) -> (TempDir, HorizonHome, PathBuf, String) {
    let root = tempfile::tempdir().expect("a temporary home");
    let home = HorizonHome::from_root(root.path().join(".horizon"));
    let session_dir = home.sessions_dir().join(session_id(version));
    fs::create_dir_all(&session_dir).expect("the folder of the saved session");
    let profile = profile_of(&home);
    for name in ["meta.yaml", "runtime.yaml"] {
        let text = fs::read_to_string(fixture(version, name)).expect("a file of the saved session");
        let text = text.replace(FIXTURE_PROFILE_ID, &profile);
        let text = text.replace(FIXTURE_CONFIG_PATH, &home.config_path().display().to_string());
        fs::write(session_dir.join(name), text).expect("a file of the saved session");
    }
    let index = fs::read_to_string(fixture(version, "index.yaml")).expect("the session index");
    fs::write(home.session_index_path(), index.replace(FIXTURE_PROFILE_ID, &profile)).expect("the session index");
    (root, home, session_dir, profile)
}

fn session_id(version: &str) -> &'static str {
    match version {
        "v0.2.7" => "9272f72e-d124-4012-9ca2-761544678156",
        "main" => "5d1f0c76-8a2e-4f83-9f10-2c7b6a4e9d21",
        other => panic!("no fixture of {other}"),
    }
}

#[test]
fn the_fixtures_name_the_profile_of_their_settings_path() {
    assert_eq!(
        fnv1a64(FIXTURE_CONFIG_PATH),
        FIXTURE_PROFILE_ID,
        "the index and the metadata of both fixtures name the hash of their own settings path, \
         which a real home computes from its own path"
    );
}

#[test]
fn a_default_run_keeps_the_profile_of_the_older_builds() {
    let root = tempfile::tempdir().expect("a temporary home");
    let home = HorizonHome::from_root(root.path().join(".horizon"));
    fs::create_dir_all(home.root()).expect("the home of the person");

    let store = SessionStore::new(home.clone(), home.config_path());

    assert_eq!(
        store.profile_id(),
        profile_of(&home),
        "the settings of this build are config.json, and the profile stays the one of config.yaml"
    );
}

#[test]
fn a_run_that_names_the_smoke_settings_keeps_the_profile_of_that_path() {
    let root = tempfile::tempdir().expect("a temporary home");
    let home = HorizonHome::from_root(root.path().join(".horizon"));
    fs::create_dir_all(home.root()).expect("the home of the person");
    let smoke = home.root().join("smoke.json");

    let store = SessionStore::new(home.clone(), smoke.clone());

    assert_ne!(store.profile_id(), profile_of(&home), "a smoke run is another profile");
    assert_eq!(store.profile_id(), fnv1a64(&smoke.to_string_lossy()));
}

#[test]
fn the_v027_session_resumes_with_the_panels_that_this_build_has() {
    let (_root, home, _session_dir, _profile) = home_with_the_session("v0.2.7");
    let store = SessionStore::new(home.clone(), home.config_path());

    let decision = store
        .prepare_startup(&Config::default())
        .expect("the startup of the profile of v0.2.7");

    match decision {
        StartupDecision::Open { disposition, session } => {
            assert_eq!(disposition, SessionOpenDisposition::Resume);
            assert_eq!(session.session_id, session_id("v0.2.7"));
            let workspaces: Vec<(String, Vec<PanelKind>)> = session
                .runtime_state
                .workspaces
                .iter()
                .map(|workspace| {
                    (
                        workspace.name.clone(),
                        workspace.panels.iter().map(|panel| panel.kind).collect(),
                    )
                })
                .collect();
            assert_eq!(
                workspaces,
                vec![
                    ("Workspace 1".to_string(), vec![PanelKind::Shell]),
                    ("Workspace 2".to_string(), vec![PanelKind::Editor]),
                    ("Workspace 3".to_string(), vec![]),
                ],
                "every panel that this build still has comes back, and the removed kinds leave \
                 their workspace empty"
            );
            assert_eq!(
                left_out(&session.retired_panels),
                vec![("git_changes", 1), ("usage", 1)]
            );
        }
        other => panic!("the session of v0.2.7 must open, and the decision was {other:?}"),
    }
}

#[test]
fn the_main_session_resumes_with_the_panels_that_this_build_has() {
    let (_root, home, _session_dir, _profile) = home_with_the_session("main");
    let store = SessionStore::new(home.clone(), home.config_path());

    let decision = store
        .prepare_startup(&Config::default())
        .expect("the startup of the profile of main");

    match decision {
        StartupDecision::Open { disposition, session } => {
            assert_eq!(
                disposition,
                SessionOpenDisposition::Resume,
                "main resumes the last session of the profile with no question, and this build \
                 does the same"
            );
            assert_eq!(session.session_id, session_id("main"));
            let kinds: Vec<Vec<PanelKind>> = session
                .runtime_state
                .workspaces
                .iter()
                .map(|workspace| workspace.panels.iter().map(|panel| panel.kind).collect())
                .collect();
            assert_eq!(
                kinds,
                vec![vec![], vec![PanelKind::Shell], vec![], vec![], vec![PanelKind::Editor],],
                "the Shell and the Markdown panel come back in their workspaces, and the Usage, \
                 Git Changes and setup board panels leave theirs empty"
            );
            assert_eq!(
                left_out(&session.retired_panels),
                vec![("git_changes", 1), ("setup_board", 1), ("usage", 1)],
                "one notice names the count of each removed kind"
            );
        }
        other => panic!("the session of main must open, and the decision was {other:?}"),
    }
}

#[test]
fn a_default_run_makes_no_second_profile() {
    let (_root, home, _session_dir, profile) = home_with_the_session("main");
    let index_before = fs::read(home.session_index_path()).expect("the index");
    let store = SessionStore::new(home.clone(), home.config_path());

    store
        .prepare_startup(&Config::default())
        .expect("the startup of the profile of main");
    store
        .resume_session(session_id("main"))
        .expect("the session of main loads");

    assert_eq!(store.profile_id(), profile, "the run reads the profile that main wrote");
    assert_eq!(
        fs::read(home.session_index_path()).expect("the index again"),
        index_before,
        "a start that only reads the profile writes no second one"
    );
}

#[test]
fn the_load_of_an_old_board_writes_nothing() {
    let (_root, home, session_dir, _profile) = home_with_the_session("v0.2.7");
    let runtime_before = fs::read(session_dir.join("runtime.yaml")).expect("the board");
    let store = SessionStore::new(home.clone(), home.config_path());

    store
        .prepare_startup(&Config::default())
        .expect("the startup of the profile of v0.2.7");

    assert_eq!(
        fs::read(session_dir.join("runtime.yaml")).expect("the board again"),
        runtime_before,
        "a load does not rewrite the board of the person"
    );
    assert!(
        !session_dir.join("runtime.yaml.before-3").exists(),
        "the copy appears at the first write, not at the load"
    );
}

#[test]
fn the_first_write_keeps_the_board_of_the_old_build() {
    let (_root, home, session_dir, _profile) = home_with_the_session("v0.2.7");
    let runtime_before = fs::read(session_dir.join("runtime.yaml")).expect("the board");
    let store = SessionStore::new(home.clone(), home.config_path());
    let session = store
        .resume_session(session_id("v0.2.7"))
        .expect("the session of v0.2.7 loads");

    store
        .save_runtime_state(&session.session_id, &session.runtime_state)
        .expect("the first write of this build");
    let kept = session_dir.join("runtime.yaml.before-3");
    assert_eq!(
        fs::read(&kept).expect("the copy of the old board"),
        runtime_before,
        "the board of the old build stays beside the board of this build"
    );
    let written = fs::read_to_string(session_dir.join("runtime.yaml")).expect("the new board");
    assert!(
        written.starts_with("version: 3\n"),
        "this build wrote its own board: {written}"
    );
    assert!(!written.contains("usage"), "the removed panel is not written again");

    store
        .save_runtime_state(&session.session_id, &session.runtime_state)
        .expect("a second write");
    assert_eq!(
        fs::read(&kept).expect("the copy of the old board again"),
        runtime_before,
        "a later write does not overwrite the copy"
    );
}

#[test]
fn the_first_write_of_a_main_board_keeps_it_too() {
    let (_root, home, session_dir, _profile) = home_with_the_session("main");
    let runtime_before = fs::read(session_dir.join("runtime.yaml")).expect("the board");
    let store = SessionStore::new(home.clone(), home.config_path());
    let session = store
        .resume_session(session_id("main"))
        .expect("the session of main loads");

    store
        .save_runtime_state(&session.session_id, &session.runtime_state)
        .expect("the write");

    assert_eq!(
        fs::read(session_dir.join("runtime.yaml.before-3")).expect("the copy"),
        runtime_before,
        "a board of main is the same version as this build, and it still keeps its removed panels"
    );
}

fn left_out(panels: &RetiredPanels) -> Vec<(&'static str, usize)> {
    panels.kinds()
}
