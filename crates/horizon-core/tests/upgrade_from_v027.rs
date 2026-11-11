//! The home that release v0.2.7 left on a computer.
//!
//! `tests/fixtures/upgrade/v0.2.7` holds the files of that release, and the
//! board names two panel kinds that this release removed. The saved session must
//! still open, the panels of those kinds must be gone, and the count of them must
//! arrive with the session.

use std::fs;
use std::path::{Path, PathBuf};

use horizon_core::{Config, HorizonHome, PanelKind, SessionOpenDisposition, SessionStore, StartupDecision};

/// The session and the profile that the release wrote in the fixture.
const SESSION_ID: &str = "9272f72e-d124-4012-9ca2-761544678156";
/// The settings path that the release recorded. It names the profile of the
/// saved session, so the store of this test is that profile.
const CONFIG_PATH: &str = "/home/user/project/.horizon/config.yaml";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/upgrade/v0.2.7")
        .join(name)
}

fn store_with_the_saved_session() -> (tempfile::TempDir, SessionStore) {
    let root = tempfile::tempdir().expect("a temporary home");
    let home = HorizonHome::from_root(root.path().join(".horizon"));
    let session_dir = home.sessions_dir().join(SESSION_ID);
    fs::create_dir_all(&session_dir).expect("the folder of the saved session");
    for name in ["meta.yaml", "runtime.yaml"] {
        fs::copy(fixture(name), session_dir.join(name)).expect("a file of the saved session");
    }
    fs::copy(fixture("index.yaml"), home.session_index_path()).expect("the session index");
    let store = SessionStore::new(home, PathBuf::from(CONFIG_PATH));
    (root, store)
}

#[test]
fn the_saved_session_opens_without_the_panels_that_this_release_removed() {
    let (_root, store) = store_with_the_saved_session();

    let session = store
        .resume_session(SESSION_ID)
        .expect("the session of v0.2.7 must load");

    let workspaces: Vec<(&str, Vec<PanelKind>)> = session
        .runtime_state
        .workspaces
        .iter()
        .map(|workspace| {
            (
                workspace.name.as_str(),
                workspace.panels.iter().map(|panel| panel.kind).collect(),
            )
        })
        .collect();
    assert_eq!(
        workspaces,
        vec![
            ("Workspace 1", vec![PanelKind::Shell]),
            ("Workspace 2", vec![PanelKind::Editor]),
            ("Workspace 3", vec![]),
        ],
        "the other panels stay in their workspaces, and the workspace of the removed panels stays empty"
    );

    let shell = &session.runtime_state.workspaces[0].panels[0];
    assert_eq!(
        shell.cwd.as_deref().map(Path::new),
        Some(Path::new("/home/user/project")),
        "the shell panel keeps the folder that the release saved"
    );

    assert_eq!(session.retired_panels.total(), 2);
    assert_eq!(session.retired_panels.count_of("usage"), 1);
    assert_eq!(session.retired_panels.count_of("git_changes"), 1);
    assert_eq!(
        session.retired_panels.kinds(),
        vec![("git_changes", 1), ("usage", 1)],
        "a notice names one count for each removed kind"
    );
}

#[test]
fn the_startup_of_that_profile_resumes_its_session() {
    let (_root, store) = store_with_the_saved_session();

    let decision = store
        .prepare_startup(&Config::default())
        .expect("the startup of the profile of v0.2.7");

    match decision {
        StartupDecision::Open { disposition, session } => {
            assert_eq!(disposition, SessionOpenDisposition::Resume);
            assert_eq!(session.session_id, SESSION_ID);
            assert_eq!(
                session.retired_panels.total(),
                2,
                "the startup carries the count of the panels that were left out"
            );
        }
        other => panic!("the saved session must open, and the decision was {other:?}"),
    }
}
