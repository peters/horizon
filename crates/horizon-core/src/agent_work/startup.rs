use std::cell::Cell;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::PanelKind;
use crate::horizon_home::HorizonHome;
use crate::panel::current_unix_millis;

use super::{
    AskReason, RestartDecision, RestartEvidence, StoredWork, TranscriptSnapshot, TurnState, WorkLaunch, WorkStore,
};

pub(crate) fn configured_resume_limit() -> usize {
    match std::env::var("HORIZON_WORK_RESUME_LIMIT") {
        Ok(value) => parse_resume_limit(&value),
        Err(std::env::VarError::NotPresent) => 3,
        Err(std::env::VarError::NotUnicode(_)) => 0,
    }
}

fn parse_resume_limit(value: &str) -> usize {
    value.parse::<usize>().ok().filter(|limit| *limit <= 32).unwrap_or(0)
}

thread_local! {
    static REMAINING: Cell<usize> = const { Cell::new(0) };
}

/// The budget is scoped to one synchronous board restore, including error paths.
/// Ordinary panel restarts never inherit permission for unattended work.
pub(crate) struct RestoreBudget(usize);

impl RestoreBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self(REMAINING.replace(limit))
    }
}

impl Drop for RestoreBudget {
    fn drop(&mut self) {
        REMAINING.set(self.0);
    }
}

#[derive(Default)]
pub(crate) struct WorkContinuation {
    reason: Option<AskReason>,
    touched: AtomicBool,
}

impl WorkContinuation {
    pub(crate) fn pending(&self) -> Option<AskReason> {
        (!self.touched.load(Ordering::Acquire)).then_some(self.reason).flatten()
    }

    pub(crate) fn note_input(&self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.touched.store(true, Ordering::Release);
        }
    }

    fn ask(reason: AskReason) -> Self {
        Self {
            reason: Some(reason),
            ..Self::default()
        }
    }
}

pub(crate) struct StartupPlan {
    pub(crate) owner: Option<std::sync::Arc<super::WorkOwner>>,
    pub(crate) state: WorkContinuation,
    seed: Option<String>,
}

impl WorkLaunch<'_> {
    pub(crate) fn start(
        &self,
        program: &str,
        is_restore: bool,
        unambiguous: bool,
        args: &mut [String],
        env: &mut std::collections::HashMap<String, String>,
    ) -> StartupPlan {
        let owned = unambiguous.then(|| self.owned_command(program, args, env)).flatten();
        let mut plan = StartupPlan::prepare(self, is_restore, unambiguous && owned.is_some());
        plan.owner = self.attach(owned.as_deref(), args, env);
        plan.seed_if_attached(plan.owner.is_some(), args);
        plan
    }
}

impl StartupPlan {
    pub(crate) fn prepare(launch: &WorkLaunch<'_>, is_restore: bool, unambiguous: bool) -> Self {
        let (decision, seed) = if is_restore && launch.policy.enabled && launch.default_command {
            if unambiguous {
                inspect(launch).unwrap_or((RestartDecision::Ask(AskReason::MissingEvidence), None))
            } else {
                (RestartDecision::Ask(AskReason::MissingEvidence), None)
            }
        } else {
            (RestartDecision::NotResumable, None)
        };
        Self {
            owner: None,
            state: match decision {
                RestartDecision::Ask(reason) => WorkContinuation::ask(reason),
                _ => WorkContinuation::default(),
            },
            seed,
        }
    }

    pub(crate) fn seed_if_attached(&mut self, attached: bool, args: &mut [String]) {
        if let Some(seed) = self.seed.take() {
            if attached && append_seed(PanelKind::Claude, args, &seed) {
                REMAINING.set(REMAINING.get().saturating_sub(1));
            } else {
                self.state = WorkContinuation::ask(AskReason::MissingEvidence);
            }
        }
    }
}

fn inspect(launch: &WorkLaunch<'_>) -> Option<(RestartDecision, Option<String>)> {
    if !launch.kind.is_agent() {
        return Some((RestartDecision::NotResumable, None));
    }
    if launch.kind != PanelKind::Claude {
        return Some((RestartDecision::Ask(AskReason::MissingEvidence), None));
    }
    let session = launch.session_id?;
    let home = std::env::var_os("HOME")?;
    inspect_at(
        launch,
        session,
        Path::new(&home),
        &WorkStore::new(HorizonHome::resolve().root()),
    )
}

