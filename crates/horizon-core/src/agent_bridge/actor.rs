//! The actor of the orchestrator of one workspace: the identity of the agent
//! program that the dock runs with no terminal.
//!
//! A card names the actor of the program that drew it, and the host resolves
//! that name against its own board. The name of an orchestrator therefore holds
//! one workspace local id and nothing else: the form is
//! `horizon:orchestrator/<local id>`, beside the form of a session of a machine,
//! `horizon:machine/<machine>/<session>` (`crates/horizon-core/src/node_cards/paths.rs`).
//!
//! Only a local id of the form that [`crate::new_local_id`] makes is accepted:
//! 36 bytes of lower-case hexadecimal in groups of 8, 4, 4, 4 and 12, divided by
//! hyphens. No other byte can be part of an actor, so an actor holds no slash, no
//! dot, no space and no control character, and no name of a person or of a file
//! can be spelled as one. A local id that this module refuses gets no actor, and
//! a program without an actor draws no card: the host answers
//! `caller_unavailable`.

/// The first parts of the actor of the orchestrator of one workspace. It differs
/// from every prefix of a panel (`horizon:<local id>`) and from the prefix of a
/// machine session, so one actor names one caller.
pub const ORCHESTRATOR_PREFIX: &str = "horizon:orchestrator/";

/// The bytes of a local id that this form holds: the length of the UUID that
/// [`crate::new_local_id`] makes. A shorter or longer id is refused, so the name
/// of a person or of a file cannot be spelled as an actor.
pub const LOCAL_ID_BYTES: usize = 36;

/// The places of the four hyphens of that form.
const HYPHENS: [usize; 4] = [8, 13, 18, 23];

/// Whether `local_id` is a local id of the form that [`crate::new_local_id`]
/// makes.
#[must_use]
pub fn is_local_id(local_id: &str) -> bool {
    local_id.len() == LOCAL_ID_BYTES
        && local_id.bytes().enumerate().all(|(index, byte)| {
            if HYPHENS.contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

/// The actor of the orchestrator of the workspace with `local_id`, or `None` for
/// a local id that this form does not accept.
#[must_use]
pub fn of_workspace(local_id: &str) -> Option<String> {
    is_local_id(local_id).then(|| format!("{ORCHESTRATOR_PREFIX}{local_id}"))
}

/// The local id of the workspace of an orchestrator actor, or `None` for every
/// other actor, such as the actor of a panel or of a machine session.
#[must_use]
pub fn workspace_of(actor: &str) -> Option<&str> {
    let local_id = actor.strip_prefix(ORCHESTRATOR_PREFIX)?;
    is_local_id(local_id).then_some(local_id)
}
