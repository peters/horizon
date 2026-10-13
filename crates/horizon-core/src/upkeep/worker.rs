//! Upkeep on a cloud worker: one exchange installs the loop, keeps its session running,
//! applies Run now and Pause, and reads back what the agent reported. The board shows the
//! result through the status document this module writes on the host.
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::cloud_runtime::{Error, Result, command::Runner, ssh::Connection};

const EXCHANGE: &str = include_str!("worker/exchange.py");
const RUN: &str = include_str!("worker/run.sh");
const SCHEDULER: &str = include_str!("worker/run.py");
const TIMEOUT: Duration = Duration::from_secs(45);
/// The remote command is one argument of the worker's shell, which Linux limits to 128 KiB.
const COMMAND_LIMIT: usize = 120 * 1024;
const REPOSITORY_LIMIT: usize = 50;
const AUTHOR_LIMIT: usize = 20;
const INSTRUCTIONS_LIMIT: usize = 8 * 1024;
const WORKER_LIMIT: usize = 200;
/// The only agent the worker loop runs.
pub const AGENT: &str = "codex";

/// How the worker finds pull requests and checks out code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checkouts {
    /// GitHub search finds the pull requests; a repository is cloned, partially, only when
    /// one of its pull requests needs work. Seconds and little disk.
    #[default]
    OnDemand,
    /// Every repository is cloned partially once and fetched each run.
    Partial,
    /// Every repository is cloned in full, with LFS, and fetched each run.
    Full,
}

impl Checkouts {
    pub const ALL: [Self; 3] = [Self::OnDemand, Self::Partial, Self::Full];

    /// The name tools and saved settings use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::OnDemand => "on_demand",
            Self::Partial => "partial",
            Self::Full => "full",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|checkouts| checkouts.name() == name)
    }

    /// What the agent is told about finding pull requests and cloning.
    fn instructions(self) -> &'static str {
        match self {
            Self::OnDemand => {
                "- Read the open pull requests without cloning: `gh pr list --repo @@REPOSITORY@@ --author <author> --state open --json number,title,url,headRefName,statusCheckRollup,mergeable`, then `gh pr view` or `gh pr checks`. Clone only when a pull request needs a fix, a rebase or a local check, and then partially: `git clone --filter=blob:none https://github.com/@@REPOSITORY@@` into /workspace/home/upkeep/repos/@@REPOSITORY@@.\n"
            }
            Self::Partial => {
                "- Keep a partial clone in /workspace/home/upkeep/repos/@@REPOSITORY@@: `git clone --filter=blob:none https://github.com/@@REPOSITORY@@` the first time, `git fetch --prune` in later runs.\n"
            }
            Self::Full => {
                "- Keep a full clone in /workspace/home/upkeep/repos/@@REPOSITORY@@: `git clone https://github.com/@@REPOSITORY@@` and `git lfs pull` the first time, `git fetch --prune` and `git lfs pull` in later runs.\n"
            }
        }
    }
}

/// What the person chose in New Upkeep. Saved next to the board's status document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// `owner/name` of every repository Upkeep keeps moving.
    pub repositories: Vec<String>,
    /// GitHub logins of the bots whose pull requests it moves, such as `app/dependabot`.
    pub authors: Vec<String>,
    /// The agent that runs; the worker loop runs only [`AGENT`].
    pub agent: String,
    pub every_minutes: u32,
    pub instructions: String,
    /// Shown on the board, such as `Hetzner cx23, Helsinki`.
    #[serde(default)]
    pub worker: String,
    #[serde(default)]
    pub checkouts: Checkouts,
    /// Repositories that go one at a time, in this order, before the others run side by
    /// side, such as the deployments a shared procedure orders.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
}

impl Config {
    /// The repository the cloud is created from: the first one.
    #[must_use]
    pub fn primary(&self) -> Option<&str> {
        self.repositories.first().map(String::as_str)
    }

