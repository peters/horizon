use std::path::Path;

use super::super::RolloutMetadataCache;

fn rollout_root_session_id(path: &Path, expected_child_id: &str) -> Option<String> {
    RolloutMetadataCache::default().parent_id(path, expected_child_id)
}

#[test]
fn rejects_non_regular_paths() {
    let temp = tempfile::tempdir().expect("temp dir");

    assert!(rollout_root_session_id(temp.path(), "child").is_none());
}

#[test]
fn ignores_a_truncated_unicode_tail() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    let mut contents =
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n".to_string();
    contents.push_str(&"ø".repeat(40_000));
    std::fs::write(&rollout, contents).expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn follows_more_than_eight_preamble_lines() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    let mut contents = "{\"type\":\"event\"}\n".repeat(9);
    contents.push_str("{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n");
    std::fs::write(&rollout, contents).expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn accepts_a_first_line_larger_than_64_kibibytes() {
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
fn reads_the_parent_thread_id_used_by_review_children() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"parent_thread_id\":\"root\"}}\n",
    )
    .expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}

#[test]
fn session_id_takes_precedence_over_parent_thread_id() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"parent_thread_id\":\"root\",\"session_id\":\"legacy-root\"}}\n",
    )
    .expect("write rollout");

    assert_eq!(
        rollout_root_session_id(&rollout, "child").as_deref(),
        Some("legacy-root")
    );
}

#[test]
fn cache_reads_each_path_once() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"root\"}}\n",
    )
    .expect("write rollout");
    let mut cache = RolloutMetadataCache::default();

    assert_eq!(cache.parent_id(&rollout, "child").as_deref(), Some("root"));
    std::fs::remove_file(&rollout).expect("remove rollout after first read");
    assert_eq!(cache.parent_id(&rollout, "child").as_deref(), Some("root"));
    assert_eq!(cache.entries.len(), 1);
}

#[test]
fn ignores_a_childs_self_session_id() {
    let temp = tempfile::tempdir().expect("temp dir");
    let rollout = temp.path().join("child.jsonl");
    std::fs::write(
        &rollout,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"session_id\":\"child\",\"parent_thread_id\":\"root\"}}\n",
    )
    .expect("write rollout");

    assert_eq!(rollout_root_session_id(&rollout, "child").as_deref(), Some("root"));
}
