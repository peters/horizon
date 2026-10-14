//! Native qualification of an Upkeep pull request on a test cloud.
//!
//! An Upkeep agent asks for it with a request file on its worker; the exchange brings the
//! request here. Horizon checks it against GitHub and the repository ([`approve`]), starts a
//! short-lived cloud from the repository's own profile on the pull request's head, lets an
//! agent there run the repository's native qualification on the worker desktop, collects
//! what that agent wrote ([`probe`]) and deletes the cloud. The next exchanges carry a
//! [`Report`] of each test back to the Upkeep worker.
//!
//! Everything a worker writes is untrusted: the request, the result and the evidence.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::worker::{self, Config};

mod approve;
pub mod probe;
pub use approve::{Approved, agent, approve};

/// How long a test cloud may live, from its start to its deletion.
pub const TIME_LIMIT: std::time::Duration = std::time::Duration::from_mins(120);
/// How long after a test that ended without a result of its agent the same request starts a
/// new test: a cause such as a missing login or a sold-out worker type may be gone by then.
/// A head gets at most [`ATTEMPTS`] tests, so a cause that stays costs no more.
pub const RETRY_AFTER: std::time::Duration = std::time::Duration::from_hours(1);
/// The most tests of one head of a pull request.
pub const ATTEMPTS: u32 = 2;
/// The most requests read from one exchange and kept waiting.
pub const QUEUE_LIMIT: usize = 8;
/// The most reports kept and sent back to the worker, newest first: more than the tests that
/// wait and run, so their reports always stay.
pub const REPORT_LIMIT: usize = QUEUE_LIMIT + 4;
/// The most ended tests remembered, so a request for a head that was tested is never taken
/// again, also after its report made way for newer ones.
const TESTED_LIMIT: usize = 256;
const REASON_LIMIT: usize = 300;
/// The longest message of a report, in characters.
pub const MESSAGE_LIMIT: usize = 300;
/// The longest report text sent back to the Upkeep agent.
pub const REPORT_TEXT_LIMIT: usize = 4 * 1024;
/// How many of the newest reports carry their report text and file names to the worker: a
/// delivery is one command line, which the worker's shell limits.
const REPORT_TEXTS: usize = 2;
/// The highest pull request number accepted.
const NUMBER_LIMIT: u64 = 99_999_999;

/// One request of an Upkeep agent, after the checks of [`requests`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// `owner/name`, spelled as the Upkeep lists it.
    pub repository: String,
    pub number: u64,
    /// The head commit the agent wants tested, 40 lowercase hexadecimal characters.
    pub head: String,
    /// The agent's reason, without control characters, at most 300 characters.
    #[serde(default)]
    pub reason: String,
}

impl Request {
    /// Whether `other` asks for the same test: the same pull request at the same head.
    #[must_use]
    pub fn same_test(&self, other: &Self) -> bool {
        self.repository.eq_ignore_ascii_case(&other.repository)
            && self.number == other.number
            && self.head == other.head
    }

    /// The pull request as people read it, such as `acme/web#12`.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}#{}", self.repository, self.number)
    }
}

/// The requests in `raw`, as the exchange read them from the worker, that name a repository
/// of `config`, a pull request number and a full head commit. Others are left out. A request
/// keeps the spelling of the repository that `config` uses.
#[must_use]
pub fn requests(raw: &[serde_json::Value], config: &Config) -> Vec<Request> {
    let mut found: Vec<Request> = Vec::new();
    for value in raw.iter().take(QUEUE_LIMIT) {
        let Some(request) = request(value, config) else {
            continue;
        };
        if !found.iter().any(|held| held.same_test(&request)) {
            found.push(request);
        }
    }
    found
}

fn request(value: &serde_json::Value, config: &Config) -> Option<Request> {
    let named = value.get("repository")?.as_str()?;
    let repository = config
        .repositories
        .iter()
        .find(|held| held.eq_ignore_ascii_case(named))?
        .clone();
    let number = value
        .get("pull_request")?
        .as_u64()
        .filter(|n| (1..=NUMBER_LIMIT).contains(n))?;
    let head = value.get("head")?.as_str()?;
    if !(head.len() == 40 && head.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) {
        return None;
    }
    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(REASON_LIMIT)
        .collect();
    Some(Request {
        repository,
        number,
        head: head.to_owned(),
        reason,
    })
}

