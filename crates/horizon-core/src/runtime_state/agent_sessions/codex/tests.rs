use std::collections::HashSet;

use rusqlite::{Connection, params};

use super::{
    load_sessions_from_path, reset_rollout_metadata_read_count, rollout_metadata_read_count, rollout_root_session_id,
    user_home_dir_from_env,
};

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
fn malformed_rows_do_not_hide_other_sessions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let unused_rollout = temp.path().join("unused.jsonl");
    insert_thread(&connection, "root", &unused_rollout, "cli", "/repo", 100, false);
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "malformed",
                unused_rollout.display().to_string(),
                vec![0xff_u8],
                "Malformed",
                "/repo",
                200_i64,
                false
            ],
        )
        .expect("insert malformed thread");
    drop(connection);

    let loaded = load_sessions_from_path(&sqlite_path, &HashSet::new()).expect("load Codex sessions");

    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].session_id, "root");
}

#[test]
fn exact_binding_validation_rejects_a_malformed_row() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let unused_rollout = temp.path().join("unused.jsonl");
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "malformed",
                unused_rollout.display().to_string(),
                vec![0xff_u8],
                "Malformed",
                "/repo",
                200_i64,
                false
            ],
        )
        .expect("insert malformed thread");
    drop(connection);

    let loaded = load_sessions_from_path(&sqlite_path, &HashSet::from(["malformed".to_string()]));

    assert!(loaded.is_err());
}

#[test]
fn malformed_json_sources_do_not_become_interactive_roots() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let unused_rollout = temp.path().join("unused.jsonl");
    insert_thread(&connection, "root", &unused_rollout, "cli", "/repo", 100, false);
    insert_thread(
        &connection,
        "malformed",
        &unused_rollout,
        r#"{"subagent": "#,
        "/repo",
        200,
        false,
    );
    drop(connection);

    let loaded = load_sessions_from_path(&sqlite_path, &HashSet::new()).expect("load Codex sessions");

    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].session_id, "root");
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
fn child_alias_requires_a_known_shared_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"root"}}
"#,
    )
    .expect("write child rollout");
    insert_thread(&connection, "root", &child_rollout, "cli", "", 100, false);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "",
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
fn source_chain_is_used_when_the_rollout_root_is_rejected() {
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
    insert_thread(&connection, "root", &child_rollout, "cli", "/repo", 100, false);
    insert_thread(&connection, "archived-root", &child_rollout, "cli", "/repo", 90, true);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"root"}}}"#,
        "/repo",
        200,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child"].session_id, "root");
}

#[test]
fn rollout_metadata_root_takes_precedence_over_the_source_parent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"metadata-root"}}
"#,
    )
    .expect("write child rollout");
    insert_thread(&connection, "metadata-root", &child_rollout, "cli", "/repo", 100, false);
    insert_thread(&connection, "source-root", &child_rollout, "cli", "/repo", 90, false);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"source-root"}}}"#,
        "/repo",
        200,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child"].session_id, "metadata-root");
}

#[test]
fn source_parent_resolves_after_a_long_rejected_metadata_chain() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"metadata-0"}}
"#,
    )
    .expect("write child rollout");
    insert_thread(&connection, "source-root", &child_rollout, "cli", "/repo", 1, false);
    insert_thread(&connection, "archived-root", &child_rollout, "cli", "/repo", 2, true);
    for index in (0_i64..64).rev() {
        let id = format!("metadata-{index}");
        let parent_id = if index == 63 {
            "archived-root".to_string()
        } else {
            format!("metadata-{}", index + 1)
        };
        let rollout = temp.path().join(format!("{id}.jsonl"));
        std::fs::write(
            &rollout,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","session_id":"{parent_id}"}}}}
"#
            ),
        )
        .expect("write metadata rollout");
        insert_thread(
            &connection,
            &id,
            &rollout,
            r#"{"subagent":{"other":"guardian"}}"#,
            "/repo",
            index + 3,
            false,
        );
    }
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"source-root"}}}"#,
        "/repo",
        100,
        false,
    );
    drop(connection);
    reset_rollout_metadata_read_count();

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child"].session_id, "source-root");
    assert_eq!(rollout_metadata_read_count(), 65);
}