    /// Adds `names` once each, ignoring case. Returns how many were new.
    pub fn add(&mut self, names: &[String]) -> usize {
        let before = self.repositories.len();
        for name in names {
            let name = name.trim();
            if !name.is_empty() && !self.repositories.iter().any(|held| held.eq_ignore_ascii_case(name)) {
                self.repositories.push(name.to_owned());
            }
        }
        self.repositories.len() - before
    }

    /// Removes `names`, ignoring case. Returns how many were removed.
    pub fn remove(&mut self, names: &[String]) -> usize {
        let before = self.repositories.len();
        self.repositories
            .retain(|held| !names.iter().any(|name| name.trim().eq_ignore_ascii_case(held)));
        before - self.repositories.len()
    }

    /// Checks every value that reaches a command line, a file name or the agent's prompt on
    /// the worker. The instructions are the person's own text and only have a size limit.
    /// # Errors
    /// The first value the worker must not get.
    pub fn validate(&self) -> Result<()> {
        if self.repositories.is_empty() || self.repositories.len() > REPOSITORY_LIMIT {
            return Err(Error::Invalid("Upkeep needs 1 to 50 repositories"));
        }
        if !self
            .repositories
            .iter()
            .all(|name| horizon_cloud::github::valid_repository(name))
        {
            return Err(Error::Invalid("An Upkeep repository is not a GitHub owner/name"));
        }
        // Two agents on one repository would work against each other.
        for (index, name) in self.repositories.iter().enumerate() {
            if self.repositories[..index]
                .iter()
                .any(|held| held.eq_ignore_ascii_case(name))
            {
                return Err(Error::Invalid("An Upkeep repository is listed twice"));
            }
        }
        if !self.order.iter().all(|name| self.repositories.contains(name)) {
            return Err(Error::Invalid(
                "The Upkeep order names a repository that is not in the list",
            ));
        }
        if self.authors.is_empty()
            || self.authors.len() > AUTHOR_LIMIT
            || !self.authors.iter().all(|name| valid_author(name))
        {
            return Err(Error::Invalid("An Upkeep bot is not a GitHub login"));
        }
        if self.agent != AGENT {
            return Err(Error::Invalid("Upkeep runs Codex only"));
        }
        if !(15..=24 * 60).contains(&self.every_minutes) {
            return Err(Error::Invalid("Upkeep runs every 15 minutes to every 24 hours"));
        }
        if self.instructions.len() > INSTRUCTIONS_LIMIT || self.worker.len() > WORKER_LIMIT {
            return Err(Error::Invalid("The Upkeep instructions or worker name are too long"));
        }
        // Control characters cost six bytes each on the way and show as nothing.
        if self
            .instructions
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
            || self.worker.chars().any(char::is_control)
        {
            return Err(Error::Invalid(
                "The Upkeep instructions or worker name hold a control character",
            ));
        }
        Ok(())
    }
}

/// A login that `gh pr list --author` accepts: `name`, `name[bot]` or `app/name`.
fn valid_author(name: &str) -> bool {
    let name = name.strip_prefix("app/").unwrap_or(name);
    let name = name.strip_suffix("[bot]").unwrap_or(name);
    (1..=39).contains(&name.len())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_cloud_id(cloud_id: &str) -> bool {
    !cloud_id.is_empty()
        && cloud_id.len() <= 90
        && cloud_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The tmux session of an Upkeep cloud's loop, which its live panel attaches to.
#[must_use]
pub fn session(cloud_id: &str) -> String {
    format!("upkeep-{cloud_id}")
}

/// `~/.horizon/upkeep/<cloud>`: the saved choice and the status the board reads.
#[must_use]
pub fn directory(cloud_id: &str) -> PathBuf {
    super::default_status_path().with_file_name(cloud_id)
}

/// Saves `config` for `cloud_id`.
/// # Errors
/// A cloud id that is not one folder name, or a folder or file that cannot be written.
pub fn save(cloud_id: &str, config: &Config) -> std::io::Result<PathBuf> {
    if !valid_cloud_id(cloud_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid cloud for Upkeep",
        ));
    }
    let folder = directory(cloud_id);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join("upkeep.json");
    let text = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    write_atomically(&path, &text)?;
    Ok(path)
}

