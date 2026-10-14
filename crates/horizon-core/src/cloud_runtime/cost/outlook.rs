//! What an hourly rate comes to over a day, a week, a month and a year, and what a
//! worker that keeps running adds by the end of the month and the year. A month
//! counts 730 hours and a year 8,760, as Hetzner counts them. The ends of a month
//! and a year are at midnight UTC, so they do not depend on the computer's time zone.
use super::{format_amount, format_rate};
use std::time::SystemTime;
use time::{Date, Month, OffsetDateTime};

/// A period that a rate is projected over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}

impl Period {
    pub const ALL: [Self; 4] = [Self::Day, Self::Week, Self::Month, Self::Year];

    #[must_use]
    pub const fn hours(self) -> f64 {
        match self {
            Self::Day => 24.0,
            Self::Week => 168.0,
            Self::Month => 730.0,
            Self::Year => 8_760.0,
        }
    }

    /// The short unit after the amount in the card header: `day`, `wk`, `mo`, `yr`.
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "wk",
            Self::Month => "mo",
            Self::Year => "yr",
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

/// `rate` an hour over `period`, in `currency`: `€0.58/day`, `€17.52/mo`.
#[must_use]
pub fn per(rate: f64, currency: &str, period: Period) -> String {
    format!("{}/{}", format_amount(rate * period.hours(), currency), period.unit())
}

/// The end of a calendar period and the hours from now until it.
#[derive(Clone, Debug, PartialEq)]
pub struct Until {
    /// The last day of the period, for example `Oct 31`.
    pub label: String,
    pub hours: f64,
}

/// The end of the current month in UTC.
#[must_use]
pub fn month_end(now: SystemTime) -> Option<Until> {
    let at = OffsetDateTime::from(now);
    let (year, month) = (at.year(), at.month());
    let (next_year, next_month) = if month == Month::December {
        (year.checked_add(1)?, Month::January)
    } else {
        (year, month.next())
    };
    until(
        at,
        next_year,
        next_month,
        format!("{} {}", short(month), month.length(year)),
    )
}

/// The end of the current year in UTC.
#[must_use]
pub fn year_end(now: SystemTime) -> Option<Until> {
    let at = OffsetDateTime::from(now);
    until(at, at.year().checked_add(1)?, Month::January, "Dec 31".to_owned())
}

/// The second row of the card header for a worker billing `rate` an hour:
/// `€4.03/wk · €17.52/mo · +€9.89 by Oct 31 if left running`.
#[must_use]
pub fn line(rate: f64, currency: &str, now: SystemTime) -> String {
    let mut parts = vec![per(rate, currency, Period::Week), per(rate, currency, Period::Month)];
    if let Some(end) = month_end(now) {
        parts.push(format!("{} if left running", added(rate, currency, &end)));
    }
    parts.join(" · ")
}

/// Every period and both calendar ends, for the tooltip of the card header.
#[must_use]
pub fn detail(rate: f64, currency: &str, now: SystemTime) -> String {
    let periods = std::iter::once(format_rate(rate, currency))
        .chain(
            Period::ALL
                .iter()
                .map(|period| format!("{}/{}", format_amount(rate * period.hours(), currency), period.name())),
        )
        .collect::<Vec<_>>()
        .join(" · ");
    let month = month_end(now);
    // In December the month and the year end on the same day.
    let year = year_end(now).filter(|year| month.as_ref().is_none_or(|month| month.label != year.label));
    let ends = [month, year]
        .into_iter()
        .flatten()
        .map(|end| added(rate, currency, &end))
        .collect::<Vec<_>>();
    let ends = (!ends.is_empty()).then(|| format!("If it keeps running: {} (UTC).", ends.join(" and ")));
    [Some(periods), ends, Some(LENGTHS.to_owned())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

const LENGTHS: &str = "A day is 24 hours, a week 168, a month 730 and a year 8,760.";

/// What `rate` adds until `end`: `+€9.89 by Oct 31`.
fn added(rate: f64, currency: &str, end: &Until) -> String {
    format!("+{} by {}", format_amount(rate * end.hours, currency), end.label)
}

fn until(at: OffsetDateTime, year: i32, month: Month, label: String) -> Option<Until> {
    let end = Date::from_calendar_date(year, month, 1).ok()?.midnight().assume_utc();
    Some(Until {
        label,
        hours: (end - at).as_seconds_f64() / 3_600.0,
    })
}

/// `Oct` for October.
fn short(month: Month) -> &'static str {
    match month {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;

    fn at(text: &str) -> SystemTime {
        let parsed = OffsetDateTime::parse(text, &Rfc3339).unwrap_or(OffsetDateTime::UNIX_EPOCH);
        SystemTime::from(parsed)
    }

    #[test]
    fn a_rate_reads_per_day_week_month_and_year() {
        assert_eq!(per(0.024, "EUR", Period::Day), "€0.58/day");
        assert_eq!(per(0.024, "EUR", Period::Week), "€4.03/wk");
        assert_eq!(per(0.024, "EUR", Period::Month), "€17.52/mo");
        assert_eq!(per(0.024, "EUR", Period::Year), "€210.24/yr");
        assert_eq!(per(0.69, "USD", Period::Day), "$16.56/day");
    }

    #[test]
    fn the_month_ends_at_midnight_utc_after_its_last_day() {
        let end = month_end(at("2026-10-14T19:00:00Z"));
        assert_eq!(
            end,
            Some(Until {
                label: "Oct 31".into(),
                hours: 17.0 * 24.0 + 5.0
            })
        );
    }

    #[test]
    fn a_leap_february_and_a_december_end_on_their_last_days() {
        let february = month_end(at("2028-02-10T00:00:00Z"));
        assert_eq!(february.as_ref().map(|end| end.label.as_str()), Some("Feb 29"));
        assert_eq!(february.map(|end| end.hours), Some(20.0 * 24.0));
        let december = month_end(at("2026-12-31T23:00:00Z"));
        assert_eq!(december.as_ref().map(|end| end.label.as_str()), Some("Dec 31"));
        assert_eq!(december.map(|end| end.hours), Some(1.0));
    }

    #[test]
    fn the_year_ends_on_december_31() {
        let end = year_end(at("2026-12-30T12:00:00Z"));
        assert_eq!(
            end,
            Some(Until {
                label: "Dec 31".into(),
                hours: 36.0
            })
        );
    }

    #[test]
    fn the_header_line_adds_the_rest_of_the_month() {
        let now = at("2026-10-14T19:00:00Z");
        assert_eq!(
            line(0.024, "EUR", now),
            "€4.03/wk · €17.52/mo · +€9.91 by Oct 31 if left running"
        );
    }

    #[test]
    fn the_detail_names_every_period_and_both_ends() {
        let text = detail(0.024, "EUR", at("2026-10-14T19:00:00Z"));
        assert!(
            text.starts_with("€0.0240/h · €0.58/day · €4.03/week · €17.52/month · €210.24/year\n"),
            "{text}"
        );
        assert!(
            text.contains("If it keeps running: +€9.91 by Oct 31 and +€45.05 by Dec 31 (UTC)."),
            "{text}"
        );
    }

    #[test]
    fn december_names_its_end_once() {
        let text = detail(0.024, "EUR", at("2026-12-30T12:00:00Z"));
        assert!(text.contains("If it keeps running: +€0.86 by Dec 31 (UTC)."), "{text}");
    }
}
