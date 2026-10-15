//! Companion clouds for Upkeep's UI acceptance. The agent asks for one with
//! `upkeep/companions/<alias>.json` on its worker; the exchange brings the request to
//! Horizon, which runs it as the companion tools run an agent panel's request, and the next
//! exchange writes the answer to `upkeep/companions-status.json`.
use serde::Deserialize;

/// At most this many requests are read from one exchange; the exchange sends no more.
const LIMIT: usize = 4;

/// What the agent may ask of a companion of its Upkeep cloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompanionAction {
    EnsureReady,
    Stop,
}

/// One request read from the worker. The agent account writes it, so Horizon checks the
/// alias against this cloud's own declarations and the operation ID again before it acts.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanionAsk {
    pub alias: String,
    pub action: CompanionAction,
    pub operation_id: String,
}

/// The well-formed requests of an exchange's answer. One that does not parse is left out,
/// so it never hides the rest of the answer.
pub(super) fn parse(values: Vec<serde_json::Value>) -> Vec<CompanionAsk> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value::<CompanionAsk>(value).ok())
        .filter(|ask| {
            horizon_cloud::companions::valid_alias(&ask.alias) && uuid::Uuid::parse_str(&ask.operation_id).is_ok()
        })
        .take(LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_well_formed_requests_are_kept() {
        let operation = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
        let asks = parse(vec![
            serde_json::json!({"alias": "youpark", "action": "ensure_ready", "operation_id": operation}),
            serde_json::json!({"alias": "web", "action": "delete", "operation_id": operation}),
            serde_json::json!({"alias": "Web", "action": "stop", "operation_id": operation}),
            serde_json::json!({"alias": "api", "action": "stop", "operation_id": "1"}),
            serde_json::json!({"alias": "api", "action": "stop", "operation_id": operation, "extra": 1}),
            serde_json::json!("youpark"),
        ]);
        assert_eq!(
            asks,
            [CompanionAsk {
                alias: "youpark".into(),
                action: CompanionAction::EnsureReady,
                operation_id: operation.into(),
            }]
        );
        let many = (0..9)
            .map(|n| serde_json::json!({"alias": format!("c{n}"), "action": "stop", "operation_id": operation}))
            .collect();
        assert_eq!(parse(many).len(), LIMIT);
    }
}
