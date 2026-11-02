//! A person's one approval for an agent to rent clouds inside limits.
//!
//! The envelope is the safety, and the tool that spends reads it. It names one provider, the
//! currency that provider bills in, the hours the person allows, and the most the envelope may
//! cost. An offer that does not fit is refused with the limit it broke, before a worker is
//! requested, and never partly: a refusal leaves no worker, no card and no charge.
//!
//! The approval lives in the `spending_envelope` section of the cloud settings, as the agent
//! lifecycle policy does, so a machine without that section rents nothing for an agent. What
//! the envelope paid for is a separate record beside those settings, because the person owns
//! the approval and Horizon owns the evidence.
use super::Error;
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

/// The longest envelope Horizon accepts. A longer one is refused when the settings load, so a
/// typo cannot approve a fortnight of a rented machine.
pub const LONGEST_HOURS: f64 = 24.0 * 14.0;
/// How much of the rent record Horizon reads. An envelope whose record is longer than this
/// refuses a rent, because a bound it cannot see is not a bound.
pub const MAX_LEDGER_BYTES: u64 = 256 * 1024;
/// The records one reading keeps, as a second bound on the same file.
pub const MAX_LEDGER_RENTS: usize = 1024;

/// A person's approval, as the `spending_envelope` section of the cloud settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// The provider this envelope pays for. Another provider never fits.
    pub provider: String,
    /// The currency of [`Self::max_total`], as that provider bills.
    pub currency: String,
    /// The hours of rent this envelope allows in total, across every cloud it pays for.
    pub max_hours: f64,
    /// The most this envelope may cost in total, in [`Self::currency`].
    pub max_total: f64,
    /// The person who approved it.
    pub approved_by: String,
    /// When that person approved it, in RFC 3339.
    pub approved_at: String,
}

/// Why an envelope cannot be approved or used as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// The provider is not named.
    Provider,
    /// The currency is not three uppercase letters.
    Currency,
    /// The hours are not a positive number no longer than [`LONGEST_HOURS`].
    Hours,
    /// The total is not a positive, finite number.
    Total,
    /// The approving person is not named, or the approval time is not RFC 3339.
    Approval,
}

/// One rent this envelope is asked to pay for.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    /// The provider the offer is on.
    pub provider: String,
    /// The currency the offer bills in.
    pub currency: String,
    /// The hours this rent may run.
    pub hours: f64,
    /// The offer's estimated total for those hours, in [`Self::currency`].
    pub estimate: f64,
}

/// What the envelope already paid for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spent {
    pub hours: f64,
    pub amount: f64,
}

/// The hours and the amount this envelope has left.
#[derive(Clone, Debug, PartialEq)]
pub struct Remaining {
    pub hours: f64,
    pub amount: f64,
    pub currency: String,
}

/// Why an envelope does not pay for a rent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The offer is for another provider.
    Provider,
    /// The offer bills in another currency, so the amounts cannot be compared.
    Currency,
    /// The rent asks for more hours than the envelope has left, or for none.
    Hours,
    /// The rent's estimate is above what the envelope has left.
    Amount,
}

/// What one rent is admitted for, as the rent path records it.
#[derive(Clone, Debug, PartialEq)]
pub struct Admitted {
    /// The hours this rent may run, never more than the envelope had left.
    pub hours: f64,
    /// The offer's estimated total for those hours, in [`Self::currency`].
    pub estimate: f64,
    /// The currency of [`Self::estimate`], as the provider bills it.
    pub currency: String,
    /// The approving person, so the record shows who allowed it.
    pub approved_by: String,
}

