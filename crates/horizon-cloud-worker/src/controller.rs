//! An interrupted command must not attribute a reused PID to the former agent.
use horizon_browser_protocol::cloud_view::CloudViewResponse;
use std::path::Path;

struct Controller {
    actor: Option<String>,
    last_actor: Option<String>,
    active: bool,
    pid: u32,
    process_identity: Option<String>,
}

pub(super) fn observe(response: &mut CloudViewResponse) {
    if let Ok(bytes) = std::fs::read("/workspace/device.json.controller.json")
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        apply(response, Controller::from_value(&value), Path::new("/proc"));
    }
}

impl Controller {
    fn from_value(value: &serde_json::Value) -> Self {
        Self {
            actor: value["actor"].as_str().map(str::to_owned),
            last_actor: value
                .get("last_actor")
                .unwrap_or(&value["actor"])
                .as_str()
                .map(str::to_owned),
            active: value["active"].as_bool() == Some(true),
            pid: value["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .unwrap_or_default(),
            process_identity: value["process_identity"].as_str().map(str::to_owned),
        }
    }
}

fn apply(response: &mut CloudViewResponse, controller: Controller, proc_root: &Path) {
    response.desktop_last_input = controller.last_actor;
    response.desktop_controller = None;
    if controller.active
        && let Some(expected) = controller.process_identity
        && identity(proc_root, controller.pid).as_ref() == Some(&expected)
    {
        response.desktop_controller = controller.actor;
    }
}

fn identity(proc_root: &Path, pid: u32) -> Option<String> {
    let record = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    let mut fields = record.rsplit_once(") ")?.1.split_whitespace();
    if matches!(fields.next()?, "Z" | "X" | "x") {
        return None;
    }
    let start = fields.nth(18)?;
    let boot = std::fs::read_to_string(proc_root.join("sys/kernel/random/boot_id")).ok()?;
    Some(format!("{}:{start}", boot.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_requires_matching_boot_and_process_start_and_a_live_state() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("44")).unwrap();
        std::fs::create_dir_all(root.path().join("sys/kernel/random")).unwrap();
        std::fs::write(root.path().join("sys/kernel/random/boot_id"), "boot-one\n").unwrap();
        for (recorded, start, state, active) in [
            (Some("boot-one:123"), "123", "S", true),
            (Some("boot-one:123"), "124", "S", false),
            (Some("old-boot:123"), "123", "S", false),
            (Some("boot-one:123"), "123", "Z", false),
            (None, "123", "S", false),
        ] {
            let fields = std::iter::repeat_n("0", 18).collect::<Vec<_>>().join(" ");
            std::fs::write(
                root.path().join("44/stat"),
                format!("44 (name with ) spaces) {state} {fields} {start} 0"),
            )
            .unwrap();
            let controller = Controller::from_value(&serde_json::json!({
                "actor":"agent-current","last_actor":"agent-previous","active":true,
                "pid":44,"process_identity":recorded
            }));
            let mut response = CloudViewResponse::default();
            apply(&mut response, controller, root.path());
            assert_eq!(
                response.desktop_controller.as_deref(),
                active.then_some("agent-current")
            );
            assert_eq!(response.desktop_last_input.as_deref(), Some("agent-previous"));
        }
    }

    #[test]
    fn unfinished_first_input_does_not_claim_completed_input() {
        let first = Controller::from_value(&serde_json::json!({"actor":"agent","last_actor":null}));
        assert!(first.last_actor.is_none());
        let legacy = Controller::from_value(&serde_json::json!({"actor":"previous"}));
        assert_eq!(legacy.last_actor.as_deref(), Some("previous"));
    }
}
