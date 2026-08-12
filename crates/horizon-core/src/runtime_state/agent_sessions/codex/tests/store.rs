use std::collections::HashSet;

use rusqlite::{Connection, params};

use super::{create_threads_db, load_sessions_from_path_with_catalog};

#[test]
fn exact_validation_skips_the_active_catalog_query() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = create_threads_db(&sqlite_path);
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params!["root", "/tmp/root.jsonl", "cli", "Root", "/repo", 1_i64, false],
        )
        .expect("insert root");
    connection
        .execute(
            "INSERT INTO threads (id, rollout_path, source, title, cwd, updated_at, archived)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params!["bad", "/tmp/bad.jsonl", "cli", "Bad", "/repo", vec![1_u8], false],
        )
        .expect("insert malformed timestamp");
    drop(connection);

    let loaded = load_sessions_from_path_with_catalog(&sqlite_path, &HashSet::from(["root".to_string()]), false)
        .expect("targeted exact validation");

    assert!(loaded.sessions.is_empty());
    assert!(loaded.verified_binding_ids.contains("root"));
}

#[test]
fn exact_validation_propagates_targeted_schema_errors() {
    let temp = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp.path().join("state_5.sqlite");
    let connection = Connection::open(&sqlite_path).expect("open sqlite");
    connection
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                title TEXT NOT NULL,
                cwd TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                archived INTEGER NOT NULL
            );",
        )
        .expect("create incompatible thread schema");
    drop(connection);

    let loaded = load_sessions_from_path_with_catalog(&sqlite_path, &HashSet::from(["root".to_string()]), false);

    assert!(matches!(loaded, Err(error) if error.to_string().contains("rollout_path")));
}

#[test]
fn empty_targeted_load_does_not_open_the_store_path() {
    let temp = tempfile::tempdir().expect("temp dir");
    let missing_sqlite_path = temp.path().join("missing").join("state_5.sqlite");

    let loaded = load_sessions_from_path_with_catalog(&missing_sqlite_path, &HashSet::new(), false)
        .expect("empty targeted load");

    assert!(loaded.sessions.is_empty());
    assert!(loaded.verified_binding_ids.is_empty());
    assert!(loaded.unavailable_binding_ids.is_empty());
}
