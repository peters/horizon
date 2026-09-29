//! What a production cloud is doing, said once: the sentence and track in its
//! header, the steps in its body and the drawer's Overview all read this.
use super::super::{Runtime, Stage};
use super::{deleted_or_redeploying, deleting, rebuild};
use horizon_core::cloud_runtime::{CreateState, diagnosis, progress};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    /// Nothing is happening and nothing is wrong.
    Idle,
    /// An operation is running.
    Live,
    Ready,
    /// Needs a decision, such as a stopped or unverified worker.
    Attention,
    Failed,
}

/// The one action the header offers for the cloud's state. Everything else is in Manage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel) enum Primary {
    Deploy,
    Reconnect,
    Retry,
    Cancel,
    Resume,
    /// Opens Manage on the stop confirmation.
    Stop,
    ReconcileStop,
    CheckProvider,
    /// Opens Manage on the redeploy confirmation.
    Redeploy,
    /// Opens Manage, where a held operation is finished.
    Manage,
}

impl Primary {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Deploy => "Deploy cloud",
            Self::Reconnect => "Reconnect",
            Self::Retry => "Retry deploy",
            Self::Cancel => "Cancel",
            Self::Resume => "Resume worker",
            Self::Stop => "Stop…",
            Self::ReconcileStop => "Reconcile stop",
            Self::CheckProvider => "Check provider",
            Self::Redeploy => "Redeploy…",
            Self::Manage => "Manage…",
        }
    }

    /// The label of a retry offered beside a failure, when this action retries it.
    pub(super) fn retry_label(self) -> Option<&'static str> {
        self.retries().map(|_| self.label())
    }

    /// The operation that retries a failure, as the header's own button does: a resume or
    /// stop is retried as itself, never as a new deployment.
    pub(super) fn retries(self) -> Option<super::Action> {
        match self {
            Self::Retry | Self::Reconnect => Some(super::Action::Deploy),
            Self::Resume => Some(super::Action::Resume),
            Self::ReconcileStop => Some(super::Action::Stop),
            _ => None,
        }
    }

    /// Filled for the step forward, outlined for the rest.
    pub(super) fn emphasized(self) -> bool {
        matches!(
            self,
            Self::Deploy | Self::Reconnect | Self::Retry | Self::Resume | Self::CheckProvider | Self::Redeploy
        )
    }

    pub(super) fn destructive(self) -> bool {
        self == Self::Cancel
    }
}

/// Why an attempt failed: Horizon's summary, the decisive output line and its meaning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) struct Failure {
    pub summary: String,
    pub cause: Option<String>,
    pub meaning: Option<&'static str>,
}

impl Failure {
    /// Read from the output once per change: every frame asks, the output rarely moves.
    fn of(runtime: &Runtime, summary: &str) -> Self {
        let key = DiagnosisKey {
            summary: summary.to_owned(),
            lines: runtime.logs.len(),
            held: runtime.pending_logs.len(),
            last: runtime
                .pending_logs
                .back()
                .or(runtime.logs.back())
                .map_or(0, |line| line.text.len()),
            generation: runtime.log_generation,
            attempt: runtime.progress.attempt(),
        };
        if let Some((cached, failure)) = runtime.diagnosis.borrow().as_ref()
            && *cached == key
        {
            return failure.clone();
        }
        // Lines held aside while a reader is scrolled up are output too; an earlier
        // attempt's lines are not this failure's.
        let attempt = runtime.progress.attempt();
        let lines = runtime
            .logs
            .iter()
            .chain(&runtime.pending_logs)
            .filter(|line| line.attempt == attempt);
        let found = diagnosis::diagnose(lines.map(|line| line.text.as_str()), summary);
        let failure = Self {
            summary: summary.to_owned(),
            meaning: found
                .as_ref()
                .and_then(|found| found.meaning)
                .or_else(|| diagnosis::meaning(summary)),
            cause: found.map(|found| found.cause),
        };
        *runtime.diagnosis.borrow_mut() = Some((key, failure.clone()));
        failure
    }

    /// The line the header leads with: the cause when found, else the summary.
    pub(super) fn headline(&self) -> &str {
        self.cause.as_deref().unwrap_or(&self.summary)
    }