fn inspect_at(
    launch: &WorkLaunch<'_>,
    session: &str,
    home: &Path,
    store: &WorkStore,
) -> Option<(RestartDecision, Option<String>)> {
    let live = super::discovery::session_presence(home, session).ok();
    if live == Some(super::discovery::Presence::Live) {
        return Some((RestartDecision::NotResumable, None));
    }
    let cwd = launch
        .cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())?
        .canonicalize()
        .ok()?;
    let record = store.read(launch.panel).ok()?;
    if record
        .as_ref()
        .is_some_and(|r| r.kind != launch.kind || r.ledger.session_id != session)
    {
        return Some((RestartDecision::NotResumable, None));
    }
    let transcript_path = record
        .as_ref()
        .filter(|r| r.transcript_path.is_file())
        .map(|r| r.transcript_path.clone())
        .or_else(|| super::discovery::transcript(home, session))?;
    let transcript = TranscriptSnapshot::read(&transcript_path, launch.kind).ok()?;
    // A handoff belongs to its recorded turn. A later question is independent
    // evidence for manual review, even after that handoff has been consumed.
    let waiting_for_user = transcript.state == TurnState::Blocked;
    if !waiting_for_user && record.as_ref().is_some_and(|record| record.consumed) {
        return Some((RestartDecision::NotResumable, None));
    }
    let fingerprint = super::repository::fingerprint(&cwd);
    let now = current_unix_millis();
    let evidence = RestartEvidence {
        kind: launch.kind,
        panel_local_id: launch.panel,
        session_id: session,
        cwd: cwd.to_str()?,
        policy: launch.policy,
        handoff: record
            .as_ref()
            .filter(|_| !waiting_for_user)
            .and_then(|r| r.handoff.as_ref()),
        ledger: record.as_ref().map(|r| &r.ledger),
        transcript: Some(&transcript),
        repo_fingerprint: fingerprint.as_deref(),
        session_live_elsewhere: false,
        stale_live_session: live == Some(super::discovery::Presence::Stale),
        now_millis: now,
    };
    let mut decision = evidence.classify(REMAINING.get());
    if decision == RestartDecision::Resume && (!cfg!(target_os = "linux") || live.is_none()) {
        decision = RestartDecision::Ask(AskReason::MissingEvidence);
    }
    let seed = if decision == RestartDecision::Resume {
        let record = record.as_ref()?;
        // Re-read the live registry after repository inspection and claim the
        // exact persisted turn before any provider can submit its seeded prompt.
        if super::discovery::session_presence(home, session).ok()? == super::discovery::Presence::Live {
            return Some((RestartDecision::NotResumable, None));
        }
        if !store.claim_handoff(record).ok()? {
            return None;
        }
        Some(resume_brief(record, now))
    } else {
        None
    };
    Some((decision, seed))
}

/// A factual brief deliberately makes no claims about tool side effects or
/// surviving background processes. It never grants additional permissions.
#[must_use]
pub fn resume_brief(record: &StoredWork, now: i64) -> String {
    let suspended = record
        .handoff
        .as_ref()
        .map_or_else(|| "unknown".into(), |r| r.suspended_at_millis.to_string());
    format!(
        "Horizon restored this conversation at Unix time {now} ms. Work was suspended at Unix time {suspended} ms. Re-read the latest user request and conversation before continuing. Re-verify the working tree, tool side effects, and any background processes; interrupted tools may have partially completed. Continue only the work already authorized by the user. Stop and ask if a permission or user decision is needed."
    )
}

pub(crate) fn append_seed(kind: PanelKind, args: &mut [String], brief: &str) -> bool {
    if args.len() != 2
        || args[0] != "-ic"
        || !matches!(
            kind,
            PanelKind::Claude | PanelKind::Codex | PanelKind::Pi | PanelKind::Grok | PanelKind::OpenCode
        )
    {
        return false;
    }
    args[1].push_str(if kind == PanelKind::OpenCode { " --prompt " } else { " " });
    let _ = write!(args[1], "'{}'", brief.replace('\'', "'\\''"));
    true
}

#[cfg(test)]
mod tests;