/// Every saved Upkeep: its cloud id and choice.
#[must_use]
pub fn saved() -> Vec<(String, Config)> {
    let root = super::default_status_path().with_file_name("");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join("upkeep.json")).ok()?;
            let config = serde_json::from_str(&text).ok()?;
            Some((entry.file_name().to_string_lossy().into_owned(), config))
        })
        .collect()
}

/// What the board asked for since the last exchange.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub run_now: bool,
    pub paused: Option<bool>,
}

/// The instructions of one repository's agent in a run. The scheduler on the worker fills in
/// `@@REPOSITORY@@` and `@@STATUS@@`; agents for other repositories run at the same time.
#[must_use]
pub fn prompt(config: &Config) -> String {
    let others = config.repositories.len().saturating_sub(1);
    let authors = config.authors.join(", ");
    format!(
        "{instructions}\n\n\
## This run\n\n\
Repository: @@REPOSITORY@@. Work only in this repository: agents for the {others} other repositories of this Upkeep run at the same time.\n\n\
Pull requests opened by: {authors}.\n\n\
## How to work on this worker\n\n\
{checkouts}\
- The worker's Git proxy signs you in for every `https://github.com/` URL.\n\
- If you cannot reach @@REPOSITORY@@ yet, first call the `github_access` tool for it with push access. Horizon allows it within seconds. When a call times out or reports that the request still waits, call it again: the same request comes back. Never give up because of a timeout. For a repository outside this Upkeep, such as one that holds shared rules linked from an AGENTS.md, ask with read access only; the person decides it.\n\
- When a pull request page, a preview or an app run helps, open it in a browser panel with the horizon-browser tools so the person can watch.\n\
- Keep exactly one comment of yours on each pull request you worked on. Its first line is `<!-- upkeep -->`. Find it by that marker with `gh api repos/@@REPOSITORY@@/issues/<number>/comments` and edit it with `gh api --method PATCH` instead of adding another. In one or two sentences say where the pull request stands and why: merged, waiting for checks, or blocked and on what. Never mention other repositories or their pull requests in it.\n\
- After every change, write @@STATUS@@ again: write a temporary file next to it, then move it into place. Use this JSON, for @@REPOSITORY@@ only:\n\
  {{\"pull_requests\": [{{\"repository\": \"owner/name\", \"number\": 1, \"title\": \"...\", \"url\": \"https://github.com/owner/name/pull/1\", \"author\": \"dependabot\", \"checks\": \"passed|failed|running|none\", \"state\": \"blocked|fixing|needs_review|waiting_checks|ready|merged\", \"note\": \"one short sentence\", \"updated_at\": \"RFC 3339\"}}], \"activity\": [{{\"at\": \"RFC 3339\", \"text\": \"one short sentence\"}}]}}\n\
  List every open pull request from these authors, and the ones you merged in this run with state merged.\n\
- Stop when no pull request can move further. End with one line that says what you merged, fixed and left.\n",
        instructions = config.instructions.trim(),
        checkouts = config.checkouts.instructions(),
    )
}

/// The remote command for one exchange. It runs as the account that owns the agent's home:
/// through `horizon-worker-tailnet agent` where the worker isolates agents, so root never
/// writes into a folder the agent controls.
/// # Errors
/// Refuses a cloud id or revision that could not name a worker session, settings that
/// [`Config::validate`] refuses, and a command too long for the worker's shell.
pub fn command(cloud_id: &str, revision: &str, config: &Config, signals: Signals) -> Result<String> {
    let valid_revision = matches!(revision.len(), 40 | 64)
        && revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !valid_cloud_id(cloud_id) || !valid_revision {
        return Err(Error::Invalid("Invalid cloud for Upkeep"));
    }
    config.validate()?;
    let payload = serde_json::json!({
        "session": session(cloud_id),
        "revision": revision,
        "config": config,
        "prompt": prompt(config),
        "run": RUN,
        "scheduler": SCHEDULER,
        "run_now": signals.run_now,
        "paused": signals.paused,
    });
    let engine = base64::engine::general_purpose::STANDARD;
    let script = engine.encode(EXCHANGE);
    let payload = engine.encode(payload.to_string());
    let command = format!(
        "launcher=python3; if [ -f /run/horizon-tailnet/agent-isolation ]; then launcher='horizon-worker-tailnet agent python3'; fi; \
printf %s '{script}' | base64 -d | $launcher -I - '{payload}'"
    );
    if command.len() > COMMAND_LIMIT {
        return Err(Error::Invalid("The Upkeep settings are too long for the worker"));
    }
    Ok(command)
}

