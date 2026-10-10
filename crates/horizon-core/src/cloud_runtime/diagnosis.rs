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
const TRANSIENT_MARKERS: [&str; 2] = ["retrying", "will retry"];

/// Authentication refusals any service can print; only registry output makes them a registry's.
const AUTHENTICATION: &[&str] = &[
    "unauthorized",
    "unauthenticated",
    "cannot be authenticated",
    "authentication required",
    "token has expired",
    "token is expired",
    "token expired",
    "expired token",
];

/// Words that place a failure in image transfer: the line's or the summary's. A plain
/// "image" is not one: a Git dependency fetched while building an image never meets a registry.
const REGISTRY_CONTEXT: &[&str] = &[
    "registry",
    "uploading image",
    "pushing image",
    "pulling image",
    "pull credential",
    "docker push",
    "docker pull",
    "azurecr.io",
    "ghcr.io",
    "docker.io",
];

/// Lookup failures any service can print; only registry output makes them a registry's.
const HOST_NOT_FOUND: &[&str] = &[
    "no such host",
    "server misbehaving",
    "temporary failure in name resolution",
];

const REGISTRY_REFUSED: &str =
    "The registry refused the request. Its saved credentials have expired or lack access to this image.";

/// A refused push: Horizon publishes with its own Docker configuration, never the
/// person's usual Docker login, so the fix is a publishing login there.
const PUSH_REFUSED: &str = "The registry refused to publish the image. On ghcr.io Horizon publishes as you once \
     you allow it on the card, so your GitHub account must be able to publish to this image repository. For \
     another registry, add a publishing credential for its image repository in Cloud settings › Container \
     registry. Then retry.";

/// Words of the summaries that a publishing credential fails: a failed image push, and
/// the preflight of a saved publishing credential.
const PUBLISHING: [&str; 2] = ["uploading image", "publishing credential"];

const DOCKER_NOT_RESPONDING: &str = "Docker stopped answering, so its commands wait without end. Restart Docker, then retry.";

/// A name still held after a timed-out create: the client was stopped, the daemon kept
/// or never finished the container, which a hung daemon does.
const DOCKER_NAME_IN_USE: &str = "A container from an earlier attempt still has this name. This usually follows a Docker \
     command that hung and was stopped, so Docker itself may be stuck. Restart Docker, then retry.";

const DOCKER_NOT_RUNNING: &str = "Docker is not running on this computer. Start it, then retry.";

/// A well-known cause: any of `patterns`, and when `context` is not empty, one of
/// those words in the line or the failure summary too.
struct Known {
    patterns: &'static [&'static str],
    context: &'static [&'static str],
    meaning: &'static str,
}

const fn known(patterns: &'static [&'static str], meaning: &'static str) -> Known {
    Known {
        patterns,
        context: &[],
        meaning,
    }
}

/// Well-known causes, first match wins. Keep patterns lowercase.
const MEANINGS: [Known; 14] = [
    known(
        &["no space left on device", "disk full", "disk quota exceeded"],
        "The disk filled up. Free space on this computer or the worker, or grow the workspace.",
    ),
    known(
        &["docker is not responding", "docker did not answer"],
        DOCKER_NOT_RESPONDING,
    ),
    known(&["is already in use by container"], DOCKER_NAME_IN_USE),
    known(
        &["permission denied (publickey"],
        "The SSH key was refused. The worker does not trust this cloud's key.",
    ),
    known(
        &[
            "from registry: denied",
            "from registry: unauthorized",
            "from registry: unauthenticated",
            "requested access to the resource is denied",
            // Docker's words for a push to a registry that wants a login it does not have.
            "push access denied",
            "no basic auth credentials",
        ],
        REGISTRY_REFUSED,
    ),
    Known {
        patterns: AUTHENTICATION,
        context: REGISTRY_CONTEXT,
        meaning: REGISTRY_REFUSED,
    },
    known(
        AUTHENTICATION,
        "The service refused the credentials Horizon sent. They may have expired or lack the rights this step needs.",
    ),
    Known {
        patterns: HOST_NOT_FOUND,
        context: REGISTRY_CONTEXT,
        meaning: "The registry's host name does not resolve. Check the registry address in cloud settings and this computer's network.",
    },
    known(
        HOST_NOT_FOUND,
        "The host name does not resolve. Check the address the failing step uses and this computer's network.",
    ),
    known(
        &["manifest unknown", "not found: manifest"],
        "The registry has no image with this name and tag.",
    ),
    known(
        &[
            "too many requests",
            "rate limit",
            "toomanyrequests",
            "rate limit exceeded",
            "rate limit reached",
            "rate limited",
        ],
        "The service is rate limiting requests. Wait a little, then retry.",
    ),
    known(
        &[
            "no instances available",
            "insufficient capacity",
            "out of stock",
            "no longer any instances available",
        ],
        "The provider has no capacity for this size right now. Try another size or data center.",
    ),
    known(
        &["timed out", "timeout"],
        "The operation timed out. The provider or network may be slow; retrying often works.",
    ),
    known(
        &["cannot connect to the docker daemon", "is the docker daemon running"],
        DOCKER_NOT_RUNNING,
    ),
];

