//! Fallible evidence boundaries for callers that durably track provider effects.
//! Composite provisioning remains pending on failure after it enters provider work:
//! a final rejection does not prove that earlier resource changes did not succeed.
//! Callers must never derive settlement solely from an error variant.
use super::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// No mutation from this attempt remains unconfirmed.
    Settled,
    /// Persist before a provider mutation; retain across ambiguous failures.
    Pending,
}

/// Atomically replace evidence under the target lock and return its prior durable
/// value only after the new value is saved. Failure may still have saved the value.
pub(crate) type Observer<'a> = &'a dyn Fn(State) -> Result<State>;

pub(crate) const IGNORE: Observer<'static> = &|_| Ok(State::Settled);