impl Envelope {
    /// Whether this envelope can be approved and used as written.
    ///
    /// # Errors
    /// [`Invalid`] for each field that cannot be used.
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.provider.trim().is_empty() {
            return Err(Invalid::Provider);
        }
        let currency = self.currency.as_bytes();
        if currency.len() != 3 || !currency.iter().all(u8::is_ascii_uppercase) {
            return Err(Invalid::Currency);
        }
        if !self.max_hours.is_finite() || self.max_hours <= 0.0 || self.max_hours > LONGEST_HOURS {
            return Err(Invalid::Hours);
        }
        if !self.max_total.is_finite() || self.max_total <= 0.0 {
            return Err(Invalid::Total);
        }
        if self.approved_by.trim().is_empty() || !is_rfc3339(&self.approved_at) {
            return Err(Invalid::Approval);
        }
        Ok(())
    }

    /// Admits one rent from `ask` against what the envelope already paid for, or names the
    /// limit it breaks. A refusal admits nothing at all.
    ///
    /// # Errors
    /// [`Refusal`] naming the first limit that does not hold.
    pub fn admit(&self, spent: Spent, ask: &Ask) -> Result<Admitted, Refusal> {
        if ask.provider != self.provider {
            return Err(Refusal::Provider);
        }
        if ask.currency != self.currency {
            return Err(Refusal::Currency);
        }
        if !ask.hours.is_finite() || ask.hours <= 0.0 || spent.hours + ask.hours > self.max_hours + EPSILON {
            return Err(Refusal::Hours);
        }
        if !ask.estimate.is_finite() || spent.amount + ask.estimate > self.max_total + EPSILON {
            return Err(Refusal::Amount);
        }
        Ok(Admitted {
            hours: ask.hours,
            estimate: ask.estimate,
            currency: self.currency.clone(),
            approved_by: self.approved_by.clone(),
        })
    }

    /// What is left of the envelope after `spent`.
    #[must_use]
    pub fn remaining(&self, spent: Spent) -> Remaining {
        Remaining {
            hours: (self.max_hours - spent.hours).max(0.0),
            amount: (self.max_total - spent.amount).max(0.0),
            currency: self.currency.clone(),
        }
    }
}

/// A float comparison's slack, so an amount that is exactly at the limit is inside it.
const EPSILON: f64 = 1e-9;

impl Invalid {
    /// The reason a person reads when the section does not hold.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Provider => "the spending envelope needs a provider",
            Self::Currency => "the spending envelope needs a three-letter currency, such as EUR",
            Self::Hours => "the spending envelope hours must be more than 0 and no more than 336",
            Self::Total => "the spending envelope total must be more than 0",
            Self::Approval => "the spending envelope needs the person who approved it and an RFC 3339 time",
        }
    }
}

impl Refusal {
    /// The error code the agent's create reports.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Provider | Self::Currency => "cloud_create_outside_envelope",
            Self::Hours => "cloud_create_envelope_hours",
            Self::Amount => "cloud_create_envelope_amount",
        }
    }

    /// The sentence a caller reads: what the envelope allows and what was asked.
    #[must_use]
    pub fn describe(self, envelope: &Envelope, spent: Spent, ask: &Ask) -> String {
        let remaining = envelope.remaining(spent);
        let allows = format!(
            "this envelope allows {} in total on {} up to {} h and {}; {} h and {} are left",
            envelope.currency,
            envelope.provider,
            count(envelope.max_hours),
            amount(envelope.max_total, &envelope.currency),
            count(remaining.hours),
            amount(remaining.amount, &remaining.currency),
        );
        match self {
            Self::Provider => format!("{allows}; this offer is on {}", ask.provider),
            Self::Currency => format!(
                "{allows}; this offer bills in {}, so the amounts cannot be compared",
                ask.currency
            ),
            Self::Hours => format!("{allows}; this rent asks for {} h", count(ask.hours)),
            Self::Amount => format!(
                "{allows}; this rent is estimated at {}",
                amount(ask.estimate, &ask.currency)
            ),
        }
    }
}

/// One rent that an envelope paid for, as Horizon records it for a later audit and for the
/// cost view. It holds no credential and no instruction.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rent {
    pub schema_version: u32,
    /// When Horizon admitted the rent, in milliseconds since the Unix epoch.
    pub at_millis: i64,
    /// The cloud the rent paid for.
    pub cloud: String,
    pub provider: String,
    pub hours: f64,
    pub hourly: f64,
    pub estimate: f64,
    pub currency: String,
    /// The person who approved the envelope.
    pub approved_by: String,
    /// When that person approved it, which names the approval this rent belongs to: a later
    /// approval counts only its own rents, so a new envelope starts with its whole budget.
    pub approved_at: String,
    /// The calling agent's actor, so a later audit shows who spent it.
    pub actor: String,
}

/// The rents of one machine's envelopes, appended one line at a time beside the cloud
/// settings. An append is one short line, so two hosts of one machine never interleave a
/// record, and a reading keeps its own bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct Ledger {
    root: PathBuf,
}