/// Whether `meaning` says that Docker itself is stuck or stopped, which restarting
/// Docker can fix.
#[must_use]
pub fn restarts_docker(meaning: &str) -> bool {
    [DOCKER_NOT_RESPONDING, DOCKER_NAME_IN_USE, DOCKER_NOT_RUNNING].contains(&meaning)
}

/// How many failure lines before the newest one may explain it.
const EXPLAINING_LINES: usize = 4;

/// Finds the cause among `lines`, newest last. `summary` is the failure message
/// Horizon already shows; a line repeating it is not the cause.
#[must_use]
pub fn diagnose<'a>(lines: impl DoubleEndedIterator<Item = &'a str>, summary: &str) -> Option<Diagnosis> {
    let summary = summary.trim();
    let failures: Vec<&str> = lines
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != summary)
        .filter(|line| is_failure(line))
        .take(EXPLAINING_LINES + 1)
        .collect();
    let newest = *failures.first()?;
    // A tool can repeat its error without its context, as Docker prints "denied" after
    // "error from registry: denied". A recent failure line that explains the failure is then
    // the cause.
    let cause = if meaning_in(newest, summary).is_some() {
        newest
    } else {
        failures
            .iter()
            .copied()
            .find(|line| meaning_in(line, summary).is_some())
            .unwrap_or(newest)
    };
    Some(Diagnosis {
        cause: cause.to_owned(),
        meaning: meaning_in(cause, summary).or_else(|| meaning(summary)),
    })
}

/// What a failure line usually means, when it is a well-known one.
#[must_use]
pub fn meaning(line: &str) -> Option<&'static str> {
    meaning_in(line, "")
}

/// What `line` means, where `summary` names the step that failed (for example
/// "Uploading image failed") and so supplies context the line lacks.
#[must_use]
pub fn meaning_in(line: &str, summary: &str) -> Option<&'static str> {
    let lower = line.to_ascii_lowercase();
    let summary = summary.to_ascii_lowercase();
    let meaning = MEANINGS
        .iter()
        .find(|known| {
            known.patterns.iter().any(|pattern| lower.contains(pattern))
                && (known.context.is_empty()
                    || known
                        .context
                        .iter()
                        .any(|word| lower.contains(word) || summary.contains(word)))
        })
        .map(|known| known.meaning);
    // A registry refusal while the image uploads is a missing publishing login.
    if meaning == Some(REGISTRY_REFUSED) && PUBLISHING.iter().any(|word| summary.contains(word)) {
        return Some(PUSH_REFUSED);
    }
    meaning
}

/// What fixes a well-known cause where a retry alone fails again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remedy {
    /// A publishing credential for the image repository in Cloud settings › Container
    /// registry, for a refused push.
    PublishingLogin,
    /// A worker pull credential for the image repository in Cloud settings › Container
    /// registry, for a refused pull.
    PullLogin,
}

/// The fix for a failure whose meaning is `meaning`, when Horizon offers one.
#[must_use]
pub fn remedy(meaning: &str) -> Option<Remedy> {
    match meaning {
        PUSH_REFUSED => Some(Remedy::PublishingLogin),
        REGISTRY_REFUSED => Some(Remedy::PullLogin),
        _ => None,
    }
}

/// Whether a line reports a failure rather than progress or a retry.
#[must_use]
pub fn is_failure(line: &str) -> bool {
    let lower = without_zero_counts(&line.to_ascii_lowercase());
    (FAILURE_MARKERS.iter().any(|marker| starts_a_word(&lower, marker)) || known_cause(&lower)) && !transient(&lower)
}

/// A line that reports a failure another try will follow. "Failed after retrying 5
/// times" is the end of the retries, not one of them.
fn transient(lower: &str) -> bool {
    const EXHAUSTED: [&str; 4] = ["after retrying", "gave up", "giving up", "retries exhausted"];
    TRANSIENT_MARKERS.iter().any(|marker| lower.contains(marker))
        && !EXHAUSTED.iter().any(|marker| lower.contains(marker))
}

