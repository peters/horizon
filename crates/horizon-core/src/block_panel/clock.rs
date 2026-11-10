//! The wall-clock time a card shows in its footer.
//!
//! A card says when its document last changed, in the person's own time zone and
//! without words like "3 h ago": a relative time would go stale without a
//! repaint, and the card must not repaint just to age a label. The label is
//! formatted when the document changes, then cached with the card.

use time::{OffsetDateTime, UtcOffset};

/// The local clock time `HH:MM` of `at_millis`, or `None` when the timestamp or
/// the local offset of this computer is unknown. A timestamp of zero or less
/// means that no time was recorded, which is how a card saved before the footer
/// existed reads.
#[must_use]
pub fn local_clock(at_millis: i64) -> Option<(u8, u8)> {
    let offset = UtcOffset::current_local_offset().ok()?;
    clock_at(at_millis, offset)
}

/// The clock time `HH:MM` of `at_millis` in `offset`, or `None` when the
/// timestamp is out of range or records no time.
#[must_use]
pub fn clock_at(at_millis: i64, offset: UtcOffset) -> Option<(u8, u8)> {
    if at_millis <= 0 {
        return None;
    }
    let nanos = i128::from(at_millis).checked_mul(1_000_000)?;
    let local = OffsetDateTime::from_unix_timestamp_nanos(nanos).ok()?.to_offset(offset);
    Some((local.hour(), local.minute()))
}

/// `updated HH:MM` for a document that changed at `at_millis`, in local time,
/// or the empty string when no time is known.
#[must_use]
pub fn updated_label(at_millis: i64) -> String {
    match local_clock(at_millis) {
        Some((hour, minute)) => format!("updated {hour:02}:{minute:02}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use time::UtcOffset;

    use super::{clock_at, updated_label};

    /// 2026-10-01 19:46 UTC.
    const AT: i64 = 1_790_883_960_000;

    #[test]
    fn a_clock_is_read_in_its_own_offset() {
        assert_eq!(clock_at(AT, UtcOffset::UTC), Some((19, 46)));
        let plus_two = UtcOffset::from_hms(2, 0, 0).expect("a whole-hour offset");
        assert_eq!(clock_at(AT, plus_two), Some((21, 46)));
    }

    #[test]
    fn a_minute_is_two_digits() {
        let minus_five_thirty = UtcOffset::from_hms(-5, -30, 0).expect("a half-hour offset");
        assert_eq!(clock_at(AT, minus_five_thirty), Some((14, 16)));
    }

    #[test]
    fn a_time_that_was_never_recorded_has_no_label() {
        for at in [0, -1, i64::MIN, i64::MAX] {
            assert_eq!(updated_label(at), String::new(), "{at}");
        }
    }

    #[test]
    fn a_known_time_is_labelled_with_the_word_updated() {
        // The label depends on this computer's zone, so the test checks the
        // shape the footer draws rather than a fixed hour.
        let label = updated_label(AT);
        assert_eq!(label.len(), "updated 19:46".len(), "{label}");
        assert!(label.starts_with("updated "), "{label}");
        let clock = &label["updated ".len()..];
        assert_eq!(clock.as_bytes()[2], b':', "{label}");
        assert!(clock[..2].chars().all(|character| character.is_ascii_digit()));
        assert!(clock[3..].chars().all(|character| character.is_ascii_digit()));
    }
}