/// What one exchange read back.
#[derive(Debug)]
pub struct Exchange {
    /// Whether the loop's session runs.
    pub loop_running: bool,
    /// Whether it already ran before this exchange, so a panel can attach to it.
    pub settled: bool,
    status: Option<String>,
    runs: Vec<serde_json::Value>,
    next_run: Option<i64>,
    pub paused: bool,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "loop")]
    loop_running: bool,
    #[serde(default)]
    settled: bool,
    status: Option<String>,
    #[serde(default)]
    runs: Vec<serde_json::Value>,
    next_run: Option<i64>,
    #[serde(default)]
    paused: bool,
}

/// Runs one exchange on the worker behind `connection`.
/// # Errors
/// A failed connection or output that is not one exchange.
pub fn exchange(
    connection: &Connection,
    runner: &Runner<'_>,
    cloud_id: &str,
    revision: &str,
    config: &Config,
    signals: Signals,
) -> Result<Exchange> {
    let remote = command(cloud_id, revision, config, signals)?;
    let output = runner.run_parsed("Upkeep exchange", &mut connection.pinned_command(&remote), TIMEOUT)?;
    parse(&output)
}

/// Parses what the exchange printed.
/// # Errors
/// Output that is not one exchange object.
pub fn parse(output: &str) -> Result<Exchange> {
    let raw: Raw =
        serde_json::from_str(output.trim()).map_err(|_| Error::Invalid("The worker's Upkeep answer is malformed"))?;
    Ok(Exchange {
        loop_running: raw.loop_running,
        settled: raw.settled,
        status: raw.status,
        runs: raw.runs,
        next_run: raw.next_run,
        paused: raw.paused,
    })
}

impl Exchange {
    /// Whether the loop's last recorded run is still going.
    #[must_use]
    pub fn running(&self) -> bool {
        self.runs
            .last()
            .and_then(|run| run.get("outcome"))
            .and_then(serde_json::Value::as_str)
            == Some("running")
    }

    /// The board's status document: the schedule and runs the loop recorded, the worker,
    /// and the pull requests and activity the agent reported. A report that is not valid
    /// JSON is left out rather than shown half.
    #[must_use]
    pub fn document(&self, config: &Config) -> serde_json::Value {
        let reported: serde_json::Value = self
            .status
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let rfc3339 = |seconds: i64| {
            time::OffsetDateTime::from_unix_timestamp(seconds)
                .ok()
                .and_then(|at| at.format(&time::format_description::well_known::Rfc3339).ok())
        };
        // The loop writes a start record and later its outcome; the last one for a start wins.
        let mut runs: Vec<(i64, serde_json::Value)> = Vec::new();
        for record in &self.runs {
            let Some(started) = record.get("started_at").and_then(serde_json::Value::as_i64) else {
                continue;
            };
            let outcome = record.get("outcome").and_then(|v| v.as_str()).unwrap_or("done");
            let mut run = serde_json::json!({ "started_at": rfc3339(started), "outcome": outcome });
            if let Some(finished) = record.get("finished_at").and_then(serde_json::Value::as_i64) {
                run["finished_at"] = rfc3339(finished).into();
            }
            if let Some(slot) = runs.iter_mut().find(|(at, _)| *at == started) {
                slot.1 = run;
            } else {
                runs.push((started, run));
            }
        }
        serde_json::json!({
            "version": 1,
            "agent": config.agent,
            "worker": {"name": config.worker, "state": if self.loop_running { "connected" } else { "stopped" }},
            "schedule": {
                "every_minutes": config.every_minutes,
                "next_run_at": self.next_run.and_then(rfc3339),
                "paused": self.paused,
            },
            "repositories": config.repositories,
            "runs": runs.into_iter().map(|(_, run)| run).collect::<Vec<_>>(),
            "pull_requests": reported.get("pull_requests").cloned().unwrap_or_else(|| serde_json::json!([])),
            "activity": reported.get("activity").cloned().unwrap_or_else(|| serde_json::json!([])),
        })
    }
}

