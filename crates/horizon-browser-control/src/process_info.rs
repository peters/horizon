//! The start time that the `ps` of a Unix system reports for one process.
//!
//! Two processes that had the same id have different start times, so a value
//! from this module tells a live process from one that reused its id. The
//! program is the `ps` of the system, and the reader is shared by the lease of
//! a standalone browser host and by the panel child of Horizon.

/// The program that reports the start time of a process.
pub const START_TIME_PROGRAM: &str = "ps";

/// The arguments that ask `ps` for the start time of `pid`.
///
/// `lstart=` keeps the header away, so the output is one value and no column
/// name. The order is fixed: `-p`, the id, then `-o` with the field.
#[must_use]
pub fn start_time_arguments(pid: u32) -> [String; 4] {
    [
        "-p".to_string(),
        pid.to_string(),
        "-o".to_string(),
        "lstart=".to_string(),
    ]
}

/// The start time in the standard output of `ps`, or `None` when the output
/// names none.
///
/// The text has spaces, because it is a date, so it is only trimmed: a
/// byte-order mark is a part of the first field and no part of the value.
#[must_use]
pub fn parse_start_time(output: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    let value = text.trim().trim_start_matches('\u{feff}').trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{START_TIME_PROGRAM, parse_start_time, start_time_arguments};

    #[test]
    fn the_arguments_name_the_process_and_ask_for_one_field() {
        assert_eq!(START_TIME_PROGRAM, "ps");
        assert_eq!(
            start_time_arguments(481),
            [
                "-p".to_string(),
                "481".to_string(),
                "-o".to_string(),
                "lstart=".to_string()
            ]
        );
    }

    #[test]
    fn the_answer_is_one_trimmed_value_and_no_header() {
        assert_eq!(
            parse_start_time(b"  Wed Nov 12 21:03:04 2026\n"),
            Some("Wed Nov 12 21:03:04 2026".to_string())
        );
        assert_eq!(
            parse_start_time("\u{feff}Thu Nov 13 08:00:00 2026".as_bytes()),
            Some("Thu Nov 13 08:00:00 2026".to_string())
        );
        assert_eq!(parse_start_time(b""), None);
        assert_eq!(parse_start_time(b" \n\t"), None);
    }
}
