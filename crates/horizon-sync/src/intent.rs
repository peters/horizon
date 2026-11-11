//! The intents that a computer sends, and the results that the server answers.
//!
//! An intent is one change of one object, named by the computer that made it and
//! a counter of that computer. The identifier is what makes a resend safe: the
//! server keeps the identifiers it applied, so a second copy of an intent gets the
//! first result instead of a second change.
//!
//! `seen` is the version of the object that the computer saw. It decides the
//! version rule of a versioned object: the first intent on one version wins and a
//! later intent on the same version gets a typed refusal. The machine list is not
//! versioned, because two computers that add and remove machines decide by commit
//! order and not by a version: `machine_put` and the removals are last write
//! wins, and `seen` is not read for them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Refusal;

/// The kind of object that holds one machine of the shared list.
pub const KIND_MACHINE: &str = "machine";

/// The kind of object that holds the key that a machine uses.
pub const KIND_BINDING: &str = "binding";

/// The kind of object that holds one secret. Its body is never in the objects
/// table, never in the log and never in a line of the log: the table `secrets`
/// holds it, sealed, and the log row of a secret holds the kind, the id and the
/// version only.
pub const KIND_SECRET: &str = "secret";

/// An object that keeps nothing a person uses. It exists so that the version
/// rule has an object to prove itself on before the cards of a later slice
/// arrive, and it is bounded like every other kind.
pub const KIND_PROBE: &str = "probe";

/// The most intents of one push.
pub const MAX_INTENTS_PER_PUSH: usize = 256;

/// The most probe objects that the server keeps.
pub const MAX_PROBES: u64 = 64;

/// The most secrets of one push. A push also holds at most
/// [`MAX_INTENTS_PER_PUSH`] intents, and the two bounds together keep the frame
/// below [`crate::MAX_FRAME_BYTES`]: eight bodies of the largest size that
/// [`horizon_core::secret::MAX_SECRET_BYTES`] allows are 128 KiB.
pub const MAX_SECRETS_PER_PUSH: usize = 8;

pub use horizon_core::secret::{MAX_SECRET_BYTES, MAX_SECRETS};

/// One change that a computer asks the server to make.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    /// The computer that made the intent. The link knows the same name, and the
    /// server refuses an intent that names another one.
    pub device: String,
    /// The counter of that computer. It only grows, and it is never reused.
    pub id: u64,
    /// The version of the object that the computer saw, when it saw one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<u64>,
    pub op: Op,
}

/// What one intent asks for.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// Adds a machine, or replaces the machine of that name.
    MachinePut { record: horizon_core::Machine },
    /// Forgets a machine of that name.
    MachineRemove { name: String },
    /// Sets the key that a machine uses, or forgets it when `key` is `None`.
    /// The binding is last write wins, as the machine list is.
    BindingPut { machine: String, key: Option<String> },
    /// Writes one versioned secret of this id. `body` is `None` in the queue of
    /// a computer, which holds the id only, and the computer fills it from its
    /// own key folder or credential store when it pushes. A body never reaches a
    /// queue file, a mirror file or a line of the log.
    SecretPut { id: String, body: Option<SecretBody> },
    /// Removes one secret. A removal is last write wins, as a machine removal is.
    SecretRemove { id: String },
    /// Writes one versioned object of an internal kind.
    ObjectPut { kind: String, id: String, body: Value },
    /// Removes one versioned object of an internal kind.
    ObjectRemove { kind: String, id: String },
}