#[test]
fn intermediate_source_parent_resolves_after_a_long_rejected_metadata_branch() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    let intermediate_rollout = temp.path().join("intermediate.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"intermediate"}}
"#,
    )
    .expect("write child rollout");
    std::fs::write(
        &intermediate_rollout,
        r#"{"type":"session_meta","payload":{"id":"intermediate","session_id":"metadata-0"}}
"#,
    )
    .expect("write intermediate rollout");
    insert_thread(&connection, "source-root", &child_rollout, "cli", "/repo", 1, false);
    insert_thread(&connection, "archived-root", &child_rollout, "cli", "/repo", 2, true);
    for index in (0_i64..62).rev() {
        let id = format!("metadata-{index}");
        let parent_id = if index == 61 {
            "archived-root".to_string()
        } else {
            format!("metadata-{}", index + 1)
        };
        let rollout = temp.path().join(format!("{id}.jsonl"));
        std::fs::write(
            &rollout,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","session_id":"{parent_id}"}}}}
"#
            ),
        )
        .expect("write metadata rollout");
        insert_thread(
            &connection,
            &id,
            &rollout,
            r#"{"subagent":{"other":"guardian"}}"#,
            "/repo",
            index + 3,
            false,
        );
    }
    insert_thread(
        &connection,
        "intermediate",
        &intermediate_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"source-root"}}}"#,
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
        101,
        false,
    );
    drop(connection);
    reset_rollout_metadata_read_count();

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child"].session_id, "source-root");
    assert_eq!(rollout_metadata_read_count(), 64);
}

#[test]
fn metadata_parent_chains_can_cross_an_intermediate_child() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    let intermediate_rollout = temp.path().join("intermediate.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","parent_thread_id":"intermediate"}}
"#,
    )
    .expect("write child rollout");
    std::fs::write(
        &intermediate_rollout,
        r#"{"type":"session_meta","payload":{"id":"intermediate","parent_thread_id":"root"}}
"#,
    )
    .expect("write intermediate rollout");
    insert_thread(&connection, "root", &child_rollout, "cli", "/repo", 100, false);
    insert_thread(
        &connection,
        "intermediate",
        &intermediate_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        200,
        false,
    );
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":"review"}"#,
        "/repo",
        300,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child"].session_id, "root");
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
fn metadata_cycles_do_not_block_a_valid_source_parent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    let first_cycle_rollout = temp.path().join("cycle-a.jsonl");
    let second_cycle_rollout = temp.path().join("cycle-b.jsonl");
    std::fs::write(
        &child_rollout,
        r#"{"type":"session_meta","payload":{"id":"child","session_id":"cycle-a"}}
"#,
    )
    .expect("write child rollout");
    std::fs::write(
        &first_cycle_rollout,
        r#"{"type":"session_meta","payload":{"id":"cycle-a","session_id":"cycle-b"}}
"#,
    )
    .expect("write first cycle rollout");
    std::fs::write(
        &second_cycle_rollout,
        r#"{"type":"session_meta","payload":{"id":"cycle-b","session_id":"cycle-a"}}
"#,
    )
    .expect("write second cycle rollout");
    insert_thread(&connection, "root", &child_rollout, "cli", "/repo", 1, false);
    insert_thread(
        &connection,
        "cycle-a",
        &first_cycle_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        2,
        false,
    );
    insert_thread(
        &connection,
        "cycle-b",
        &second_cycle_rollout,
        r#"{"subagent":{"other":"guardian"}}"#,
        "/repo",
        3,
        false,
    );
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"root"}}}"#,
        "/repo",
        4,
        false,
    );
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child"].session_id, "root");
}