/// Where a test stands, as the Upkeep agent reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Another test of this Upkeep runs first.
    Queued,
    /// Horizon checks the request and starts the test cloud.
    Starting,
    /// The agent on the test cloud runs the qualification.
    Running,
    /// The agent reported that the qualification passed.
    Passed,
    /// The agent reported that the qualification failed.
    Failed,
    /// The qualification could not run: the agent lacked something, such as a credential,
    /// or the test cloud did not start or was closed before a result.
    Blocked,
    /// Horizon did not start a test cloud for the request; the message says why.
    Refused,
    /// The time limit ended before the agent reported a result.
    Expired,
}

impl State {
    /// Whether the test is over.
    #[must_use]
    pub fn finished(self) -> bool {
        !matches!(self, Self::Queued | Self::Starting | Self::Running)
    }
}

/// What the Upkeep agent learns about one test.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub repository: String,
    pub pull_request: u64,
    pub head: String,
    pub state: State,
    /// One or two sentences for people.
    pub message: String,
    /// The report of the agent on the test cloud, such as its matrix, cut to
    /// [`REPORT_TEXT_LIMIT`].
    #[serde(default)]
    pub report: String,
    /// The evidence files Horizon kept on this computer.
    #[serde(default)]
    pub files: Vec<String>,
    /// Unix seconds of the last change.
    pub updated_at: i64,
}

impl Report {
    #[must_use]
    pub fn new(request: &Request, state: State, message: impl Into<String>) -> Self {
        Self {
            repository: request.repository.clone(),
            pull_request: request.number,
            head: request.head.clone(),
            state,
            message: message.into(),
            report: String::new(),
            files: Vec::new(),
            updated_at: now(),
        }
    }
}

/// The test cloud of the test that runs now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Current {
    pub request: Request,
    /// The id of the test cloud, once Horizon made it.
    #[serde(default)]
    pub cloud: Option<String>,
    /// Unix seconds of the start.
    pub started_at: i64,
    /// Whether the agent on the test cloud got its task.
    #[serde(default)]
    pub prompted: bool,
}

impl Current {
    /// Whether the time limit of the test ended at `now` (Unix seconds).
    #[must_use]
    pub fn expired(&self, now: i64) -> bool {
        now.saturating_sub(self.started_at) >= i64::try_from(TIME_LIMIT.as_secs()).unwrap_or(i64::MAX)
    }
}

/// The native tests of one Upkeep: the one that runs, the ones that wait and the reports.
/// Saved next to the Upkeep's settings, so a restart of Horizon continues or deletes a test
/// cloud it started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub current: Option<Current>,
    #[serde(default)]
    pub queue: Vec<Request>,
    /// Newest first, one for each pull request.
    #[serde(default)]
    pub reports: Vec<Report>,
    /// The tests that ended, newest first, without their texts.
    #[serde(default)]
    pub tested: Vec<Tested>,
    /// The test clouds that Horizon closes, until their cards are gone, so a restart closes
    /// them too.
    #[serde(default)]
    pub closing: Vec<String>,
}

/// A test that ended, as [`Ledger::take`] remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tested {
    pub repository: String,
    pub pull_request: u64,
    pub head: String,
    pub state: State,
    pub updated_at: i64,
    /// How many tests of this head ran.
    #[serde(default = "one")]
    pub attempts: u32,
}

fn one() -> u32 {
    1
}

impl Tested {
    fn of(&self, request: &Request) -> bool {
        self.repository.eq_ignore_ascii_case(&request.repository)
            && self.pull_request == request.number
            && self.head == request.head
    }

    /// Whether an agent that asks for this test again at `now` (Unix seconds) gets a new one:
    /// the test ended without `passed` or `failed`, long enough ago, and fewer than
    /// [`ATTEMPTS`] tests of the head ran.
    fn retryable(&self, now: i64) -> bool {
        matches!(self.state, State::Refused | State::Blocked | State::Expired)
            && self.attempts < ATTEMPTS
            && now.saturating_sub(self.updated_at) >= i64::try_from(RETRY_AFTER.as_secs()).unwrap_or(i64::MAX)
    }
}

