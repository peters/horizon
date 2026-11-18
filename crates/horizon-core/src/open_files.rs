//! The open-file limit of this process: macOS starts one with 256 of them and
//! Linux with 1024, while every panel holds files (its PTY and the socket or the
//! pipes of its program, a browser or a device panel more). A board of a few dozen
//! panels reaches the limit, and a panel that cannot start answers `Too many open
//! files (os error 24)`. Every mode raises the soft limit once at its start, and so
//! does the one place that opens a terminal of a panel; a refusal is not fatal.

use rustix::process::{Resource, getrlimit, setrlimit};

/// The most open files that this process asks for: a board of a hundred panels
/// holds a few thousand, and a program that walks every possible file at its start
/// walks at most this many. It stays below `kern.maxfilesperproc` of macOS.
const CEILING: u64 = 8192;

/// What [`raise_limit`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenFileLimit {
    /// The soft limit already allowed [`CEILING`] files, or was infinite.
    Unchanged,
    /// The soft limit of this process is `to`, and was `from`.
    Raised { from: u64, to: u64 },
    /// The kernel refused every value, so the soft limit stays at `kept`.
    Refused { kept: u64 },
}

/// Raises the soft limit of open files of this process toward the hard limit, up
/// to [`CEILING`]. It never lowers a limit and keeps the hard limit as it is, it is
/// idempotent, and a refusal is not fatal, because a process that cannot raise its
/// limit runs with the one that it has.
pub fn raise_limit() -> OpenFileLimit {
    let limits = getrlimit(Resource::Nofile);
    let Some(from) = limits.current else {
        // An infinite soft limit is above every value that this function asks for.
        return OpenFileLimit::Unchanged;
    };
    let ceiling = limits.maximum.map_or(CEILING, |hard| hard.min(CEILING));
    if from >= ceiling {
        return OpenFileLimit::Unchanged;
    }
    let mut ask = limits;
    for to in requests(from, limits.maximum) {
        ask.current = Some(to);
        if setrlimit(Resource::Nofile, ask).is_ok() {
            tracing::info!(from, to, "raised the open-file limit of this process");
            return OpenFileLimit::Raised { from, to };
        }
    }
    tracing::warn!(kept = from, "the kernel keeps the open-file limit of this process");
    OpenFileLimit::Refused { kept: from }
}

/// The values that a process with the soft limit `current` asks for, in the order
/// of the attempts. macOS refuses a soft limit above `kern.maxfilesperproc`
/// although the hard limit allows it, so a refused value asks for an eighth and a
/// sixteenth of it.
fn requests(current: u64, hard: Option<u64>) -> impl Iterator<Item = u64> {
    let ceiling = hard.map_or(CEILING, |hard| hard.min(CEILING));
    std::iter::once(ceiling)
        .chain([8, 16].map(|part| ceiling / part))
        .filter(move |wanted| *wanted > current)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use rustix::process::{Resource, getrlimit};

    use super::{CEILING, OpenFileLimit, raise_limit, requests};

    fn limits() -> (Option<u64>, Option<u64>) {
        let limits = getrlimit(Resource::Nofile);
        (limits.current, limits.maximum)
    }

    fn asked(current: u64, hard: Option<u64>) -> Vec<u64> {
        requests(current, hard).collect()
    }

    #[test]
    fn a_low_soft_limit_asks_for_the_ceiling_and_never_lowers_one() {
        // macOS as a person meets it, and a kernel that bounds the value below
        // the ceiling.
        assert_eq!(asked(256, None).first(), Some(&CEILING), "macOS asks for the ceiling");
        assert_eq!(
            asked(1024, Some(u64::MAX)).first(),
            Some(&CEILING),
            "Linux asks for it too"
        );
        assert_eq!(asked(256, Some(4096)), vec![4096, 512], "a low hard limit");

        // It never asks for a value at or below the limit that the process has,
        // so no call can lower it.
        for current in [0, 1, 255, 256, 1024, CEILING - 1, CEILING, u64::MAX] {
            for hard in [None, Some(0), Some(300), Some(CEILING), Some(1 << 20)] {
                assert!(
                    requests(current, hard).all(|wanted| wanted > current),
                    "current={current} hard={hard:?}"
                );
            }
        }
        assert!(
            asked(CEILING, None).is_empty(),
            "a limit that covers the ceiling asks for nothing"
        );
    }

    #[test]
    fn a_refused_value_falls_back_to_a_lower_one() {
        assert_eq!(
            asked(256, None),
            vec![CEILING, CEILING / 8, CEILING / 16],
            "the attempts"
        );
        assert_eq!(asked(6000, None), vec![CEILING], "a fallback stays below the ceiling");
    }

    #[test]
    fn the_limit_rises_once_and_a_second_call_changes_nothing() {
        let (Some(before), hard) = limits() else {
            return;
        };
        let raised = raise_limit();
        let after = limits().0.expect("a raised soft limit is finite");
        let ceiling = hard.map_or(CEILING, |hard| hard.min(CEILING));

        assert!(after >= before, "no call lowers the limit: {before} -> {after}");
        match raised {
            OpenFileLimit::Raised { from, to } => {
                assert_eq!(from, before, "the call reports the limit that it found");
                assert_eq!(to, after, "the call reports the limit that it set");
                assert!(to > from && to <= ceiling, "{from} -> {to}");
            }
            OpenFileLimit::Unchanged => assert!(after >= ceiling, "only a limit at the ceiling stays: {after}"),
            OpenFileLimit::Refused { kept } => assert_eq!(kept, after, "a refusal keeps the limit"),
        }
        assert_eq!(raise_limit(), OpenFileLimit::Unchanged, "a second call changes nothing");
        assert_eq!(limits().0, Some(after), "and the limit stays as it is");
    }

    /// The program of a panel starts with the limit of this process, which is
    /// what Horizon raised: the panel hands it to the child, and no program of
    /// this project bounds it a second time.
    #[test]
    fn a_program_of_a_panel_starts_with_the_limit_of_this_process() {
        raise_limit();
        let Some(limit) = limits().0 else {
            return;
        };
        let Ok(answered) = Command::new("/bin/sh").arg("-c").arg("ulimit -n").output() else {
            return;
        };
        let text = String::from_utf8_lossy(&answered.stdout);
        assert_eq!(
            text.trim().parse::<u64>().ok(),
            Some(limit),
            "the child inherits the limit: {text:?}"
        );
    }
}