#[test]
fn source_chain_stops_after_the_parent_traversal_limit() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let missing_rollout = temp.path().join("missing.jsonl");
    insert_thread(&connection, "root", &missing_rollout, "cli", "/repo", 1, false);
    for index in (0_i64..=64).rev() {
        let id = format!("child-{index}");
        let parent_id = if index == 64 {
            "root".to_string()
        } else {
            format!("child-{}", index + 1)
        };
        let source = format!(r#"{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent_id}"}}}}}}"#);
        insert_thread(&connection, &id, &missing_rollout, &source, "/repo", index + 2, false);
    }
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child-0".to_string()])).expect("load Codex sessions");

    assert!(!loaded.root_aliases.contains_key("child-0"));
    assert!(loaded.child_binding_ids.contains("child-0"));
}

#[test]
fn source_chain_resolves_at_the_parent_traversal_limit() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let missing_rollout = temp.path().join("missing.jsonl");
    insert_thread(&connection, "root", &missing_rollout, "cli", "/repo", 1, false);
    for index in (0_i64..64).rev() {
        let id = format!("child-{index}");
        let parent_id = if index == 63 {
            "root".to_string()
        } else {
            format!("child-{}", index + 1)
        };
        let source = format!(r#"{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent_id}"}}}}}}"#);
        insert_thread(&connection, &id, &missing_rollout, &source, "/repo", index + 2, false);
    }
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child-0".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.root_aliases["child-0"].session_id, "root");
}

#[test]
fn rejected_same_parent_fallbacks_read_each_rollout_once() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let root_rollout = temp.path().join("root.jsonl");
    std::fs::write(&root_rollout, "{}\n").expect("write root rollout");
    insert_thread(&connection, "root", &root_rollout, "cli", "/repo", 1, true);
    for index in (0_i64..10).rev() {
        let id = format!("child-{index}");
        let parent_id = if index == 9 {
            "root".to_string()
        } else {
            format!("child-{}", index + 1)
        };
        let rollout = temp.path().join(format!("{id}.jsonl"));
        std::fs::write(
            &rollout,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","session_id":"{parent_id}"}}}}
"#
            ),
        )
        .expect("write child rollout");
        let source = format!(r#"{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent_id}"}}}}}}"#);
        insert_thread(&connection, &id, &rollout, &source, "/repo", index + 2, false);
    }
    drop(connection);
    reset_rollout_metadata_read_count();

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["child-0".to_string()])).expect("load Codex sessions");

    assert!(!loaded.root_aliases.contains_key("child-0"));
    assert!(loaded.child_binding_ids.contains("child-0"));
    assert_eq!(rollout_metadata_read_count(), 10);
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
fn rollout_metadata_can_follow_more_than_eight_preamble_lines() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    let mut contents = "{\"type\":\"event\"}\n".repeat(9);
    contents.push_str("{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n");
    std::fs::write(&rollout, contents).expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn rollout_metadata_accepts_a_first_line_larger_than_64_kibibytes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    let instructions = "x".repeat(70 * 1024);
    let contents = format!(
        r#"{{"type":"session_meta","payload":{{"instructions":"{instructions}","id":"child","parent_thread_id":"root"}}}}
"#
    );
    assert!(contents.len() > 64 * 1024);
    std::fs::write(&rollout, contents).expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn rollout_metadata_reads_the_parent_thread_id_used_by_review_children() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &rollout,
        r#"{"type":"session_meta","payload":{"id":"child","parent_thread_id":"root"}}
"#,
    )
    .expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn rollout_session_id_takes_precedence_over_parent_thread_id() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &rollout,
        r#"{"type":"session_meta","payload":{"id":"child","parent_thread_id":"root","session_id":"legacy-root"}}
"#,
    )
    .expect("write rollout");

    assert_eq!(
        rollout_root_session_id(&rollout, "child").as_deref(),
        Some("legacy-root")
    );
}

#[test]
fn user_home_falls_back_to_the_windows_profile() {
    let profile = std::path::PathBuf::from(r"C:\Users\tester");

    assert_eq!(user_home_dir_from_env(None, Some(profile.clone())), Some(profile));
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
