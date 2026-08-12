use std::collections::HashSet;

use rusqlite::params;

use super::{create_threads_db, insert_thread, load_sessions_from_path};

#[test]
fn catalog_accepts_unknown_and_json_quoted_interactive_sources() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let unused_rollout = temp.path().join("unused.jsonl");
    for (index, source) in ["app", "tui", "mcp", "cli\n", r#""cli""#, r#"{"app":"desktop"}"#]
        .into_iter()
        .enumerate()
    {
        insert_thread(
            &connection,
            &format!("root-{index}"),
            &unused_rollout,
            source,
            "/repo",
            i64::try_from(index).expect("small index"),
            false,
        );
    }
    drop(connection);

    let loaded = load_sessions_from_path(&sqlite_path, &HashSet::new()).expect("load Codex sessions");

    assert_eq!(loaded.sessions.len(), 6);
}

#[test]
fn optional_text_columns_do_not_abort_catalog_or_exact_validation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "root",
                vec![0xff_u8],
                "cli",
                vec![0xfe_u8],
                vec![0xfd_u8],
                100_i64,
                false
            ],
        )
        .expect("insert thread with malformed optional text");
    drop(connection);

    let loaded =
        load_sessions_from_path(&sqlite_path, &HashSet::from(["root".to_string()])).expect("load Codex sessions");

    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions[0].session_id, "root");
    assert_eq!(loaded.sessions[0].label, None);
    assert_eq!(loaded.sessions[0].cwd, None);
    assert!(loaded.verified_binding_ids.contains("root"));
}
