use crate::mission::{
    FIRST_PLAN_VERSION, MAX_MISSION_TASKS, MISSION_PLAN_SCHEMA, MissionMeta, MissionPlan, MissionStatus, PlanTask,
    PlannerKind, TaskId, TaskModel, TaskSize, TaskStatus, ThinkingLevel, load_mission_file, load_plan_file,
    save_mission_file, save_plan_file,
};

fn task(id: &str, deps: &[&str]) -> PlanTask {
    PlanTask {
        id: TaskId::new(id).expect("valid id in test"),
        title: format!("task {id}"),
        brief: format!("do {id}"),
        size: TaskSize::Medium,
        deps: deps
            .iter()
            .map(|d| TaskId::new(*d).expect("valid dep in test"))
            .collect(),
        worktree: format!("m01-{}", id.to_lowercase()),
        model: TaskModel::Worker,
        effort: ThinkingLevel::High,
        selected: true,
        version: 1,
        status: TaskStatus::Queued,
    }
}

fn plan(tasks: Vec<PlanTask>) -> MissionPlan {
    MissionPlan {
        schema: MISSION_PLAN_SCHEMA,
        version: FIRST_PLAN_VERSION,
        tasks,
    }
}

mod schema;
mod validate;

#[cfg(test)]
mod file_io {
    use super::*;

    #[test]
    fn plan_round_trips_through_json() {
        let dir = tempfile_dir();
        let path = dir.join("plan.json");
        let mut original = plan(vec![task("T1", &[]), task("T2", &["T1"])]);
        original.tasks[1].status = TaskStatus::Replaced {
            by: TaskId::new("T2b").expect("valid"),
        };
        original.tasks.push(task("T2b", &["T1"]));

        save_plan_file(&path, &original).expect("save");
        let loaded = load_plan_file(&path).expect("load");
        assert_eq!(loaded, original);
    }

    #[test]
    fn mission_round_trips_through_json() {
        let dir = tempfile_dir();
        let path = dir.join("mission.json");
        let meta = MissionMeta {
            id: "mission-01".into(),
            goal: "add mission orchestration".into(),
            planner: PlannerKind::CodexCli,
            status: MissionStatus::Planned,
        };
        save_mission_file(&path, &meta).expect("save");
        assert_eq!(load_mission_file(&path).expect("load"), meta);
    }

    #[test]
    fn wrong_schema_is_rejected() {
        let dir = tempfile_dir();
        let path = dir.join("plan.json");
        std::fs::write(&path, r#"{"schema": 99, "version": 1, "tasks": []}"#).expect("write");
        let err = load_plan_file(&path).expect_err("bad schema");
        assert!(err.to_string().contains("schema 99"), "{err}");
    }

    #[test]
    fn invalid_json_is_an_error_not_a_partial_plan() {
        let dir = tempfile_dir();
        let path = dir.join("plan.json");
        std::fs::write(&path, "{not json").expect("write");
        assert!(load_plan_file(&path).is_err());
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile_dir();
        assert!(load_plan_file(&dir.join("nope.json")).is_err());
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "horizon-mission-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }
}

#[cfg(test)]
mod status_semantics {
    use super::*;

    #[test]
    fn replaced_and_done_satisfy_dependency_edges() {
        assert!(TaskStatus::Done.satisfies_deps());
        assert!(
            TaskStatus::Replaced {
                by: TaskId::new("T2").expect("valid")
            }
            .satisfies_deps()
        );
        assert!(!TaskStatus::Failed.satisfies_deps());
        assert!(!TaskStatus::Queued.satisfies_deps());
        assert!(!TaskStatus::Running.satisfies_deps());
    }

    #[test]
    fn terminal_statuses_count_toward_completion() {
        assert!(TaskStatus::Done.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Skipped.is_terminal());
        assert!(!TaskStatus::Queued.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
    }

    #[test]
    fn effort_factors_scale_token_estimates() {
        let off = TaskSize::Small.base_megatokens() * ThinkingLevel::Off.effort_factor();
        assert!((off - 0.3).abs() < 1e-6, "off effort must not scale: {off}");
        assert!((TaskSize::Large.base_megatokens() * ThinkingLevel::Max.effort_factor() - 3.15).abs() < 1e-6);
        assert!(ThinkingLevel::High.effort_factor() > ThinkingLevel::Medium.effort_factor());
    }

    #[test]
    fn thinking_flag_omits_off() {
        assert_eq!(ThinkingLevel::Off.flag_value(), None);
        assert_eq!(ThinkingLevel::Xhigh.flag_value(), Some("xhigh"));
        assert_eq!(ThinkingLevel::High.to_string(), "high");
        assert_eq!(ThinkingLevel::Off.to_string(), "off");
    }

    #[track_caller]
    fn expect_task_count_error(plan: &MissionPlan, expected: usize) {
        let err = plan.validate().expect_err("should reject");
        let msg = err.to_string();
        assert!(msg.contains(&expected.to_string()), "{msg}");
    }

    #[test]
    fn task_count_limit_is_enforced() {
        let tasks = (0..=MAX_MISSION_TASKS)
            .map(|i| task(&format!("T{i:02}"), &[]))
            .collect();
        expect_task_count_error(&plan(tasks), MAX_MISSION_TASKS + 1);
    }
}
