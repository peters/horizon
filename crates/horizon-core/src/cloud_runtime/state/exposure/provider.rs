//! Passive provider comparison/journal facts, never a client or replay permission.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Create,
    Rules,
    Apply,
    Remove,
    Delete,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderPhase {
    Intent { step: Step },
    Actions { step: Step },
    Reconciling {},
    Observed {},
    Unknown {},
    Absent {},
}

/// Kept readable even when runtime provider control is disabled. JSON rules are
/// opaque evidence; no runtime controller is ever constructed from this record.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderProgress {
    pub epoch: Uuid,
    pub phase: ProviderPhase,
    pub firewall_id: Option<u64>,
    pub rules_json: String,
    pub foreign: BTreeMap<u64, String>,
    pub action_ids: Vec<u64>,
    pub services: BTreeMap<Uuid, u16>,
}
impl ProviderProgress {
    pub(super) fn valid(&self, epoch: Uuid) -> bool {
        let id = |id: u64| id > 0 && id <= 9_007_199_254_740_991;
        self.epoch == epoch
            && !epoch.is_nil()
            && (!self.absent() || self.services.is_empty())
            && self.firewall_id.is_none_or(id)
            && self.rules_json.len() <= 131_072
            && self.foreign.len() <= 5
            && self.foreign.iter().all(|(key, digest)| {
                id(*key)
                    && digest.len() == 64
                    && digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            && self.services.len() <= 8
            && self.services.iter().all(|(id, port)| {
                !id.is_nil()
                    && (horizon_cloud_protocol::exposure::RESERVED_INGRESS_START
                        ..=horizon_cloud_protocol::exposure::RESERVED_INGRESS_END)
                        .contains(port)
            })
            && self.action_ids.len() <= 64
            && self.action_ids.iter().all(|value| id(*value))
            && self.action_ids.iter().collect::<BTreeSet<_>>().len() == self.action_ids.len()
    }
    #[must_use]
    pub fn absent(&self) -> bool {
        self.phase == ProviderPhase::Absent {}
    }
}
