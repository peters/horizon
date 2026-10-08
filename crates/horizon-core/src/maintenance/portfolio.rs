//! The repository portfolio read from a worker's status document. The document is
//! external input: missing or unexpected fields fall back to neutral values.

use serde_json::Value;

/// How a state reads, for the view to map onto its theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Active,
    Warning,
    Danger,
    /// Readable but unremarkable, such as a queued repository.
    Neutral,
    /// De-emphasized, such as an idle repository.
    Quiet,
    /// The weakest tier, such as a paused pull request.
    Muted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Attention,
    Active,
    Queued,
    Complete,
}

impl Filter {
    pub const ALL: [Self; 5] = [Self::All, Self::Attention, Self::Active, Self::Queued, Self::Complete];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All repositories",
            Self::Attention => "Needs attention",
            Self::Active => "Working",
            Self::Queued => "Queued",
            Self::Complete => "Complete",
        }
    }

    #[must_use]
    pub fn status(self) -> Option<RepositoryStatus> {
        match self {
            Self::All => None,
            Self::Attention => Some(RepositoryStatus::Attention),
            Self::Active => Some(RepositoryStatus::Active),
            Self::Queued => Some(RepositoryStatus::Queued),
            Self::Complete => Some(RepositoryStatus::Complete),
        }
    }

    #[must_use]
    pub fn count(self, counts: &Counts, total: usize) -> usize {
        match self {
            Self::All => total,
            Self::Attention => counts.attention,
            Self::Active => counts.active,
            Self::Queued => counts.queued,
            Self::Complete => counts.complete,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryStatus {
    Attention,
    Active,
    Queued,
    Complete,
    Idle,
    Disabled,
}

impl RepositoryStatus {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Attention => "Attention",
            Self::Active => "Working",
            Self::Queued => "Queued",
            Self::Complete => "Complete",
            Self::Idle => "Idle",
            Self::Disabled => "Paused",
        }
    }

    #[must_use]
    pub fn tone(self) -> Tone {
        match self {
            Self::Attention => Tone::Warning,
            Self::Active => Tone::Active,
            Self::Complete => Tone::Good,
            Self::Queued => Tone::Neutral,
            Self::Idle | Self::Disabled => Tone::Quiet,
        }
    }
}

pub struct Repository<'a> {
    pub data: &'a Value,
    pub name: &'a str,
    pub ecosystems: Vec<&'a str>,
    pub groups: Vec<&'a str>,
    pub prs: Vec<&'a Value>,
    pub status: RepositoryStatus,
}

impl Repository<'_> {
    #[must_use]
    pub fn open_prs(&self) -> usize {
        self.prs.iter().filter(|pr| is_open(pr)).count()
    }

    /// `("example", "sample-web")`, or an empty owner when the name has none.
    #[must_use]
    pub fn owner_and_name(&self) -> (&str, &str) {
        self.name.rsplit_once('/').unwrap_or(("", self.name))
    }

    /// Whether the repository's name, ecosystems or pull request titles contain `search`.
    #[must_use]
    pub fn matches(&self, filter: Filter, search: &str) -> bool {
        let search = search.trim().to_lowercase();
        filter.status().is_none_or(|status| self.status == status)
            && (search.is_empty()
                || self.name.to_lowercase().contains(&search)
                || self
                    .ecosystems
                    .iter()
                    .any(|ecosystem| ecosystem.to_lowercase().contains(&search))
                || self
                    .prs
                    .iter()
                    .any(|pr| text(pr, "title", "").to_lowercase().contains(&search)))
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Counts {
    pub open_prs: usize,
    pub attention: usize,
    pub active: usize,
    pub queued: usize,
    pub complete: usize,
}

/// Where a pull request is in the worker's pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrStage {
    Verified,
    Working,
    Queued,
    Blocked,
    Failed,
    Paused,
}

impl PrStage {
    pub const ORDER: [Self; 6] = [
        Self::Verified,
        Self::Working,
        Self::Queued,
        Self::Blocked,
        Self::Failed,
        Self::Paused,
    ];