impl Ledger {
    /// The ledger under a cloud root, which holds the settings it belongs to.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// The file the rents are appended to.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root.join("envelope-rents.jsonl")
    }

    /// Appends one admitted rent.
    ///
    /// # Errors
    /// [`Error::Io`] when the record cannot be written, and [`Error::Invalid`] for a rent that
    /// no envelope could have admitted.
    pub fn record(&self, rent: &Rent) -> super::Result<()> {
        if rent.schema_version != 1 || rent.cloud.is_empty() || !rent.hours.is_finite() || rent.hours <= 0.0 {
            return Err(Error::Invalid("An envelope rent must name a cloud and its hours"));
        }
        std::fs::create_dir_all(&self.root)?;
        // A torn tail is dropped before the append, so this rent is a line of its own and a
        // reader never sees two records joined into one. A torn tail held no admitted rent:
        // the create that wrote it was refused, because this write did not finish.
        truncate_torn_tail(&self.path())?;
        let mut line = serde_json::to_string(rent).map_err(|_| Error::Json)?;
        line.push('\n');
        let mut file = OpenOptions::new().create(true).append(true).open(self.path())?;
        file.write_all(line.as_bytes())?;
        file.sync_data()?;
        Ok(())
    }

    /// Every rent this machine recorded, oldest first.
    ///
    /// # Errors
    /// [`Error::Io`] when the file cannot be read, and [`Error::Invalid`] when it is longer than
    /// [`MAX_LEDGER_BYTES`], because a bound that Horizon cannot read is not a bound.
    pub fn rents(&self) -> super::Result<Vec<Rent>> {
        let text = match std::fs::read_to_string(self.path()) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(Error::Io(error)),
        };
        if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_LEDGER_BYTES {
            return Err(Error::Invalid(
                "The envelope rent record is longer than Horizon reads, so no rent is admitted",
            ));
        }
        let mut rents = Vec::new();
        for line in text.split_inclusive('\n') {
            if line.trim().is_empty() {
                continue;
            }
            // A tail without its newline is an append that never finished, so it holds no
            // admitted rent: the create that tried to write it was refused.
            if !line.ends_with('\n') {
                continue;
            }
            // A line that arrived whole and cannot be read is a record Horizon does not
            // understand, and a bound it cannot read is not a bound.
            let Ok(rent) = serde_json::from_str::<Rent>(line) else {
                return Err(Error::Invalid(
                    "The envelope rent record holds a line Horizon cannot read, so no rent is admitted",
                ));
            };
            rents.push(rent);
            if rents.len() > MAX_LEDGER_RENTS {
                return Err(Error::Invalid(
                    "The envelope rent record holds more rents than Horizon reads, so no rent is admitted",
                ));
            }
        }
        Ok(rents)
    }

    /// What `envelope` already paid for. A rent of an earlier approval is not counted, so a
    /// new approval starts with its whole budget, and a rent that another approval recorded
    /// cannot make its amount wrong.
    ///
    /// # Errors
    /// As [`Self::rents`].
    pub fn spent(&self, envelope: &Envelope) -> super::Result<Spent> {
        Ok(self
            .rents()?
            .into_iter()
            .filter(|rent| {
                rent.approved_at == envelope.approved_at
                    && rent.provider == envelope.provider
                    && rent.currency == envelope.currency
            })
            .fold(Spent::default(), |spent, rent| Spent {
                hours: spent.hours + rent.hours,
                amount: spent.amount + rent.estimate,
            }))
    }
}

/// Drops a trailing line that has no newline, so the next record is a line of its own. The
/// file is truncated at its last newline, which is the only byte this ever removes.
fn truncate_torn_tail(path: &Path) -> super::Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::Io(error)),
    };
    if bytes.last().is_none_or(|byte| *byte == b'\n') {
        return Ok(());
    }
    let kept = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(u64::try_from(kept).unwrap_or(0))?;
    Ok(())
}

/// Whole hours read as an integer when they are one, such as `6 h` or `1.5 h`.
fn count(hours: f64) -> String {
    if hours.fract().abs() < 1e-9 {
        format!("{hours:.0}")
    } else {
        format!("{hours:.1}")
    }
}

/// An amount with the cents that the cost view shows, then the currency it bills in.
fn amount(value: f64, currency: &str) -> String {
    format!("{value:.2} {currency}")
}

/// Whether `text` is an RFC 3339 timestamp, as a person's approval time is written.
fn is_rfc3339(text: &str) -> bool {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).is_ok()
}

#[cfg(test)]
mod tests;