    /// Text copied by "Copy error".
    pub(super) fn copy_text(&self) -> String {
        match &self.cause {
            Some(cause) => format!("{cause}\n{}", self.summary),
            None => self.summary.clone(),
        }
    }
}

/// What a cached diagnosis was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) struct DiagnosisKey {
    summary: String,
    lines: usize,
    held: usize,
    /// The newest line's length, for output set without `push_log`.
    last: usize,
    /// Output replaced or added through `push_log`.
    generation: u64,
    attempt: u64,
}

/// The stage track along the header's bottom edge.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Track {
    pub stages: &'static [Stage],
    /// Index of the running or failed stage; `None` when nothing started or all finished.
    pub current: Option<usize>,
    /// Stages before this index are finished.
    pub finished: usize,
    /// Measured share of the current stage.
    pub fraction: Option<f32>,
    pub failed: bool,
    /// A stopped or disconnected cloud keeps its finished stages, faded.
    pub faded: bool,
}

impl Track {
    fn at(stages: &'static [Stage], stage: Option<Stage>) -> Self {
        let current = stage.and_then(|stage| stages.iter().position(|each| *each == stage));
        Self {
            stages,
            current,
            finished: current.unwrap_or(0),
            fraction: None,
            failed: false,
            faded: false,
        }
    }

    fn complete(stages: &'static [Stage], faded: bool) -> Self {
        Self {
            stages,
            current: None,
            finished: stages.len(),
            fraction: None,
            failed: false,
            faded,
        }
    }

    fn empty() -> Self {
        Self::at(&Stage::ALL, None)
    }

    /// "Stage 3/8", when a stage is running or failed.
    pub(super) fn position(&self) -> Option<String> {
        self.current
            .map(|index| format!("Stage {}/{}", index + 1, self.stages.len()))
    }
}

pub(in crate::app::cloud_panel::production) struct Status {
    pub(super) tone: Tone,
    /// "Pushing image", "Push failed", "Ready".
    pub(super) verb: String,
    /// Measured numbers, the failure's cause, or the cloud's occupancy.
    pub(super) numbers: String,
    /// ETA, time since, or what a failure means.
    pub(super) tail: String,
    /// The right end of the status line: stage position and elapsed time, or totals.
    pub(super) right: String,
    pub(super) failure: Option<Failure>,
    pub(super) track: Track,
    pub(super) primary: Option<Primary>,
}

impl Status {
    pub(super) fn live(&self) -> bool {
        self.tone == Tone::Live
    }
}

/// Terminal occupancy, counted by the caller from the board.
#[derive(Clone, Copy, Default)]
pub(super) struct Occupancy {
    pub panels: usize,
    pub running: usize,
    pub terminals: usize,
}

fn blank() -> Status {
    Status {
        tone: Tone::Idle,
        verb: String::new(),
        numbers: String::new(),
        tail: String::new(),
        right: String::new(),
        failure: None,
        track: Track::empty(),
        primary: None,
    }
}

/// States that override the deployment's own: an unreadable record, deletion and
/// its aftermath, and a running provider check.
fn exceptional(runtime: &Runtime) -> Option<Status> {
    let base = blank();
    if runtime.state_unavailable {
        return Some(Status {
            tone: Tone::Failed,
            verb: "Record unavailable".into(),
            // Restore keeps the reason the record could not be read.
            numbers: runtime
                .error
                .clone()
                .unwrap_or_else(|| "Deployment state could not be read".into()),
            // A provider check needs the record too; reading it again is what Manage offers.
            tail: "Repair or restore the record, then read it again in Manage".into(),
            ..base
        });
    }
    if deleting(runtime) {
        return Some(deletion(runtime, base));
    }
    if deleted_or_redeploying(runtime) && runtime.receiver.is_none() {
        return Some(Status {
            tone: Tone::Attention,
            verb: "Worker deleted".into(),
            numbers: "Workspace storage cleaned up".into(),
            tail: runtime
                .progress
                .ended_in(Stage::Deleted)
                .map_or_else(String::new, |elapsed| format!("in {}", progress::duration(elapsed))),
            track: Track::complete(&Stage::DELETION, true),
            primary: Some(Primary::Redeploy),
            ..base
        });
    }
    if runtime
        .state
        .as_ref()
        .is_some_and(|state| matches!(state.operation, CreateState::Terminated { .. }))
    {
        return Some(Status {
            tone: Tone::Attention,
            verb: "Worker deleted".into(),
            numbers: "Storage cleanup unfinished · still billable".into(),
            tail: "Open Manage to finish it".into(),
            // The earlier steps are done; cleanup waits for the owner, it is not running.
            track: Track {
                current: None,
                ..Track::at(&Stage::DELETION, Some(Stage::DeleteStorage))
            },
            primary: Some(Primary::Manage),
            ..base
        });
    }
    runtime.recovery_receiver.is_some().then(|| Status {
        tone: Tone::Live,
        verb: "Checking provider".into(),
        numbers: "Confirming the worker's status".into(),
        track: track(runtime),
        ..base
    })
}

