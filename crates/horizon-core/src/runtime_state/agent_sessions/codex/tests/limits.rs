use std::collections::HashSet;

use super::super::{CodexStore, RootResolution, RootTraversal};
use super::{create_threads_db, insert_thread, load_sessions_from_path};

fn insert_parent_chain(connection: &rusqlite::Connection, rollout: &std::path::Path, prefix: &str) {
    for index in (0_i64..64).rev() {
        let id = format!("{prefix}-{index}");
        let parent_id = if index == 63 {
            format!("{prefix}-beyond-budget")
        } else {
            format!("{prefix}-{}", index + 1)
        };
        let source = format!(r#"{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent_id}"}}}}}}"#);
        insert_thread(connection, &id, rollout, &source, "/repo", index + 1, false);
    }
}

#[test]
fn an_exhausted_binding_does_not_starve_a_later_valid_child() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let missing_rollout = temp.path().join("missing.jsonl");
    let exhausting_rollout = temp.path().join("a-exhausting-child.jsonl");
    let second_exhausting_rollout = temp.path().join("b-exhausting-child.jsonl");
    let valid_rollout = temp.path().join("z-valid-child.jsonl");
    std::fs::write(
        &exhausting_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"a-exhausting-child\",\"session_id\":\"metadata-0\"}}\n",
    )
    .expect("write exhausting rollout");
    std::fs::write(
        &second_exhausting_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"b-exhausting-child\",\"session_id\":\"metadata-0\"}}\n",
    )
    .expect("write second exhausting rollout");
    std::fs::write(
        &valid_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"z-valid-child\",\"session_id\":\"z-root\"}}\n",
    )
    .expect("write valid rollout");
    insert_parent_chain(&connection, &missing_rollout, "metadata");
    insert_parent_chain(&connection, &missing_rollout, "source");
    insert_thread(&connection, "z-root", &missing_rollout, "cli", "/repo", 1, false);
    insert_thread(
        &connection,
        "a-exhausting-child",
        &exhausting_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"source-0"}}}"#,
        "/repo",
        2,
        false,
    );
    insert_thread(
        &connection,
        "b-exhausting-child",
        &second_exhausting_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"source-0"}}}"#,
        "/repo",
        2,
        false,
    );
    insert_thread(
        &connection,
        "z-valid-child",
        &valid_rollout,
        r#"{"subagent":"review"}"#,
        "/repo",
        3,
        false,
    );
    drop(connection);

    let loaded = load_sessions_from_path(
        &sqlite_path,
        &HashSet::from([
            "a-exhausting-child".to_string(),
            "b-exhausting-child".to_string(),
            "z-valid-child".to_string(),
        ]),
    )
    .expect("validate bindings");

    assert!(loaded.unavailable_binding_ids.contains("a-exhausting-child"));
    assert!(loaded.unavailable_binding_ids.contains("b-exhausting-child"));
    assert_eq!(loaded.root_aliases["z-valid-child"].session_id, "z-root");
}

#[test]
fn exhausted_candidate_skips_thread_lookup() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let missing_rollout = temp.path().join("missing.jsonl");
    insert_thread(&connection, "child", &missing_rollout, "cli", "/repo", 1, false);

    let mut store = CodexStore::new(connection);
    let mut traversal = RootTraversal::new(&mut store);
    traversal.remaining_binding_steps = 0;

    let resolution = traversal
        .resolve_candidate("child", "/repo")
        .expect("resolve candidate");

    assert!(matches!(resolution, RootResolution::BudgetExhausted));
    assert!(!traversal.store.threads.contains_key("child"));
}

#[test]
fn last_step_skips_rollout_and_parent_reads() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    let child_rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &child_rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n",
    )
    .expect("write child rollout");
    insert_thread(&connection, "root", &child_rollout, "cli", "/repo", 1, false);
    insert_thread(
        &connection,
        "child",
        &child_rollout,
        r#"{"subagent":{"thread_spawn":{"parent_thread_id":"root"}}}"#,
        "/repo",
        2,
        false,
    );

    let mut store = CodexStore::new(connection);
    let mut traversal = RootTraversal::new(&mut store);
    traversal.remaining_binding_steps = 1;

    let resolution = traversal
        .resolve_candidate("child", "/repo")
        .expect("resolve candidate");

    assert!(matches!(resolution, RootResolution::BudgetExhausted));
    assert!(!traversal.rollout_metadata.entries.contains_key(&child_rollout));
    assert!(!traversal.store.threads.contains_key("root"));
}
