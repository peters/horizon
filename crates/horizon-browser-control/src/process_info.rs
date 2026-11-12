//! The start time that the `ps` of a Unix system reports for one process.
//!
//! Two processes that had the same id have different start times, so a value
//! from this module tells a live process from one that reused its id. The
//! program is the `ps` of the system, and the reader is shared by the lease of
//! a standalone browser host and by the panel child of Horizon.

/// The program that reports the start time of a process.
pub const START_TIME_PROGRAM: &str = "ps";

/// The locale of the read of a start time.
///
/// A Mac writes the start time as a date of the language of the session, and a
/// lease keeps that value for a later process: a reader of another language
/// would call a live host stale. `LC_ALL` overrides every other category, so
/// the answer is one text whatever the language of the person is.
pub const START_TIME_LOCALE: (&str, &str) = ("LC_ALL", "C");

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

/// The `ps` command that reads the start time of `pid`.
///
/// Every reader of the shared format starts the program through this command,
/// so the locale of the value is the same for all of them.
#[must_use]
pub fn start_time_command(pid: u32) -> std::process::Command {
    let mut command = std::process::Command::new(START_TIME_PROGRAM);
    command
        .args(start_time_arguments(pid))
        .env(START_TIME_LOCALE.0, START_TIME_LOCALE.1);
    command
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
    use super::{START_TIME_LOCALE, START_TIME_PROGRAM, parse_start_time, start_time_arguments, start_time_command};

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
    fn the_read_of_a_start_time_pins_the_locale_of_its_date() {
        assert_eq!(START_TIME_LOCALE, ("LC_ALL", "C"));
        let command = start_time_command(481);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            [("LC_ALL".as_ref(), Some("C".as_ref()))],
            "a date of another language never decides the identity of a process"
        );
        assert_eq!(command.get_program(), START_TIME_PROGRAM);
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
