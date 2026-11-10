//! One click on a card, as the agent that owns the card reads it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A click the person made on a card, answered to the owning agent.
///
/// A click names the component the person hit and the document revision they
/// saw. The person's row selection travels with it, so a button such as
/// "Continue" tells the agent which option was chosen. A click reaches an agent
/// at most once: an answer of `next_click` removes it from the card.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
#[serde(deny_unknown_fields)]
pub struct ClickEvent {
    /// Unique in one card, counted up from 1 in the order of the clicks. A
    /// second click on the same component has a new id.
    pub event_id: u64,
    /// The card that was clicked, as `create` returned it.
    pub panel_id: String,
    /// The id of the button or list row the person hit.
    pub component_id: String,
    /// The document revision the person saw, not the current one.
    pub revision: u64,
    /// The row ids selected in the person's screen at that moment, in document
    /// order. Empty when no list had a selected row.
    pub selected: Vec<String>,
    /// Wall-clock time of the click on the Horizon host, in milliseconds since
    /// the Unix epoch.
    pub at_millis: i64,
    /// Whether `revision` is older than the card's revision when Horizon
    /// answered, so the person pressed a button of an older document.
    pub stale: bool,
}
