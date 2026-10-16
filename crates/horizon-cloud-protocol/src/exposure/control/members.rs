//! Bounded comparison facts for the original held owner frame, never a vector grant.
use super::{Error, WorkerIdentity};
use serde::{Deserialize, Serialize};

pub const MAX_MEMBERS: usize = 64;
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberFacts {
    pub recipient: WorkerIdentity,
    pub stable_ids: Vec<String>,
}
impl MemberFacts {
    /// # Errors
    /// Rejects absent, repeated, noncanonical or foreign-recipient facts.
    /// Actual authority still requires the original owner stream and live source chain.
    pub fn validate(&self) -> Result<(), Error> {
        self.recipient.validate()?;
        if self.stable_ids.is_empty()
            || self.stable_ids.len() > MAX_MEMBERS
            || self.stable_ids.windows(2).any(|pair| pair[0] >= pair[1])
            || !self.stable_ids.contains(&self.recipient.self_id)
            || self.stable_ids.iter().any(|id| {
                id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        {
            return Err(Error::Members);
        }
        Ok(())
    }
}