    #[must_use]
    pub fn of(pr: &Value) -> Self {
        match pr.get("state").and_then(Value::as_str) {
            Some("merged") => return Self::Verified,
            Some("closed") => return Self::Paused,
            _ => {}
        }
        let status = text(pr, "status", "Queued").to_lowercase();
        if status.contains("merged") || matches!(status.as_str(), "verified" | "complete" | "completed") {
            Self::Verified
        } else if status == "queued" {
            Self::Queued
        } else if status == "blocked" {
            Self::Blocked
        } else if status.contains("failed") {
            Self::Failed
        } else if matches!(status.as_str(), "disabled" | "paused") || status.contains("closed") {
            Self::Paused
        } else {
            Self::Working
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Working => "in progress",
            Self::Queued => "queued",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Paused => "paused",
        }
    }

    #[must_use]
    pub fn tone(self) -> Tone {
        match self {
            Self::Verified => Tone::Good,
            Self::Working => Tone::Active,
            Self::Queued => Tone::Quiet,
            Self::Blocked => Tone::Warning,
            Self::Failed => Tone::Danger,
            Self::Paused => Tone::Muted,
        }
    }
}

/// Pull requests per [`PrStage::ORDER`] entry.
#[must_use]
pub fn pr_stages(status: &Value) -> [usize; 6] {
    let mut stages = [0; 6];
    for pr in status.get("prs").and_then(Value::as_array).into_iter().flatten() {
        let stage = PrStage::of(pr);
        if let Some(index) = PrStage::ORDER.iter().position(|candidate| *candidate == stage) {
            stages[index] += 1;
        }
    }
    stages
}

#[must_use]
pub fn repositories(status: &Value) -> Vec<Repository<'_>> {
    let prs = status.get("prs").and_then(Value::as_array);
    status
        .get("repos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|data| {
            let name = data.get("repository")?.as_str()?;
            let linked: Vec<_> = prs
                .into_iter()
                .flatten()
                .filter(|pr| text(pr, "repository", "") == name)
                .collect();
            let state = repository_status(data, &linked, status.get("active"));
            Some(Repository {
                data,
                name,
                ecosystems: strings(data.get("ecosystems")),
                groups: strings(data.get("groups")),
                prs: linked,
                status: state,
            })
        })
        .collect()
}

fn repository_status(repo: &Value, prs: &[&Value], active: Option<&Value>) -> RepositoryStatus {
    if repo.get("enabled").and_then(Value::as_bool) == Some(false) {
        return RepositoryStatus::Disabled;
    }
    match text(repo, "status", "").to_lowercase().as_str() {
        "blocked" | "attention" | "failed" => RepositoryStatus::Attention,
        "active" | "running" => RepositoryStatus::Active,
        "queued" => RepositoryStatus::Queued,
        "complete" | "completed" | "done" => RepositoryStatus::Complete,
        "disabled" | "paused" => RepositoryStatus::Disabled,
        "idle" => RepositoryStatus::Idle,
        _ => {
            if active.is_some_and(|active| text(active, "repository", "") == text(repo, "repository", "")) {
                RepositoryStatus::Active
            } else if prs.iter().any(|pr| is_open(pr)) {
                RepositoryStatus::Queued
            } else {
                RepositoryStatus::Idle
            }
        }
    }
}

