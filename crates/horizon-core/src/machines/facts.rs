//! The three facts of a rented machine that the agent which rented it knows: the
//! size of the machine, the place that it stands in and what one hour of it costs.
//!
//! A fact is text of the agent program that the record keeps and that Horizon only
//! draws, as the provider line is: no program reads it, and it is no command, no
//! path and no title. It is one line of 1 to
//! [`MAX_FACT_CHARS`](horizon_browser_control::manifest::block_machines::MAX_FACT_CHARS)
//! characters with no control character.
//!
//! A caller that names no fact keeps the one that the record holds, and a caller
//! that sends the empty text clears it. The three fields therefore reach a record
//! only through [`MachineFacts`], which is the one place that reads that rule.

use horizon_browser_control::manifest::block_machines::{self, MachineRefusal};

use super::MachineError;
use super::record::Machine;

/// The three facts of one rented machine: its size, its place and its price for one
/// hour, after they passed their rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineFacts {
    pub size: Option<String>,
    pub place: Option<String>,
    pub price_per_hour: Option<String>,
}

impl MachineFacts {
    /// The facts that a caller reported: `size`, `place` and `price_per_hour`.
    ///
    /// An absent field is a fact that the caller does not name, which keeps the one
    /// that the record holds. The empty text clears the stored fact, so an agent
    /// program that sends every field as a string clears the ones that it left
    /// empty. [`Self::checked`] reads both rules.
    #[must_use]
    pub fn reported(size: Option<&str>, place: Option<&str>, price_per_hour: Option<&str>) -> Self {
        Self {
            size: size.map(str::to_owned),
            place: place.map(str::to_owned),
            price_per_hour: price_per_hour.map(str::to_owned),
        }
    }

    /// The facts that `record` holds.
    #[must_use]
    pub fn of(record: &Machine) -> Self {
        Self {
            size: record.size.clone(),
            place: record.place.clone(),
            price_per_hour: record.price_per_hour.clone(),
        }
    }

    /// These facts checked, with the facts of `stored` in the place of the fields
    /// that the caller did not name: the record of an update keeps the facts that
    /// the update says nothing about.
    ///
    /// # Errors
    /// Describes the first refused fact with the code of [`MachineError::code`] and
    /// the name of its field.
    pub fn checked(self, stored: Option<&Machine>) -> Result<Self, MachineError> {
        Ok(Self {
            size: fact("size", self.size, stored.and_then(|stored| stored.size.clone()))?,
            place: fact("place", self.place, stored.and_then(|stored| stored.place.clone()))?,
            price_per_hour: fact(
                "price_per_hour",
                self.price_per_hour,
                stored.and_then(|stored| stored.price_per_hour.clone()),
            )?,
        })
    }

    /// `record` with these facts in the place of the ones that it holds.
    #[must_use]
    pub fn into_record(self, mut record: Machine) -> Machine {
        record.size = self.size;
        record.place = self.place;
        record.price_per_hour = self.price_per_hour;
        record
    }

    /// The size and the place of the machine, in the words of the second line of its
    /// row in the sidebar: `cpx32 · Helsinki`, or the one that the record knows.
    #[must_use]
    pub fn size_and_place(&self) -> Option<String> {
        let parts: Vec<&str> = [self.size.as_deref(), self.place.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        (!parts.is_empty()).then(|| parts.join(" \u{00b7} "))
    }
}

/// Check the facts that a record names: its `size`, its `place` and its
/// `price_per_hour`, with no stored fact behind them.
///
/// # Errors
/// Describes the first refused fact with the code of [`MachineError::code`] and the
/// name of its field.
pub fn check_facts(
    size: Option<&str>,
    place: Option<&str>,
    price_per_hour: Option<&str>,
) -> Result<MachineFacts, MachineError> {
    MachineFacts::reported(size, place, price_per_hour).checked(None)
}

/// One fact as a caller reported it: `None` for a fact that it did not name, which
/// keeps `stored`, and the checked text for the one that it named, where the empty
/// text is no fact at all.
fn fact(field: &str, reported: Option<String>, stored: Option<String>) -> Result<Option<String>, MachineError> {
    match reported {
        None => Ok(stored),
        Some(text) if text.is_empty() => Ok(None),
        Some(text) => block_machines::check_fact(field, &text)
            .map(Some)
            .map_err(|refusal: MachineRefusal| MachineError::InvalidFact {
                message: refusal.message,
            }),
    }
}