impl Ledger {
    /// Takes in the requests of an exchange: a request for a test that runs, waits or has a
    /// report for the same head already is left out, unless that test ended without `passed`
    /// or `failed` more than [`RETRY_AFTER`] ago. Returns whether anything changed.
    pub fn take(&mut self, requests: Vec<Request>) -> bool {
        let mut changed = false;
        for request in requests {
            let now = now();
            let known = self
                .current
                .as_ref()
                .is_some_and(|current| current.request.same_test(&request))
                || self.queue.iter().any(|held| held.same_test(&request))
                || self
                    .tested
                    .iter()
                    .find(|tested| tested.of(&request))
                    .is_some_and(|tested| !tested.retryable(now));
            if known {
                // The agent asks about a head that was tested, and the pull request's report
                // made way for newer ones: it gets the outcome back, without the details. A
                // report of another head of the pull request stays: it is the newer one.
                if let Some(tested) = self.tested.iter().find(|tested| tested.of(&request))
                    && !self.reports.iter().any(|report| {
                        report.repository.eq_ignore_ascii_case(&request.repository)
                            && report.pull_request == request.number
                    })
                {
                    let mut report = Report::new(
                        &request,
                        tested.state,
                        "Horizon tested this head before and kept only its outcome.",
                    );
                    report.updated_at = tested.updated_at;
                    self.report(report);
                    changed = true;
                }
                continue;
            }
            if self.queue.len() >= QUEUE_LIMIT {
                continue;
            }
            self.report(Report::new(
                &request,
                State::Queued,
                "Waits for the test cloud of another pull request.",
            ));
            self.queue.push(request);
            changed = true;
        }
        changed
    }

    /// Records `report` as the newest of its pull request.
    pub fn report(&mut self, mut report: Report) {
        report.message = report.message.chars().take(MESSAGE_LIMIT).collect();
        self.reports.retain(|held| {
            !(held.repository.eq_ignore_ascii_case(&report.repository) && held.pull_request == report.pull_request)
        });
        self.reports.insert(0, report);
        // The oldest ended tests make way; a test that waits or runs keeps its report.
        while self.reports.len() > REPORT_LIMIT {
            let Some(oldest) = self.reports.iter().rposition(|held| held.state.finished()) else {
                break;
            };
            self.reports.remove(oldest);
        }
    }

    /// Starts the next waiting test, when none runs. Returns its request.
    pub fn start_next(&mut self) -> Option<Request> {
        if self.current.is_some() || self.queue.is_empty() {
            return None;
        }
        let request = self.queue.remove(0);
        self.report(Report::new(
            &request,
            State::Starting,
            "Horizon checks the pull request and starts a test cloud.",
        ));
        self.current = Some(Current {
            request: request.clone(),
            cloud: None,
            started_at: now(),
            prompted: false,
        });
        Some(request)
    }

    /// Ends the test that runs with `report`.
    pub fn finish(&mut self, report: Report) {
        self.current = None;
        let same = |tested: &Tested| {
            tested.repository.eq_ignore_ascii_case(&report.repository)
                && tested.pull_request == report.pull_request
                && tested.head == report.head
        };
        let attempts = self
            .tested
            .iter()
            .find(|tested| same(tested))
            .map_or(0, |tested| tested.attempts);
        self.tested.retain(|tested| !same(tested));
        self.tested.insert(
            0,
            Tested {
                repository: report.repository.clone(),
                pull_request: report.pull_request,
                head: report.head.clone(),
                state: report.state,
                updated_at: report.updated_at,
                attempts: attempts.saturating_add(1),
            },
        );
        self.tested.truncate(TESTED_LIMIT);
        self.report(report);
    }

    /// The reports as the exchange sends them to the worker.
    #[must_use]
    pub fn payload(&self) -> Vec<serde_json::Value> {
        self.reports
            .iter()
            .enumerate()
            .filter_map(|(index, report)| {
                let mut value = serde_json::to_value(report).ok()?;
                if index >= REPORT_TEXTS {
                    value["report"] = serde_json::Value::String(String::new());
                    value["files"] = serde_json::Value::Array(Vec::new());
                }
                Some(value)
            })
            .collect()
    }
}

