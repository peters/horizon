//! The refusals that root logged on the worker, on the board.
use super::Exchange;

impl Exchange {
    /// The agent's activity lines and the refusals that root logged on the worker. The board
    /// puts them in time order.
    pub(super) fn activity(
        &self,
        reported: &serde_json::Value,
        rfc3339: impl Fn(i64) -> Option<String>,
    ) -> serde_json::Value {
        let mut lines = match reported.get("activity") {
            Some(serde_json::Value::Array(lines)) => lines.clone(),
            _ => Vec::new(),
        };
        for refusal in &self.refused {
            if let (Some(at), Some(text)) = (
                refusal.get("at").and_then(serde_json::Value::as_i64).and_then(&rfc3339),
                refusal.get("text").and_then(serde_json::Value::as_str),
            ) {
                lines.push(serde_json::json!({ "at": at, "text": text }));
            }
        }
        lines.into()
    }
}