/// Well-known causes that name no failure word ("manifest unknown", "toomanyrequests").
/// Patterns as likely in settings or progress as in failures are left to the markers.
fn known_cause(lower: &str) -> bool {
    const AMBIGUOUS: [&str; 2] = ["timeout", "rate limit"];
    MEANINGS
        .iter()
        .flat_map(|known| known.patterns)
        .filter(|pattern| !AMBIGUOUS.contains(pattern))
        .any(|pattern| starts_a_word(lower, pattern))
}

/// Whether `marker` begins a word of `text`: "error:" and "errors" count, the
/// "error" in a crate named "thiserror" does not.
fn starts_a_word(text: &str, marker: &str) -> bool {
    text.match_indices(marker)
        .any(|(index, _)| !text[..index].ends_with(|c: char| c.is_ascii_alphanumeric()))
}

/// Counts of nothing, such as `0 failed` in `test result: ok. 214 passed; 0 failed`,
/// report success; drop them before looking for failure words. `10 failed` stays.
fn without_zero_counts(lower: &str) -> String {
    const NONE: [&str; 4] = ["0 ", "no ", "zero ", "without "];
    const COUNTED: [&str; 6] = ["failed", "failures", "failure", "errors", "error", "warnings"];
    let mut kept = String::with_capacity(lower.len());
    let mut rest = lower;
    while let Some((index, none)) = NONE
        .iter()
        .filter_map(|none| rest.find(none).map(|index| (index, *none)))
        .min_by_key(|(index, _)| *index)
    {
        let after = &rest[index + none.len()..];
        let counted = COUNTED.iter().find(|word| after.starts_with(*word));
        // "10 errors" and "piano error" are not counts of nothing.
        let inside_word = |text: &str| text.ends_with(|c: char| c.is_ascii_alphanumeric());
        let starts_word = if index == 0 {
            !inside_word(&kept)
        } else {
            !inside_word(&rest[..index])
        };
        match counted {
            Some(word) if starts_word => {
                kept.push_str(&rest[..index]);
                rest = &after[word.len()..];
            }
            _ => {
                kept.push_str(&rest[..index + none.len()]);
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    kept
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
    fn a_refused_push_asks_for_a_publishing_login_and_a_refused_pull_for_valid_credentials() {
        let lines = [
            "edd1ed89f0d4: Layer already exists",
            "error from registry: unauthenticated: User cannot be authenticated with the token provided.",
        ];
        let push = diagnose(lines.into_iter(), SUMMARY).unwrap().meaning.unwrap();
        assert!(push.contains("publishing credential"), "{push}");
        assert!(push.contains("On ghcr.io Horizon publishes as you"), "{push}");
        let pull = diagnose(
            ["docker pull ghcr.io/example/worker: unauthorized"].into_iter(),
            "Readiness failed",
        )
        .unwrap()
        .meaning
        .unwrap();
        assert_eq!(pull, REGISTRY_REFUSED);
    }

    #[test]
    fn a_registry_refusal_is_fixed_by_a_login_and_other_causes_by_a_retry() {
        let lines = ["error from registry: unauthenticated: User cannot be authenticated with the token provided."];
        for (summary, fix) in [
            (SUMMARY, Remedy::PublishingLogin),
            (
                "Publishing credential preflight failed; check the saved login, repository push permissions and registry connectivity before retrying",
                Remedy::PublishingLogin,
            ),
            (
                "Worker pull credential preflight failed; check the saved login, repository permissions and registry connectivity before retrying",
                Remedy::PullLogin,
            ),
            ("Readiness failed", Remedy::PullLogin),
        ] {
            let meaning = diagnose(lines.into_iter(), summary).unwrap().meaning.unwrap();
            assert_eq!(remedy(meaning), Some(fix), "{summary}");
        }
        let without_login = [
            "push access denied, repository does not exist or may require authorization: authorization failed: \
             no basic auth credentials",
        ];
        let meaning = diagnose(without_login.into_iter(), SUMMARY).unwrap().meaning.unwrap();
        assert_eq!(meaning, PUSH_REFUSED);
        assert_eq!(remedy(meaning), Some(Remedy::PublishingLogin));
        for line in [
            "write /var/lib/docker: no space left on device",
            "provider API: unauthorized",
            "dial tcp: lookup registry.invalid: no such host",
        ] {
            let meaning = diagnose([line].into_iter(), "Requesting a worker failed")
                .unwrap()
                .meaning
                .unwrap();
            assert_eq!(remedy(meaning), None, "{line}");
        }
    }

    #[test]
    fn a_bare_repeat_of_the_error_leaves_the_cause_to_the_line_that_explains_it() {
        let lines = ["b686816be845: Waiting", "error from registry: denied", "denied"];
        let found = diagnose(lines.into_iter(), SUMMARY).unwrap();
        assert_eq!(found.cause, "error from registry: denied");
        assert!(found.meaning.unwrap().contains("publish"));
        // A newest line without a known meaning stays the cause when no recent line explains it.
        let found = diagnose(["error: something odd", "fatal: odd"].into_iter(), "Build failed").unwrap();
        assert_eq!(found.cause, "fatal: odd");
    }

    #[test]
    fn a_count_of_nothing_is_success_and_a_count_of_ten_is_not() {
        for success in [
            "test result: ok. 214 passed; 0 failed; 0 ignored",
            "Finished with 0 errors and 0 warnings",
            "build: 0 failures",
            "No errors detected",
            "request timeout: 30s",
            "rate limit: 100 requests per minute",
            "Validation finished with no failures",
            "completed without errors",
            "   Compiling thiserror v2.0.12",
        ] {
            assert!(!is_failure(success), "{success}");
        }
        for failure in [
            "test result: FAILED. 3 passed; 10 failed",
            "20 errors generated",
            "error: 0 bytes written",
            "no errors in the plan, but the build failed",
            "error: request failed after retrying 5 times",
            "push failed: gave up retrying after 3 attempts",
            "piano error: string snapped",
            "manifest unknown: manifest unknown",
            "disk full",
            "Your token has expired",
            "error: rate limit exceeded",
            "API rate limit reached for this account",
            "toomanyrequests: You have reached your pull rate limit",
            "insufficient capacity in the selected data center",
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
        ] {
            assert!(is_failure(failure), "{failure}");
        }
        let lines = ["error: could not connect", "test result: ok. 5 passed; 0 failed"];
        assert_eq!(
            diagnose(lines.into_iter(), "Build failed").unwrap().cause,
            "error: could not connect",
            "a later success summary is not picked as the cause"
        );
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
            (
                "dial tcp: lookup registry.invalid: no such host",
                "registry's host name does not resolve",
            ),
            (
                "dial tcp: lookup api.runpod.io: no such host",
                "address the failing step uses",
            ),
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
    fn a_stuck_docker_is_named_rather_than_its_symptom() {
        const CONFLICT: &str = "docker: Error response from daemon: Conflict. The container name \"/horizon-contract-0b1c\" \
             is already in use by container \"0123456789ab\". You have to remove (or rename) that container to be able to \
             reuse that name.";
        let summary = "worker image contract creation failed; inspect deployment output";
        let found = diagnose([CONFLICT].into_iter(), summary).unwrap();
        assert_eq!(found.cause, CONFLICT);
        assert_eq!(found.meaning, Some(DOCKER_NAME_IN_USE));

        let summary = "Docker is not responding: worker image contract creation did not finish and Docker did not \
             answer a health check";
        let found = diagnose(
            ["Docker did not answer docker version within 5 s"].into_iter(),
            summary,
        )
        .unwrap();
        assert_eq!(found.cause, "Docker did not answer docker version within 5 s");
        assert_eq!(found.meaning, Some(DOCKER_NOT_RESPONDING));
        assert_eq!(meaning(summary), Some(DOCKER_NOT_RESPONDING), "the summary alone says it too");

        for docker in [DOCKER_NOT_RESPONDING, DOCKER_NAME_IN_USE, DOCKER_NOT_RUNNING] {
            assert!(restarts_docker(docker));
        }
        assert!(!restarts_docker(REGISTRY_REFUSED));
        // A plain timeout is not a stuck Docker.
        assert!(!meaning("Local operation timed out").is_some_and(restarts_docker));
    }

    #[test]
    fn no_output_gives_no_diagnosis() {
        assert_eq!(diagnose(std::iter::empty(), SUMMARY), None);
        assert_eq!(diagnose([SUMMARY].into_iter(), SUMMARY), None);
    }

    #[test]
    fn an_authentication_refusal_is_a_registry_one_only_in_image_transfer() {
        const SERVICE: &str = "service refused";
        for (line, summary, wanted) in [
            ("provider API: unauthorized", "Requesting a worker failed", SERVICE),
            (
                "fatal: remote: authentication required",
                "Preparing worktrees failed",
                SERVICE,
            ),
            ("fatal: remote: authentication required", "Build failed", SERVICE),
            (
                "fatal: remote: authentication required",
                "Building image failed",
                SERVICE,
            ),
            ("unauthorized: authentication required", SUMMARY, "registry refused"),
            (
                "docker pull ghcr.io/example/worker: unauthorized",
                "Readiness failed",
                "registry refused",
            ),
        ] {
            let found = diagnose([line].into_iter(), summary).unwrap();
            assert!(
                found.meaning.is_some_and(|text| text.contains(wanted)),
                "{line}: {found:?}"
            );
        }
    }
}