/// Writes the reports of `ledger` to the Upkeep worker behind `connection`, where its agents
/// read them, and removes the reports it no longer holds.
/// # Errors
/// A failed connection, or a worker that refused the delivery.
pub fn deliver(
    connection: &crate::cloud_runtime::ssh::Connection,
    runner: &crate::cloud_runtime::command::Runner<'_>,
    ledger: &Ledger,
) -> crate::cloud_runtime::Result<()> {
    let remote = worker::delivery_command(&ledger.payload())?;
    runner
        .run_parsed(
            "Upkeep native reports",
            &mut connection.pinned_command(&remote),
            std::time::Duration::from_secs(45),
        )
        .map(drop)
}

/// `~/.horizon/upkeep/<cloud>/native.json`.
#[must_use]
pub fn ledger_path(cloud_id: &str) -> PathBuf {
    worker::directory(cloud_id).join("native.json")
}

/// The saved ledger of `cloud_id`, or an empty one when none was saved. Blocks on the disk.
/// # Errors
/// A ledger that is there but cannot be read or parsed.
pub fn load(cloud_id: &str) -> std::io::Result<Ledger> {
    match std::fs::read_to_string(ledger_path(cloud_id)) {
        Ok(text) => serde_json::from_str(&text).map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::default()),
        Err(error) => Err(error),
    }
}

/// Saves `ledger` for `cloud_id`. Blocks on the disk.
/// # Errors
/// The folder or file cannot be written.
pub fn save(cloud_id: &str, ledger: &Ledger) -> std::io::Result<()> {
    let path = ledger_path(cloud_id);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let text = serde_json::to_string_pretty(ledger).map_err(std::io::Error::other)?;
    worker::write_atomically(&path, &text)
}

/// Where Horizon keeps the evidence of a test on this computer.
#[must_use]
pub fn evidence_directory(cloud_id: &str, request: &Request) -> PathBuf {
    worker::directory(cloud_id).join("native").join(format!(
        "{}-{}-{}",
        request.repository.replace('/', "__"),
        request.number,
        &request.head[..12]
    ))
}

/// The folder on the test cloud where its agent writes the result and the evidence.
pub const EVIDENCE: &str = "/workspace/home/native-test";

/// The task that Horizon gives the agent on the test cloud, as one pasted message.
#[must_use]
pub fn task(request: &Request) -> String {
    format!(
        "You run on a short-lived test cloud that Horizon started for one native qualification: pull request #{number} of {repository}, head commit {head}. \
The checkout in your current folder is that commit. This cloud has no GitHub access, and you must not push, comment, merge or start a release. \
Read AGENTS.md and the documents it links in this checkout, find the native UI qualification (the isolated desktop and Device panel smoke) that an update like this one needs, and run it here. \
Run the application on the worker desktop :99 (DISPLAY=:99 is set), which the person watches live in a Horizon Device panel. Where the procedure starts its own Xvfb display, run the application on :99 instead, so it stays watchable. \
Freeze the build you test and record its SHA-256. Walk the scenarios and resolutions the procedure requires, and look at each screen. \
Write the evidence to {evidence}: report.md with the build hash, the scenario table with pass or fail per row and what you saw; one PNG screenshot per required resolution; a short recording of the run when the image has a recorder. Use plain file names with letters, digits, dots, dashes and underscores, at most 12 files besides report.md and result.json, each at most 2 MiB. \
Write {evidence}/result.json last, as {{\"outcome\": \"passed\" | \"failed\" | \"blocked\", \"summary\": \"one or two sentences\"}}: passed only when every required scenario passed, failed when one failed, blocked when you could not run the procedure (for example a missing credential or package), with the reason. \
Horizon reads result.json, keeps the evidence and deletes this cloud {limit} minutes after it started at the latest.",
        number = request.number,
        repository = request.repository,
        head = request.head,
        evidence = EVIDENCE,
        limit = TIME_LIMIT.as_secs() / 60,
    )
}

/// Unix seconds now.
#[must_use]
pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests;