#[must_use]
pub fn counts(repositories: &[Repository<'_>]) -> Counts {
    let mut counts = Counts::default();
    for repo in repositories {
        counts.open_prs += repo.open_prs();
        match repo.status {
            RepositoryStatus::Attention => counts.attention += 1,
            RepositoryStatus::Active => counts.active += 1,
            RepositoryStatus::Queued => counts.queued += 1,
            RepositoryStatus::Complete => counts.complete += 1,
            RepositoryStatus::Idle | RepositoryStatus::Disabled => {}
        }
    }
    counts
}

/// Keeps a selection across status updates; drops it when the repository disappears,
/// or when the person changed the search or filter and it no longer matches.
#[must_use]
pub fn keep_selection(
    selected: Option<&str>,
    repositories: &[Repository<'_>],
    visible: &[&Repository<'_>],
    criteria_changed: bool,
) -> bool {
    let Some(selected) = selected else {
        return false;
    };
    let exists = repositories.iter().any(|repo| repo.name == selected);
    let matches = visible.iter().any(|repo| repo.name == selected);
    exists && (!criteria_changed || matches)
}

#[must_use]
pub fn is_open(pr: &Value) -> bool {
    if let Some(state) = pr.get("state").and_then(Value::as_str) {
        return state == "open";
    }
    let status = text(pr, "status", "Queued").to_lowercase();
    !(status.contains("merged")
        || status.contains("closed")
        || matches!(status.as_str(), "verified" | "complete" | "completed"))
}

/// The worker agent's health, in words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub label: &'static str,
    pub tone: Tone,
}

/// Health as reported, unless SSH is down: then a cached green state would mislead.
#[must_use]
pub fn reported_health(status: &Value) -> Health {
    if status.get("ssh_connected").and_then(Value::as_bool) == Some(false) {
        return Health {
            label: "Agent unknown · SSH unavailable",
            tone: Tone::Warning,
        };
    }
    health(&status["worker_health"])
}

#[must_use]
pub fn health(worker: &Value) -> Health {
    let stale = heartbeat_age(worker).is_some_and(|age| age > 12.0);
    let alive = worker.get("alive").and_then(Value::as_bool) == Some(true);
    let (label, tone) = match text(worker, "state", "") {
        "stopped" => ("Agent stopped", Tone::Quiet),
        "error" => ("Agent needs attention", Tone::Warning),
        "not_started" => ("Agent not started", Tone::Quiet),
        "starting" => ("Agent starting", Tone::Quiet),
        "unresponsive" => ("Agent unresponsive", Tone::Warning),
        _ if !alive || stale => ("Agent unresponsive", Tone::Warning),
        "working" => ("Agent online · working", Tone::Good),
        _ => ("Agent online · idle", Tone::Good),
    };
    Health { label, tone }
}

/// The heartbeat age, unless SSH is down: a cached age would read as current.
#[must_use]
pub fn reported_heartbeat(status: &Value) -> Option<f64> {
    if status.get("ssh_connected").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    heartbeat_age(&status["worker_health"])
}

fn heartbeat_age(worker: &Value) -> Option<f64> {
    worker
        .get("heartbeat_age_seconds")
        .and_then(Value::as_f64)
        .filter(|age| age.is_finite() && *age >= 0.0)
}

/// "just now", "12s ago", "31 min ago", "2 h ago" or "3 d ago".
#[must_use]
pub fn ago(seconds: f64) -> String {
    if seconds < 2.0 {
        "just now".to_owned()
    } else if seconds < 60.0 {
        format!("{:.0}s ago", seconds.floor())
    } else if seconds < 3600.0 {
        format!("{:.0} min ago", (seconds / 60.0).floor())
    } else if seconds < 86_400.0 {
        format!("{:.0} h ago", (seconds / 3600.0).floor())
    } else {
        format!("{:.0} d ago", (seconds / 86_400.0).floor())
    }
}

#[must_use]
pub fn strings(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|text| !text.is_empty())
        .collect()
}

/// `13:27:31` from an RFC 3339 timestamp, or the text as reported.
#[must_use]
pub fn clock(timestamp: &str) -> &str {
    if timestamp.len() >= 19 && timestamp.as_bytes().get(10) == Some(&b'T') {
        timestamp.get(11..19).unwrap_or(timestamp)
    } else {
        timestamp
    }
}

#[must_use]
pub fn text<'a>(value: &'a Value, field: &str, fallback: &'a str) -> &'a str {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
}

/// Only the HTTPS GitHub page of the pull request the worker reported, never another URL.
#[must_use]
pub fn github_pull_request_url(pr: &Value) -> Option<&str> {
    let url = pr.get("url")?.as_str()?;
    let path = url.strip_prefix("https://github.com/")?;
    let repository = pr.get("repository")?.as_str()?;
    let number = pr.get("number")?.as_u64()?;
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if !valid_repository_part(owner)
        || !valid_repository_part(name)
        || parts.next()? != "pull"
        || parts.next()?.parse::<u64>().ok()? != number
        || parts.next().is_some()
        || repository != format!("{owner}/{name}")
    {
        return None;
    }
    Some(url)
}

fn valid_repository_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
