use crate::mission::{MissionMeta, PlannerKind, TaskId, TaskStatus};

use super::{plan, task};

#[test]
fn valid_plan_passes() {
    let p = plan(vec![
        task("T1", &[]),
        task("T2", &["T1"]),
        task("T3", &["T1"]),
        task("T4", &["T2"]),
    ]);
    assert!(p.validate().is_ok());
}

#[test]
fn duplicate_ids_are_rejected() {
    let mut p = plan(vec![task("T1", &[]), task("T1", &[])]);
    p.version = 1;
    let err = p.validate().expect_err("dup ids");
    assert!(err.to_string().contains("duplicate"), "{err}");
}

#[test]
fn unknown_deps_are_rejected() {
    let p = plan(vec![task("T1", &["T9"])]);
    let err = p.validate().expect_err("unknown dep");
    assert!(err.to_string().contains("T9"), "{err}");
}

#[test]
fn self_dependency_is_rejected() {
    let p = plan(vec![task("T1", &["T1"])]);
    let err = p.validate().expect_err("self dep");
    assert!(err.to_string().contains("itself"), "{err}");
}

#[test]
fn direct_cycle_is_rejected() {
    let p = plan(vec![task("T1", &["T2"]), task("T2", &["T1"])]);
    let err = p.validate().expect_err("cycle");
    assert!(err.to_string().contains("cycle"), "{err}");
}

#[test]
fn transitive_cycle_is_rejected() {
    let p = plan(vec![task("T1", &["T3"]), task("T2", &["T1"]), task("T3", &["T2"])]);
    let err = p.validate().expect_err("cycle");
    assert!(err.to_string().contains("cycle"), "{err}");
}

#[test]
fn diamond_dependencies_are_not_a_cycle() {
    let p = plan(vec![
        task("T1", &[]),
        task("T2", &["T1"]),
        task("T3", &["T1"]),
        task("T4", &["T2", "T3"]),
    ]);
    assert!(p.validate().is_ok());
}

#[test]
fn empty_brief_is_rejected() {
    let mut t = task("T1", &[]);
    t.brief = "   ".into();
    let err = plan(vec![t]).validate().expect_err("empty brief");
    assert!(err.to_string().contains("brief"), "{err}");
}

#[test]
fn empty_title_is_rejected() {
    let mut t = task("T1", &[]);
    t.title = String::new();
    let err = plan(vec![t]).validate().expect_err("empty title");
    assert!(err.to_string().contains("title"), "{err}");
}

#[test]
fn bad_worktree_names_are_rejected() {
    for name in ["-leading", "Upper", "has space", "dot.dot"] {
        let mut t = task("T1", &[]);
        t.worktree = name.into();
        let err = plan(vec![t]).validate().expect_err("bad worktree");
        assert!(err.to_string().contains(name), "{err} for `{name}`");
    }
}

#[test]
fn replaced_by_unknown_task_is_rejected() {
    let mut t = task("T4", &[]);
    t.status = TaskStatus::Replaced {
        by: TaskId::new("T4b").expect("valid"),
    };
    let err = plan(vec![t]).validate().expect_err("replaced by missing");
    assert!(err.to_string().contains("T4b"), "{err}");
}

#[test]
fn replaced_by_self_is_rejected() {
    let mut t = task("T4", &[]);
    t.status = TaskStatus::Replaced {
        by: TaskId::new("T4").expect("valid"),
    };
    let err = plan(vec![t]).validate().expect_err("self replace");
    assert!(err.to_string().contains("itself"), "{err}");
}

#[test]
fn replaced_by_existing_task_passes() {
    let mut old = task("T4", &[]);
    old.status = TaskStatus::Replaced {
        by: TaskId::new("T4b").expect("valid"),
    };
    let new = task("T4b", &[]);
    let p = plan(vec![old, new]);
    assert!(p.validate().is_ok());
}

#[test]
fn version_below_first_is_rejected() {
    let mut p = plan(vec![task("T1", &[])]);
    p.version = 0;
    let err = p.validate().expect_err("version 0");
    assert!(err.to_string().contains("version"), "{err}");
}

#[test]
fn task_id_format_is_enforced() {
    let bad: Vec<String> = vec!["1".into(), "1abc".into(), "a".repeat(9), "ab-c".into(), String::new()];
    for bad in &bad {
        assert!(TaskId::new(bad).is_err(), "should reject `{bad}`");
    }
    for good in ["T1", "T3a", "T4b", "ab12cd34"] {
        assert!(TaskId::new(good).is_ok(), "should accept `{good}`");
    }
}

#[test]
fn mission_id_must_be_a_safe_slug() {
    let cases: Vec<(String, bool)> = vec![
        ("mission-01".into(), true),
        ("m1".into(), true),
        ("Mission-01".into(), false),
        ("-lead".into(), false),
        ("has_underscore".into(), false),
        ("x".repeat(40), false),
        (String::new(), false),
    ];
    for (id, ok) in cases {
        let meta = MissionMeta {
            id: id.clone(),
            goal: "goal".into(),
            planner: PlannerKind::Pi,
            status: crate::mission::MissionStatus::Planning,
        };
        if ok {
            assert!(meta.validate_id().is_ok(), "`{id}` should pass");
        } else {
            assert!(meta.validate_id().is_err(), "`{id}` should fail");
        }
    }
}
