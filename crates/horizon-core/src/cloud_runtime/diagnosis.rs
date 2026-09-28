//! The decisive line of a failed operation's output, and what it usually means.
//!
//! A failure summary such as "Uploading image failed; inspect deployment output"
//! names the step, not the cause. The cause is usually the last line a tool
//! printed as an error; a few well-known ones also get a plain explanation.

/// The line that explains a failure and, when recognised, what it means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnosis {
    pub cause: String,
    pub meaning: Option<&'static str>,
}

/// Markers of a line that reports a failure rather than progress.
const FAILURE_MARKERS: [&str; 10] = [
    "error",
    "no such host",
    "fatal",
    "denied",
    "unauthorized",
    "forbidden",
    "failed",
    "no space left",
    "timed out",
    "refused",
];

/// Lines that mention a failure while the operation keeps going.
const TRANSIENT_MARKERS: [&str; 3] = ["retrying", "will retry", "0 errors"];

/// Well-known causes, first match wins. Keep patterns lowercase.
const MEANINGS: [(&[&str], &str); 9] = [
    (
        &["no space left on device"],
        "The disk filled up. Free space on this computer or the worker, or grow the workspace.",
    ),
    (
        &["permission denied (publickey"],
        "The SSH key was refused. The worker does not trust this cloud's key.",
    ),
    (
        &[
            "from registry: denied",
            "unauthorized",
            "unauthenticated",
            "cannot be authenticated",
            "authentication required",
            "requested access to the resource is denied",
        ],
        "The registry refused the request. Its saved credentials have expired or lack push rights to this image.",
    ),
    (
        &[
            "no such host",
            "server misbehaving",
            "temporary failure in name resolution",
        ],
        "The host name does not resolve. Check the registry address in cloud settings and this computer's network.",
    ),
    (
        &["manifest unknown", "not found: manifest"],
        "The registry has no image with this name and tag.",
    ),
    (
        &["too many requests", "rate limit", "toomanyrequests"],
        "The service is rate limiting requests. Wait a little, then retry.",
    ),
    (
        &[
            "no instances available",
            "insufficient capacity",
            "out of stock",
            "no longer any instances available",
        ],
        "The provider has no capacity for this size right now. Try another size or data center.",
    ),
    (
        &["timed out", "timeout"],
        "The operation timed out. The provider or network may be slow; retrying often works.",
    ),
    (
        &["cannot connect to the docker daemon", "is the docker daemon running"],
        "Docker is not running on this computer. Start it, then retry.",
    ),
];

/// Finds the cause among `lines`, newest last. `summary` is the failure message
/// Horizon already shows; a line repeating it is not the cause.
#[must_use]
pub fn diagnose<'a>(lines: impl DoubleEndedIterator<Item = &'a str>, summary: &str) -> Option<Diagnosis> {
    let summary = summary.trim();
    let cause = lines
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != summary)
        .find(|line| is_failure(line))?;
    Some(Diagnosis {
        cause: cause.to_owned(),
        meaning: meaning(cause).or_else(|| meaning(summary)),
    })
}

/// What a failure line usually means, when it is a well-known one.
#[must_use]
pub fn meaning(line: &str) -> Option<&'static str> {
    let lower = line.to_ascii_lowercase();
    MEANINGS
        .iter()
        .find(|(patterns, _)| patterns.iter().any(|pattern| lower.contains(pattern)))
        .map(|(_, meaning)| *meaning)
}

/// Whether a line reports a failure rather than progress or a retry.
#[must_use]
pub fn is_failure(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    FAILURE_MARKERS.iter().any(|marker| lower.contains(marker))
        && !TRANSIENT_MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY: &str = "Uploading image failed; inspect deployment output";

    #[test]
    fn the_registry_denial_behind_a_push_failure_is_the_cause() {
        let lines = [
            "docker push registry.example/worker:9f3c2a1",
            "5f70bf18a086: Waiting",
            "ceabf9021ae9: Waiting",
            "error from registry: denied",
            SUMMARY,
        ];
        let found = diagnose(lines.into_iter(), SUMMARY).unwrap();
        assert_eq!(found.cause, "error from registry: denied");
        assert!(found.meaning.unwrap().contains("registry refused"));
    }

    #[test]
    fn progress_and_retries_are_not_causes() {
        let lines = [
            "ssh: connect to 203.0.113.24 port 22: connection refused (retrying)",
            "Pushed",
            "test result: ok. 0 errors",
        ];
        assert_eq!(diagnose(lines.into_iter(), SUMMARY), None);
    }

    #[test]
    fn an_unrecognised_failure_keeps_its_line_without_a_meaning() {
        let found = diagnose(["step 4", "Error: exit status 17"].into_iter(), "Build failed").unwrap();
        assert_eq!(found.cause, "Error: exit status 17");
        assert_eq!(found.meaning, None);
    }

    #[test]
    fn the_summary_supplies_a_meaning_when_the_line_has_none() {
        let found = diagnose(["error: operation failed"].into_iter(), "Request timed out").unwrap();
        assert!(found.meaning.unwrap().contains("timed out"));
    }

    #[test]
    fn the_newest_failure_wins_and_blank_lines_are_skipped() {
        let lines = ["error: first", "fatal: second", "   ", SUMMARY];
        assert_eq!(diagnose(lines.into_iter(), SUMMARY).unwrap().cause, "fatal: second");
    }

    #[test]
    fn well_known_causes_have_plain_meanings() {
        for (line, wanted) in [
            ("write /var/lib/docker: no space left on device", "disk filled"),
            ("Permission denied (publickey).", "SSH key"),
            ("manifest unknown: manifest unknown", "no image"),
            (
                "toomanyrequests: You have reached your pull rate limit",
                "rate limiting",
            ),
            (
                "There are no longer any instances available with the requested specifications",
                "capacity",
            ),
            (
                "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
                "Docker is not running",
            ),
            ("dial tcp: lookup registry.invalid: no such host", "does not resolve"),
            (
                "error from registry: unauthenticated: User cannot be authenticated with the token provided.",
                "registry refused",
            ),
        ] {
            assert!(meaning(line).is_some_and(|text| text.contains(wanted)), "{line}");
        }
        assert_eq!(meaning("everything fine"), None);
    }

    #[test]
    fn no_output_gives_no_diagnosis() {
        assert_eq!(diagnose(std::iter::empty(), SUMMARY), None);
        assert_eq!(diagnose([SUMMARY].into_iter(), SUMMARY), None);
    }
}