impl Op {
    /// The kind of object that this intent changes.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::MachinePut { .. } | Self::MachineRemove { .. } => KIND_MACHINE,
            Self::BindingPut { .. } => KIND_BINDING,
            Self::SecretPut { .. } | Self::SecretRemove { .. } => KIND_SECRET,
            Self::ObjectPut { kind, .. } | Self::ObjectRemove { kind, .. } => kind,
        }
    }

    /// The id of the object that this intent changes.
    #[must_use]
    pub fn object_id(&self) -> &str {
        match self {
            Self::MachinePut { record } => &record.name,
            Self::MachineRemove { name } => name,
            Self::BindingPut { machine, .. } => machine,
            Self::SecretPut { id, .. }
            | Self::SecretRemove { id }
            | Self::ObjectPut { id, .. }
            | Self::ObjectRemove { id, .. } => id,
        }
    }

    /// Whether this intent changes a secret, whose body stays outside the queue
    /// file, the mirror file and the log.
    #[must_use]
    pub fn is_secret(&self) -> bool {
        matches!(self, Self::SecretPut { .. } | Self::SecretRemove { .. })
    }

    /// The body of a secret intent, when it carries one.
    #[must_use]
    pub fn secret_body(&self) -> Option<&SecretBody> {
        match self {
            Self::SecretPut { body, .. } => body.as_ref(),
            _ => None,
        }
    }
}

/// One secret body: the opaque text of one machine key or one credential value.
///
/// The server stores these bytes and never reads them. The [`std::fmt::Debug`]
/// of this type is redacted, so an intent that a caller prints or logs shows the
/// id of the secret and not its value.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SecretBody(String);

impl SecretBody {
    /// The body of `text`.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// The body as the text that it is.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretBody(<redacted>)")
    }
}

/// What the server did with one intent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum Outcome {
    /// The change is committed at this log index.
    Applied { index: u64 },
    /// The change is not committed, with the reason.
    Refused(Refusal),
}

/// The result of one intent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntentResult {
    pub id: u64,
    pub outcome: Outcome,
}

/// The answer to one push.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PushReply {
    /// One result for each intent of the push, in the order of the push.
    pub results: Vec<IntentResult>,
    /// The first intent counter of this computer that the server has not
    /// decided. A computer that holds no intent below it lost nothing.
    pub next_expected: u64,
}

impl PushReply {
    /// The results that changed nothing.
    pub fn refusals(&self) -> impl Iterator<Item = (u64, &Refusal)> {
        self.results.iter().filter_map(|result| match &result.outcome {
            Outcome::Refused(refusal) => Some((result.id, refusal)),
            Outcome::Applied { .. } => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_intent_result_and_a_refusal_round_trip_as_json() {
        let applied = IntentResult {
            id: 42,
            outcome: Outcome::Applied { index: 118 },
        };
        let encoded = serde_json::to_value(&applied).expect("encode");
        assert_eq!(
            encoded,
            json!({ "id": 42, "outcome": { "outcome": "applied", "index": 118 } })
        );
        assert_eq!(
            serde_json::from_value::<IntentResult>(encoded).expect("decode"),
            applied
        );

        let refused = IntentResult {
            id: 7,
            outcome: Outcome::Refused(Refusal::ChangedSinceSeen { seen: 3, version: 4 }),
        };
        let encoded = serde_json::to_value(&refused).expect("encode");
        assert_eq!(
            encoded,
            json!({
                "id": 7,
                "outcome": {
                    "outcome": "refused",
                    "refusal": "changed_since_seen",
                    "seen": 3,
                    "version": 4
                }
            }),
            "a refusal is one typed reason with its numbers"
        );
        assert_eq!(
            serde_json::from_value::<IntentResult>(encoded).expect("decode"),
            refused
        );
    }

    #[test]
    fn an_intent_names_its_object_for_the_log() {
        let machine = Intent {
            device: "mbp-7c1".to_owned(),
            id: 3,
            seen: None,
            op: Op::MachinePut {
                record: horizon_core::Machine {
                    name: "build-1".to_owned(),
                    ..horizon_core::Machine::default()
                },
            },
        };
        assert_eq!(machine.op.kind(), KIND_MACHINE);
        assert_eq!(machine.op.object_id(), "build-1");
        let remove = Op::MachineRemove {
            name: "build-2".to_owned(),
        };
        assert_eq!(remove.object_id(), "build-2");
        let probe = Op::ObjectRemove {
            kind: KIND_PROBE.to_owned(),
            id: "p1".to_owned(),
        };
        assert_eq!(probe.kind(), KIND_PROBE);
        assert_eq!(probe.object_id(), "p1");
    }
}