/// This frame's status of `runtime`: computed by whichever of the header and the card
/// asks first, then reused. Put it back with [`keep`] after use.
pub(super) fn for_frame(runtime: &mut Runtime, occupancy: Occupancy, now: SystemTime, frame: u64) -> Status {
    match runtime.frame_status.take() {
        Some((computed, status)) if computed == frame => status,
        _ => of(runtime, occupancy, now),
    }
}

pub(super) fn keep(runtime: &mut Runtime, status: Status, frame: u64) {
    runtime.frame_status = Some((frame, status));
}

pub(super) fn of(runtime: &Runtime, occupancy: Occupancy, now: SystemTime) -> Status {
    if let Some(status) = exceptional(runtime) {
        return status;
    }
    let base = blank();
    if runtime.receiver.is_some() && runtime.stage != Some(Stage::Ready) {
        return running(runtime, base);
    }
    if let Some(status) = held(runtime) {
        return status;
    }
    if runtime.receiver.is_some() && runtime.stage == Some(Stage::Ready) {
        return ready(runtime, occupancy, now, base);
    }
    if let Some(error) = &runtime.error {
        return failed(runtime, error);
    }
    match runtime.stage {
        Some(Stage::Stopping) => Status {
            tone: Tone::Attention,
            verb: "Stop requested".into(),
            numbers: "Waiting for the provider to confirm".into(),
            track: Track::complete(&Stage::ALL, true),
            primary: Some(Primary::ReconcileStop),
            ..base
        },
        Some(Stage::Stopped) => Status {
            tone: Tone::Attention,
            verb: "Stopped".into(),
            numbers: "Storage kept · billable".into(),
            tail: super::self_stop::line(runtime.state.as_ref()).unwrap_or_default(),
            right: panels(occupancy.panels),
            track: Track::complete(&Stage::ALL, true),
            primary: Some(Primary::Resume),
            ..base
        },
        _ if runtime.state.is_some() => Status {
            tone: Tone::Idle,
            verb: "Disconnected".into(),
            numbers: "Sessions continue while disconnected".into(),
            right: panels(occupancy.panels),
            track: runtime
                .state
                .as_ref()
                .filter(|state| state.stage == Stage::Ready)
                .map_or_else(|| track(runtime), |_| Track::complete(&Stage::ALL, true)),
            primary: Some(Primary::Reconnect),
            ..base
        },
        _ => Status {
            tone: Tone::Idle,
            verb: "Not deployed".into(),
            numbers: "Nothing is allocated or billed yet".into(),
            primary: Some(Primary::Deploy),
            ..base
        },
    }
}

