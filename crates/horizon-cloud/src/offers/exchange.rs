//! Dated reference rates for estimates; provider invoices retain their own currency.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};
pub use time::{Date, OffsetDateTime};

const SOURCE: &str = "https://data-api.ecb.europa.eu/service/data/EXR/D..EUR.SP00.A?lastNObservations=1&format=csvdata";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Rates {
    pub date: String,
    pub usd_per_unit: BTreeMap<String, f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Exchange rates are unavailable: {0}")]
    Request(#[from] ureq::Error),
    #[error("Exchange rates are invalid or more than seven days old")]
    Invalid,
}

impl Rates {
    /// Current ECB reference rates, without any provider credentials.
    /// # Errors
    /// Refuses missing, malformed, future or stale rates and bounded HTTP failures.
    pub fn fetch() -> Result<Self, Error> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .build();
        let text = ureq::Agent::new_with_config(config)
            .get(SOURCE)
            .call()?
            .body_mut()
            .with_config()
            .limit(128 * 1024)
            .read_to_string()?;
        Self::parse(&text, OffsetDateTime::now_utc().date())
    }

    /// Convert a finite nonnegative estimate using a current dated quote.
    #[must_use]
    pub fn dollars(&self, amount: f64, currency: &str) -> Option<f64> {
        self.dollars_on(amount, currency, OffsetDateTime::now_utc().date())
    }

    #[must_use]
    pub fn dollars_on(&self, amount: f64, currency: &str, today: Date) -> Option<f64> {
        if !amount.is_finite() || amount < 0.0 || !self.current(today) {
            return None;
        }
        let rate = *self.usd_per_unit.get(currency)?;
        let converted = amount * rate;
        (rate.is_finite() && rate > 0.0 && converted.is_finite()).then_some(converted)
    }

    #[must_use]
    pub fn current(&self, today: Date) -> bool {
        let Some(date) = date(&self.date) else { return false };
        (0..=7).contains(&(today - date).whole_days()) && self.usd_per_unit.get("USD") == Some(&1.0)
    }

    fn parse(text: &str, today: Date) -> Result<Self, Error> {
        // The first eight ECB CSV fields contain no quoted metadata; titles follow them.
        let mut rows = Vec::new();
        for line in text.lines().skip(1) {
            let fields: Vec<_> = line.split(',').take(8).collect();
            if let [_, "D", currency, "EUR", "SP00", "A", date, value] = fields.as_slice()
                && currency.len() == 3
                && currency.bytes().all(|letter| letter.is_ascii_uppercase())
                && let Ok(value) = value.parse::<f64>()
                && value.is_finite()
                && value > 0.0
            {
                rows.push((*currency, *date, value));
            }
        }
        let (_, observed, dollars) = rows.iter().find(|row| row.0 == "USD").ok_or(Error::Invalid)?;
        let mut usd_per_unit = BTreeMap::from([("EUR".to_owned(), *dollars)]);
        for (currency, _, value) in rows.iter().filter(|row| row.1 == *observed) {
            usd_per_unit.insert((*currency).to_owned(), dollars / value);
        }
        let rates = Self {
            date: (*observed).to_owned(),
            usd_per_unit,
        };
        rates.current(today).then_some(rates).ok_or(Error::Invalid)
    }
}

fn date(value: &str) -> Option<Date> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = time::Month::try_from(parts.next()?.parse::<u8>().ok()?).ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Date::from_calendar_date(year, month, day).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_preserve_the_date_and_convert_only_current_supported_currencies() {
        let csv = "HEADER\nEXR,D,USD,EUR,SP00,A,2026-09-29,1.2,\"metadata, with commas\"\nEXR,D,GBP,EUR,SP00,A,2026-09-29,0.8\nEXR,D,JPY,EUR,SP00,A,2020-01-01,100";
        let today = date("2026-09-30").unwrap();
        let rates = Rates::parse(csv, today).unwrap();
        assert_eq!(rates.dollars_on(10.0, "EUR", today), Some(12.0));
        assert_eq!(rates.dollars_on(10.0, "USD", today), Some(10.0));
        assert!((rates.dollars_on(10.0, "GBP", today).unwrap() - 15.0).abs() < 1e-10);
        assert!(rates.dollars_on(10.0, "JPY", today).is_none());
        assert!(rates.dollars_on(f64::NAN, "USD", today).is_none());
        assert!(rates.dollars_on(10.0, "EUR", date("2026-10-07").unwrap()).is_none());
        assert!(Rates::parse(csv, date("2026-09-28").unwrap()).is_err());
        assert!(Rates::parse("HEADER\nEXR,D,USD,EUR,SP00,A,2026-09-29,NaN", today).is_err());
    }
}
