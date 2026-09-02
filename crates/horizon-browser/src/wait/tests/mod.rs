use super::*;
use crate::BrowserControlAction;

mod conditions;
mod failures;
mod release;

fn node(visible: bool) -> BrowserNode {
    BrowserNode {
        reference: String::new(),
        role: "paragraph".to_string(),
        name: String::new(),
        text: "late".to_string(),
        visible,
        enabled: true,
        bounds: None,
    }
}

fn pending(state: SelectorState, timeout_millis: u64, now: Instant) -> PendingWait {
    let request = AgentAction {
        action_id: "wait-1".to_string(),
        actor: "horizon:agent".to_string(),
        requested_at_millis: 0,
        action: BrowserControlAction::WaitForSelector {
            selector: "#late".to_string(),
            state,
            timeout_millis: Some(timeout_millis),
        },
    };
    PendingWait::new(
        request,
        "#late".to_string(),
        state,
        Some(timeout_millis),
        Duration::ZERO,
        7,
        now,
    )
}

fn scan() -> serde_json::Value {
    serde_json::json!({ "nodes": [] })
}

fn done(observation: Observation) -> WaitResult {
    match observation {
        Observation::Done(result) => result,
        other => panic!("expected a completed wait, got {other:?}"),
    }
}

fn released(wait: &PendingWait, nodes: Vec<BrowserNode>, now: Instant) -> WaitOutcome {
    match wait.outcome(7, 1, nodes, now) {
        BrowserControlValue::Wait { wait } => wait,
        other => panic!("unexpected value {other:?}"),
    }
}
