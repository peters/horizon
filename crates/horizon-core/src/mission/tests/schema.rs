use crate::mission::{TaskId, TaskSize, ThinkingLevel};

#[test]
fn thinking_level_serde_is_lowercase() {
    assert_eq!(
        serde_json::to_string(&ThinkingLevel::Xhigh).expect("serde"),
        "\"xhigh\""
    );
    let parsed: ThinkingLevel = serde_json::from_str("\"medium\"").expect("serde");
    assert_eq!(parsed, ThinkingLevel::Medium);
    let err = serde_json::from_str::<ThinkingLevel>("\"turbo\"").expect_err("unknown level");
    assert!(err.to_string().contains("turbo"));
}

#[test]
fn task_size_serde_is_snake_case() {
    assert_eq!(serde_json::to_string(&TaskSize::Small).expect("serde"), "\"small\"");
    let parsed: TaskSize = serde_json::from_str("\"large\"").expect("serde");
    assert_eq!(parsed, TaskSize::Large);
}

#[test]
fn plan_task_selected_defaults_to_true_when_omitted() {
    let raw = r#"{
        "id": "T1",
        "title": "core",
        "brief": "build it",
        "size": "small",
        "deps": [],
        "worktree": "m01-t1",
        "model": "worker",
        "effort": "high",
        "version": 1,
        "status": "queued"
    }"#;
    let task: crate::mission::PlanTask = serde_json::from_str(raw).expect("serde");
    assert!(task.selected);
}

#[test]
fn replaced_status_carries_the_replacement_id() {
    let raw = r#"{
        "id": "T4",
        "title": "ui",
        "brief": "build it",
        "size": "medium",
        "deps": ["T2"],
        "worktree": "m01-t4",
        "model": "worker",
        "effort": "high",
        "selected": true,
        "version": 1,
        "status": { "replaced": { "by": "T4b" } }
    }"#;
    let task: crate::mission::PlanTask = serde_json::from_str(raw).expect("serde");
    assert!(matches!(task.status, crate::mission::TaskStatus::Replaced { .. }));
    let round = serde_json::to_string(&task).expect("serde");
    let reparsed: crate::mission::PlanTask = serde_json::from_str(&round).expect("serde");
    assert_eq!(reparsed, task);
}

#[test]
fn plan_file_shape_round_trips_verbatim_fields() {
    let task = super::task("T1", &[]);
    let raw = serde_json::to_string(&crate::mission::MissionPlan {
        schema: crate::mission::MISSION_PLAN_SCHEMA,
        version: 2,
        tasks: vec![task],
    })
    .expect("serde");
    let back: crate::mission::MissionPlan = serde_json::from_str(&raw).expect("serde");
    assert_eq!(back.version, 2);
    assert_eq!(back.tasks.len(), 1);
}

#[test]
fn task_id_deserialize_enforces_format() {
    let parsed: TaskId = serde_json::from_str("\"T1\"").expect("valid id");
    assert_eq!(parsed.as_str(), "T1");
    assert!(serde_json::from_str::<TaskId>("\"1bad\"").is_err());
}