/// Operations Manage holds before anything else, in its order: a pending resize,
/// a device release, a pending rebuild and an unconfirmed worker.
fn held(runtime: &Runtime) -> Option<Status> {
    let base = blank();
    if runtime.resize.pending.is_some() {
        return Some(Status {
            tone: Tone::Attention,
            verb: "Resize pending".into(),
            numbers: "Finish or retry the resize before other operations".into(),
            track: track(runtime),
            primary: Some(Primary::Manage),
            ..base
        });
    }
    if runtime.remote_release.is_some() {
        return Some(Status {
            tone: Tone::Live,
            verb: "Releasing remote devices".into(),
            numbers: "Stopping hosted browser sessions and removing copied credentials".into(),
            track: track(runtime),
            ..base
        });
    }
    if let Some(error) = &runtime.remote_release_error {
        return Some(Status {
            tone: Tone::Failed,
            verb: "Device release failed".into(),
            numbers: error.clone(),
            failure: Some(Failure {
                summary: error.clone(),
                cause: None,
                meaning: diagnosis::meaning(error),
            }),
            track: track(runtime),
            primary: Some(Primary::Manage),
            ..base
        });
    }
    if rebuild::has_pending(runtime) {
        return Some(Status {
            tone: Tone::Attention,
            verb: "Image rebuild pending".into(),
            numbers: "Continue or cancel it in Manage".into(),
            track: track(runtime),
            primary: Some(Primary::Manage),
            ..base
        });
    }
    runtime
        .state
        .as_ref()
        .is_some_and(Runtime::needs_provider_check)
        .then(|| Status {
            tone: Tone::Attention,
            verb: "Needs provider check".into(),
            numbers: runtime
                .error
                .clone()
                .unwrap_or_else(|| "The worker's status is not confirmed".into()),
            tail: "The check cannot start or delete a worker".into(),
            track: track(runtime),
            primary: Some(Primary::CheckProvider),
            ..base
        })
}

fn panels(count: usize) -> String {
    match count {
        0 => "No panels".into(),
        1 => "1 panel".into(),
        count => format!("{count} panels"),
    }
}

/// The stage list shown now: a deletion's, a rebuild's or the deployment's.
fn stages(runtime: &Runtime) -> &'static [Stage] {
    if runtime.progress.is_deletion() {
        &Stage::DELETION
    } else {
        rebuild::stages(runtime)
    }
}

fn track(runtime: &Runtime) -> Track {
    let stages = stages(runtime);
    let stage = runtime
        .stage
        .or_else(|| runtime.state.as_ref().map(|state| state.stage))
        .filter(|stage| stages.contains(stage))
        // A deletion that failed returns the saved deployment stage; its own last step
        // (or its first, when it failed before reporting one) is where it stopped.
        .or_else(|| {
            runtime
                .progress
                .is_deletion()
                .then(|| runtime.progress.last_stage().unwrap_or(Stage::ReleaseDevices))
        });
    let mut track = Track::at(stages, stage);
    // A step revisited after a later one (the image contract checked under Validate
    // after Build) keeps the later one finished while it runs, not only once it fails.
    if let Some(later) = finished_after(runtime, &track) {
        track.finished = track.finished.max(later + 1);
    }
    track
}

fn verb(stage: Stage) -> &'static str {
    match stage {
        Stage::Validate => "Validating",
        Stage::Build => "Building image",
        Stage::Push => "Pushing image",
        Stage::Replace => "Replacing image",
        Stage::Provision => "Requesting worker",
        Stage::Readiness => "Worker starting",
        Stage::Worktrees => "Preparing worktrees",
        Stage::Sessions => "Starting sessions",
        Stage::Ready => "Ready",
        Stage::Stopped => "Stopped",
        Stage::Stopping => "Stopping",
        Stage::Deleted => "Worker deleted",
        Stage::ReleaseDevices => "Releasing devices",
        Stage::DeleteWorker => "Deleting worker",
        Stage::DeleteStorage => "Deleting storage",
    }
}

/// A failed step is named by its short label: "Push failed".
fn failed_verb(stage: Option<Stage>) -> String {
    let short = match stage {
        Some(Stage::Validate) => "Validation",
        Some(Stage::Build) => "Build",
        Some(Stage::Push) => "Push",
        Some(Stage::Replace) => "Image switch",
        Some(Stage::Provision) => "Provisioning",
        Some(Stage::Readiness) => "Readiness check",
        Some(Stage::Worktrees) => "Worktree setup",
        Some(Stage::Sessions) => "Session start",
        Some(Stage::ReleaseDevices | Stage::DeleteWorker | Stage::DeleteStorage) => "Deletion",
        Some(Stage::Stopping) => "Stop",
        Some(Stage::Ready | Stage::Stopped | Stage::Deleted) | None => "Operation",
    };
    format!("{short} failed")
}

