//! The machine's spending envelope: the one approval that bounds what an agent's clouds may
//! cost, and the record of what it already paid for.
//!
//! The approval is a section of the cloud settings, as the agent lifecycle policy is, and the
//! rents are lines beside those settings. A machine without the section rents nothing for an
//! agent, because a rent that nobody bounded would bill for as long as nobody looks.
use horizon_core::{
    browser::manifest,
    cloud_runtime::{envelope as record, settings::Settings},
};
use std::path::Path;

/// Why a machine rents nothing for an agent: it holds no approved envelope.
pub(super) const NO_ENVELOPE: &str = "cloud_create_refused: this machine has no spending envelope, so no agent may rent; the person approves one in the spending_envelope section of the cloud settings";
/// Why a rent is refused when the record of what the envelope paid for cannot be read.
pub(super) const LEDGER: &str =
    "cloud_create_refused: the spending envelope record could not be read, so no rent is admitted";

/// The approved envelope of these settings, or the refusal that rents nothing.
///
/// # Errors
/// [`NO_ENVELOPE`] without the section, and [`LEDGER`] when the record that a bound would be
/// read from cannot be read: a bound that Horizon cannot see is not a bound.
pub(super) fn approval(machine: &Settings, root: &Path) -> Result<record::Envelope, String> {
    let envelope = machine.spending_envelope.clone().ok_or(NO_ENVELOPE)?;
    record::Ledger::at(root).spent().map_err(|_| LEDGER.to_owned())?;
    Ok(envelope)
}

/// The envelope's own admission of one rent, and the line that a later audit and the cost
/// view read. A refusal writes nothing, so a cloud that is refused leaves no record and no
/// budget consumed. The line stands once it is written, because a worker may be rented after
/// it even when the deployment that follows fails.
pub(super) fn admit(
    root: &Path,
    envelope: &record::Envelope,
    ask: &record::Ask,
    cloud: &str,
    hourly: f64,
    actor: &str,
) -> Result<record::Admitted, String> {
    let ledger = record::Ledger::at(root);
    let spent = ledger.spent().map_err(|_| LEDGER.to_owned())?;
    let admitted = envelope
        .admit(spent, ask)
        .map_err(|refusal| format!("{}: {}", refusal.code(), refusal.describe(envelope, spent, ask)))?;
    ledger
        .record(&record::Rent {
            schema_version: 1,
            at_millis: manifest::now_millis(),
            cloud: cloud.to_owned(),
            provider: ask.provider.clone(),
            hours: admitted.hours,
            hourly,
            estimate: admitted.estimate,
            currency: admitted.currency.clone(),
            approved_by: admitted.approved_by.clone(),
            actor: actor.to_owned(),
        })
        .map_err(|error| format!("cloud_create_refused: the spending envelope record could not be written: {error}"))?;
    Ok(admitted)
}
