use std::collections::HashSet;

use rusqlite::{Connection, params};

use super::{load_sessions_from_path, rollout_root_session_id};

fn create_threads_db(path: &std::path::Path) -> Connection {
    let connection = Connection::open(path).expect("create Codex state database");
    connection
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                source TEXT NOT NULL,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create threads table");
    connection
}

fn insert_thread(
    connection: &Connection,
    id: &str,
    rollout_path: &std::path::Path,
    source: &str,
    cwd: &str,
    updated_at: i64,
    archived: bool,
) {
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                rollout_path.display().to_string(),
                source,
                format!("Title {id}"),
                cwd,
                updated_at,
                archived
            ],
        )
        .expect("insert thread");
}

#[test]
fn catalog_excludes_parent_controlled_threads() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let unused_rollout = temp.path().join("unused.jsonl");
    insert_thread(&connection, "root-cli", &unused_rollout, "cli", "/repo", 100, false);
    insert_thread(
        &connection,
        "root-vscode",
        &unused_rollout,
        "vscode",
        "/repo",
        90,
        false,
    );
    insert_thread(&connection, "root-exec", &unused_rollout, "exec", "/other", 80, false);
    insert_thread(
        &connection,
        "child-spawn",
        &unused_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"root-cli"}}}"#,
        "/repo",
        300,
        false,
    );
    insert_thread(
        &connection,
        "child-guardian",
        &unused_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        290,
        false,
    );
    insert_thread(
        &connection,
        "child-review",
        &unused_rollout,
        r#"{"subagent":"review"}"#,
        "/repo",
        280,
        false,
    );
    insert_thread(&connection, "archived-root", &unused_rollout, "cli", "/repo", 400, true);
    drop(connection);

    let loaded = load_sessions_from_path(&sqlite_path, &HashSet::new()).expect("load Codex sessions");
    let ids: Vec<_> = loaded
        .sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect();

    assert_eq!(ids, ["root-cli", "root-vscode", "root-exec"]);
    assert!(loaded.root_aliases.is_empty());
    assert!(loaded.child_binding_ids.is_empty());
}

#[test]
fn persisted_child_bindings_resolve_to_verified_root_sessions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let root_rollout = temp.path().join("root.jsonl");
    let child_rollout = temp.path().join("child.jsonl");
    let guardian_rollout = temp.path().join("guardian.jsonl");
    let review_rollout = temp.path().join("review.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"root"}}
"#,
    )
    .expect("write child rollout");
    std::fs::write(
        &guardian_rollout,
        r#"{"type":"session_meta","payload":{"id":"guardian","session_id":"root"}}
"#,
    )
    .expect("write guardian rollout");
    std::fs::write(
        &review_rollout,
        r#"{"type":"session_meta","payload":{"id":"review","session_id":"root"}}
"#,
    )
    .expect("write review rollout");
    insert_thread(&connection, "root", &root_rollout, "cli", "/repo", 100, false);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"root"}}}"#,
        "/repo",
        300,
        false,
    );
    insert_thread(
        &connection,
        "guardian",
        &guardian_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        290,
        false,
    );
    insert_thread(
        &connection,
        "source-fallback",
        &temp.path().join("missing.jsonl"),
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"child"}}}"#,
        "/repo",
        280,
        false,
    );
    insert_thread(
        &connection,
        "review",
        &review_rollout,
        r#"{"subagent":"review"}"#,
        "/repo",
        270,
        false,
    );
    drop(connection);
    let binding_ids = HashSet::from([
        "child".to_string(),
        "guardian".to_string(),
        "source-fallback".to_string(),
        "review".to_string(),
    ]);

    let loaded = load_sessions_from_path(&sqlite_path, &binding_ids).expect("load Codex sessions");

    for child_id in ["child", "guardian", "source-fallback", "review"] {
        assert_eq!(loaded.root_aliases[child_id].session_id, "root");
        assert!(loaded.child_binding_ids.contains(child_id));
    }
}

#[test]
fn child_alias_rejects_a_root_from_another_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"other-root"}}
"#,
    )
    .expect("write child rollout");
    insert_thread(&connection, "other-root", &child_rollout, "cli", "/other", 100, false);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        200,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert!(!loaded.root_aliases.contains_key("child"));
    assert!(loaded.child_binding_ids.contains("child"));
}

#[test]
fn child_alias_rejects_mismatched_rollout_metadata() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"different-child","session_id":"unrelated-root"}}
"#,
    )
    .expect("write child rollout");
    insert_thread(
        &connection,
        "unrelated-root",
        &child_rollout,
        "cli",
        "/repo",
        100,
        false,
    );
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        200,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert!(!loaded.root_aliases.contains_key("child"));
    assert!(loaded.child_binding_ids.contains("child"));
}

#[test]
fn archived_child_bindings_remain_classified_but_archived_roots_are_not_aliases() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"archived-root"}}
"#,
    )
    .expect("write child rollout");
    insert_thread(&connection, "archived-root", &child_rollout, "cli", "/repo", 100, true);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"archived-root"}}}"#,
        "/repo",
        200,
        true,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert!(loaded.sessions.is_empty());
    assert!(!loaded.root_aliases.contains_key("child"));
    assert!(loaded.child_binding_ids.contains("child"));
}

#[test]
fn source_chain_cycles_do_not_produce_aliases() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let missing_rollout = temp.path().join("missing.jsonl");
    insert_thread(
        &connection,
        "child-a",
        &missing_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"child-b"}}}"#,
        "/repo",
        200,
        false,
    );
    insert_thread(
        &connection,
        "child-b",
        &missing_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"child-a"}}}"#,
        "/repo",
        100,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child-a".to_string()])).expect("load Codex sessions");

    assert!(!loaded.root_aliases.contains_key("child-a"));
    assert!(loaded.child_binding_ids.contains("child-a"));
}

#[test]
fn rollout_metadata_rejects_non_regular_paths() {
    let temp = tempfile::tempdir().expect("temp dir");

    assert!(rollout_root_session_id(temp.path(), "child").is_none());
}

#[test]
fn rollout_metadata_ignores_a_truncated_unicode_tail() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    let mut contents =
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n".to_string();
    contents.push_str(&"ø".repeat(40_000));
    std::fs::write(&rollout, contents).expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn exact_binding_validation_rejects_missing_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    drop(create_threads_db(&sqlite_path));

    let Err(error) = load_sessions_from_path(&sqlite_path, &HashSet::from(["missing".to_string()])) else {
        panic!("missing exact binding must fail validation");
    };

    assert!(error.to_string().contains("missing"));
}
