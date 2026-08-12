use std::collections::HashSet;

use super::{create_threads_db, insert_thread, load_sessions_from_path, load_sessions_from_path_with_catalog};

#[test]
fn catalog_bounds_large_session_titles() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let unused_rollout = temp.path().join("unused.jsonl");
    insert_thread(&connection, "root", &unused_rollout, "cli", "/repo", 100, false);
    let large_title = "ø".repeat(8 * 1024);
    connection
        .execute("UPDATE threads SET title = ?1 WHERE id = 'root'", [&large_title])
        .expect("set large title");
    drop(connection);

    let loaded = load_sessions_from_path(&sqlite_path, &HashSet::new()).expect("load Codex sessions");
    let label = loaded.sessions[0].label.as_deref().expect("bounded title");

    assert_eq!(label.chars().count(), 256);
    assert_eq!(label, "ø".repeat(256));
}

#[test]
fn targeted_alias_bounds_large_root_titles() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n",
    )
    .expect("write child rollout");
    insert_thread(&connection, "root", &child_rollout, "cli", "/repo", 100, false);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":"review"}"#,
        "/repo",
        200,
        false,
    );
    let large_title = "ø".repeat(8 * 1024);
    connection
        .execute("UPDATE threads SET title = ?1 WHERE id = 'root'", [&large_title])
        .expect("set large root title");
    drop(connection);

    let loaded = load_sessions_from_path_with_catalog(&sqlite_path, &HashSet::from(["child".to_string()]), false)
        .expect("validate exact child");
    let label = loaded.root_aliases["child"].label.as_deref().expect("bounded title");

    assert_eq!(label.chars().count(), 256);
    assert_eq!(label, "ø".repeat(256));
}