/// "Building image · 1/3 reported steps complete" after the verb "Building image"
/// reads "1/3 reported steps complete".
fn without_verb<'a>(detail: &'a str, verb: &str) -> &'a str {
    detail
        .strip_prefix(verb)
        .map(|rest| rest.trim_start_matches([' ', '·']))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(detail)
}

fn elapsed(runtime: &Runtime) -> Option<String> {
    runtime
        .progress
        .elapsed()
        .map(|elapsed| format!("{} elapsed", progress::duration(elapsed)))
}

fn with_position(track: &Track, detail: Option<String>) -> String {
    [track.position(), detail]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn running(runtime: &Runtime, base: Status) -> Status {
    let mut track = if runtime.stage == Some(Stage::Stopping) {
        // Stopping is not a deployment step: the deployment it stops stays drawn, faded.
        Track::complete(&Stage::ALL, true)
    } else {
        track(runtime)
    };
    let measured = runtime.progress.measured();
    track.fraction = measured.as_ref().and_then(super::super::progress::Measured::fraction);
    let rebuilding = runtime.rebuild.is_some();
    let verb = match runtime.stage {
        Some(Stage::Validate | Stage::Build | Stage::Push) if rebuilding => "Rebuilding image",
        Some(stage) => verb(stage),
        None => "Starting",
    };
    let numbers = measured.as_ref().map_or_else(
        || runtime.progress.activity().unwrap_or_default().to_owned(),
        |measured| {
            let numbers = measured.numbers();
            if numbers.is_empty() {
                without_verb(measured.detail, verb).to_owned()
            } else {
                numbers
            }
        },
    );
    let tail = measured.as_ref().and_then(super::super::progress::Measured::eta);
    Status {
        tone: Tone::Live,
        verb: verb.into(),
        numbers,
        tail: tail.unwrap_or_default(),
        right: with_position(&track, elapsed(runtime)),
        track,
        // A rebuild cancels only before the worker's image switch, as Manage offers it.
        primary: if runtime.rebuild.is_some() {
            rebuild::cancellable(runtime).then_some(Primary::Cancel)
        } else {
            runtime.cancel.is_some().then_some(Primary::Cancel)
        },
        ..base
    }
}

fn deletion(runtime: &Runtime, base: Status) -> Status {
    let track = Track::at(&Stage::DELETION, runtime.stage);
    Status {
        tone: Tone::Live,
        verb: "Deleting cloud resources".into(),
        numbers: runtime
            .progress
            .activity()
            .map_or_else(|| runtime.stage.map_or("", Stage::label).to_owned(), str::to_owned),
        right: with_position(&track, elapsed(runtime)),
        // A sent delete request cannot be recalled; only releasing devices can be cancelled.
        primary: (runtime.stage == Some(Stage::ReleaseDevices)).then_some(Primary::Cancel),
        track,
        ..base
    }
}

/// The furthest stage of `track` this attempt already finished, after `current`.
/// Core reports some checks under an earlier stage, such as the built image's contract
/// under Validate after Build; those steps stay finished rather than looking undone.
fn finished_after(runtime: &Runtime, track: &Track) -> Option<usize> {
    let current = track.current?;
    track
        .stages
        .iter()
        .enumerate()
        .skip(current + 1)
        .filter(|(_, stage)| runtime.progress.stage_duration(**stage).is_some())
        .map(|(index, _)| index)
        .next_back()
}

/// A resume that failed before the provider acted: the record is still the stopped one,
/// whether it was reloaded or the resume never got past its preflight.
fn resume_failed(runtime: &Runtime) -> bool {
    runtime.operation == Some(super::Action::Resume)
        && runtime
            .state
            .as_ref()
            .is_some_and(|state| state.stage == Stage::Stopped)
}

/// A stop that failed, whether or not the provider recorded it as stopping first: a
/// failure before that reloads the record that was Ready.
fn stop_failed(runtime: &Runtime) -> bool {
    runtime.stage == Some(Stage::Stopping) || runtime.operation == Some(super::Action::Stop)
}

fn failed(runtime: &Runtime, error: &str) -> Status {
    let mut track = track(runtime);
    if stop_failed(runtime) {
        // The worker got as far as Ready; the stop is not a deployment step.
        track = Track::complete(&Stage::ALL, true);
    }
    // A reloaded record whose stage is off the track (a stopped one after a failed
    // resume) does not erase the step this attempt tried.
    let attempted = runtime
        .progress
        .last_stage()
        .or_else(|| resume_failed(runtime).then_some(Stage::Provision))
        .filter(|stage| track.stages.contains(stage));
    if track.current.is_none()
        && !runtime.progress.is_deletion()
        && let Some(stage) = attempted
    {
        track = Track::at(track.stages, Some(stage));
    }
    track.failed = track.current.is_some();
    let later = finished_after(runtime, &track);
    let failure = Failure::of(runtime, error);
    let never_ready = runtime.state.as_ref().is_none_or(|state| state.stage != Stage::Ready);
    let primary = if runtime.progress.is_deletion() {
        // Deleting again needs Manage's confirmation.
        Some(Primary::Manage)
    } else if resume_failed(runtime)
        || runtime
            .state
            .as_ref()
            .is_some_and(|state| state.stage == Stage::Stopped)
    {
        // The worker is still stopped: resuming it, not a new deployment.
        Some(Primary::Resume)
    } else if stop_failed(runtime) {
        // A failed stop is finished by confirming it, not by reconnecting the worker.
        Some(Primary::ReconcileStop)
    } else if never_ready {
        Some(Primary::Retry)
    } else {
        Some(Primary::Reconnect)
    };
    Status {
        tone: Tone::Failed,
        verb: if runtime.progress.is_deletion() {
            "Deletion failed".into()
        } else if resume_failed(runtime) {
            "Resume failed".into()
        } else if stop_failed(runtime) {
            "Stop failed".into()
        } else {
            failed_verb(runtime.stage)
        },
        numbers: failure.headline().to_owned(),
        tail: runtime
            .progress
            .elapsed()
            .map_or_else(String::new, |elapsed| format!("after {}", progress::duration(elapsed))),
        right: match (track.current, later) {
            (_, Some(later)) => format!("Failed after {} · output kept", track.stages[later].label()),
            (Some(index), None) => format!("Stopped at stage {}/{} · output kept", index + 1, track.stages.len()),
            (None, None) => String::new(),
        },
        failure: Some(failure),
        track,
        primary,
    }
}

fn ready(runtime: &Runtime, occupancy: Occupancy, now: SystemTime, base: Status) -> Status {
    let numbers = if occupancy.panels == 0 {
        "No panels yet".to_owned()
    } else if occupancy.terminals == 0 {
        panels(occupancy.panels)
    } else {
        format!("{}/{} terminals running", occupancy.running, occupancy.terminals)
    };
    let uptime = runtime
        .current_run_cost(now)
        .map(|run| format!("up {}", progress::duration(run.elapsed)));
    let timeline = runtime
        .state
        .as_ref()
        .and_then(|state| state.timeline.as_ref())
        .filter(|timeline| !timeline.total().is_zero());
    // Records from before timelines keep their single total.
    let took = timeline.map_or_else(
        || {
            runtime
                .state
                .as_ref()
                .and_then(|state| state.ready_after_seconds)
                .map(|seconds| ("Ready", Duration::from_secs(seconds)))
        },
        |timeline| {
            Some((
                if timeline.reconnected { "Reconnected" } else { "Ready" },
                timeline.total(),
            ))
        },
    );
    let right = [
        took.map(|(verb, took)| format!("{verb} in {}", progress::duration(took))),
        Some(panels(occupancy.panels)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Status {
        tone: Tone::Ready,
        verb: "Ready".into(),
        numbers,
        // A failed follow-up operation on a connected cloud leaves it Ready; say what failed.
        tail: runtime.error.clone().or(uptime).unwrap_or_default(),
        right,
        track: Track::complete(stages(runtime), false),
        primary: (!rebuild::blocks_stop(runtime)).then_some(Primary::Stop),
        ..base
    }
}

#[cfg(test)]
mod tests;