/// Writes `text` to `path` through a temporary file, so the board never reads half of it.
/// # Errors
/// The file cannot be written or moved into place.
pub fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            repositories: vec!["acme/web".into(), "acme/api".into()],
            authors: vec!["app/dependabot".into()],
            agent: "codex".into(),
            every_minutes: 60,
            instructions: "Follow AGENTS.md.".into(),
            worker: "Hetzner cx23".into(),
            checkouts: Checkouts::OnDemand,
            order: Vec::new(),
        }
    }

    #[test]
    fn repositories_are_added_once_and_removed_ignoring_case() {
        let mut config = config();
        assert_eq!(
            config.add(&["Acme/Web".into(), "acme/docs".into(), "acme/docs".into()]),
            1
        );
        assert_eq!(config.repositories, ["acme/web", "acme/api", "acme/docs"]);
        assert_eq!(config.remove(&["ACME/API".into(), "acme/missing".into()]), 1);
        assert_eq!(config.repositories, ["acme/web", "acme/docs"]);
    }

    #[test]
    fn the_first_repository_starts_the_cloud_and_clones_follow_the_choice() {
        let mut config = config();
        assert_eq!(config.primary(), Some("acme/web"));
        assert!(prompt(&config).contains("Clone only when a pull request needs a fix"));
        config.checkouts = Checkouts::Full;
        assert!(prompt(&config).contains("git lfs pull"));
        assert_eq!(Checkouts::from_name("partial"), Some(Checkouts::Partial));
    }

    #[test]
    fn the_command_refuses_names_that_could_escape_the_session() {
        let revision = "a".repeat(40);
        assert!(command("abc-123", &revision, &config(), Signals::default()).is_ok());
        assert!(command("abc;rm", &revision, &config(), Signals::default()).is_err());
        assert!(command("abc", "HEAD", &config(), Signals::default()).is_err());
    }

    #[test]
    fn settings_that_could_reach_a_command_line_or_path_are_refused() {
        assert!(config().validate().is_ok());
        let refused = |change: fn(&mut Config)| {
            let mut config = config();
            change(&mut config);
            config.validate().is_err()
        };
        assert!(refused(|config| config.repositories.clear()));
        assert!(refused(|config| config.repositories.push("--repo=evil/x".into())));
        assert!(refused(|config| config.repositories.push("acme/..".into())));
        assert!(refused(|config| config.repositories.push("acme/web/../x".into())));
        assert!(refused(|config| config.repositories.push("acme/a b".into())));
        assert!(refused(|config| config.repositories.push("ACME/Web".into())));
        assert!(refused(
            |config| config.repositories = (0..=REPOSITORY_LIMIT).map(|n| format!("acme/r{n}")).collect()
        ));
        assert!(refused(|config| config.order.push("acme/other".into())));
        assert!(refused(|config| config.authors.clear()));
        assert!(refused(|config| config.authors.push("--state=closed".into())));
        assert!(refused(|config| config.authors.push("app/".into())));
        assert!(refused(|config| config.authors.push("bot;id".into())));
        assert!(refused(|config| config.agent = "claude".into()));
        assert!(refused(|config| config.every_minutes = 5));
        assert!(refused(
            |config| config.instructions = "x".repeat(INSTRUCTIONS_LIMIT + 1)
        ));
        assert!(refused(|config| config.worker = "x".repeat(WORKER_LIMIT + 1)));
        assert!(refused(|config| config.instructions = "Merge.\u{1b}[2J".into()));
        assert!(refused(|config| config.worker = "Hetzner\n".into()));
        let mut config = config();
        config.authors = vec!["app/dependabot".into(), "renovate[bot]".into(), "dependabot".into()];
        config.order = vec!["acme/api".into()];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn the_command_runs_as_the_agent_where_the_worker_isolates_it() {
        let remote = command("abc-123", &"a".repeat(40), &config(), Signals::default()).unwrap();
        // The worker procedure in docs/testing/procedures/upkeep.md runs this exact command.
        if let Some(path) = std::env::var_os("HORIZON_TEST_UPKEEP_COMMAND") {
            std::fs::write(path, &remote).unwrap();
        }
        assert!(remote.starts_with(
            "launcher=python3; if [ -f /run/horizon-tailnet/agent-isolation ]; then launcher='horizon-worker-tailnet agent python3'; fi;"
        ));
        assert!(remote.contains("| $launcher -I - '"));
        // The longest settings Config::validate accepts, in the characters that escaping makes
        // longest, still fit the worker's shell.
        let mut long = config();
        long.instructions = "\"".repeat(INSTRUCTIONS_LIMIT);
        long.worker = "\\".repeat(WORKER_LIMIT);
        long.repositories = (0..REPOSITORY_LIMIT)
            .map(|n| format!("{}/{n:0>100}", "o".repeat(39)))
            .collect();
        long.order.clone_from(&long.repositories);
        long.authors = (0..AUTHOR_LIMIT).map(|n| format!("app/{n:0>39}[bot]")).collect();
        assert!(long.validate().is_ok());
        assert!(command("abc-123", &"a".repeat(40), &long, Signals::default()).is_ok());
        assert!(command("abc-123", &"A".repeat(40), &config(), Signals::default()).is_err());
    }

    #[test]
    fn a_cloud_id_that_is_not_one_folder_name_is_never_saved() {
        assert!(save("../escape", &config()).is_err());
        assert!(save("", &config()).is_err());
    }

    #[test]
    fn the_default_instructions_lead_the_prompt() {
        let mut config = config();
        config.instructions = crate::upkeep::DEFAULT_INSTRUCTIONS.into();
        let prompt = prompt(&config);
        assert!(prompt.starts_with("Keep dependency pull requests moving"));
        assert!(prompt.contains("Pull requests opened by: app/dependabot."));
        assert!(prompt.contains("--match-head-commit"));
    }

    #[test]
    fn the_prompt_is_a_template_for_one_repository() {
        let prompt = prompt(&config());
        assert!(prompt.starts_with("Follow AGENTS.md."));
        assert!(prompt.contains("Repository: @@REPOSITORY@@."));
        assert!(prompt.contains("the 1 other repositories"));
        assert!(prompt.contains("write @@STATUS@@ again"));
        assert!(prompt.contains("github_access"));
    }

    #[test]
    fn the_document_keeps_the_last_record_of_each_run() {
        let exchange = parse(
            r#"{"loop": true, "paused": false, "next_run": 1800000000,
                "status": "{\"pull_requests\": [{\"repository\": \"acme/web\"}]}",
                "runs": [{"started_at": 1799996400, "outcome": "running"},
                         {"started_at": 1799996400, "finished_at": 1799997000, "outcome": "done"}]}"#,
        )
        .unwrap();
        let document = exchange.document(&config());
        assert_eq!(document["runs"].as_array().unwrap().len(), 1);
        assert_eq!(document["runs"][0]["outcome"], "done");
        assert_eq!(document["pull_requests"][0]["repository"], "acme/web");
        assert_eq!(document["schedule"]["every_minutes"], 60);
    }

    #[test]
    fn a_report_that_is_not_json_is_left_out() {
        let exchange = parse(r#"{"loop": false, "status": "{half", "runs": [], "next_run": null}"#).unwrap();
        assert_eq!(exchange.document(&config())["pull_requests"], serde_json::json!([]));
    }
}
