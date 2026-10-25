//! Cloud delete requests of agents. Horizon deletes one cloud's provider resources in the
//! caller's workspace, on a thread this host owns, and only when an agent panel of that
//! same actor created the cloud. A delete cannot be recalled once its provider request is
//! sent, so every answer says what the provider confirmed and never that it was undone.
use super::{HorizonApp, Runtime, Stage, cloud_runtime, deployment};
use crate::app::browser_requests::actor_panel;
use egui::Context;
use horizon_core::browser::manifest::{
    self,
    provider_usage::{CloudDeleteOperation, CloudDeleteRequest, UsageRequest},
};
use horizon_core::cloud_runtime::{
    Cancellation, Event,
    deployment::Absence,
    settings::Settings,
    state::{Creator, Deployment, Store},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{Receiver, Sender, channel},
    time::Duration,
};

/// A delete takes minutes, so a frame during one repaints sooner than an idle board.
const POLL_REPAINT: Duration = Duration::from_millis(500);

const UNAVAILABLE: &str = "cloud_delete_unavailable: requires a Horizon agent panel";
const INVALID: &str = "cloud_delete_invalid_request: operation_id is one lowercase UUID; submit needs one cloud ID";
const EXPIRED: &str = "cloud_delete_expired: the request expired before Horizon answered it; nothing changed";
const UNKNOWN_CLOUD: &str = "cloud_delete_unknown_cloud: no cloud with this ID in your workspace; run cloud_list first";
const AMBIGUOUS: &str = "cloud_delete_unknown_cloud: more than one saved cloud has this ID; use the sidebar";
const UNKNOWN_OPERATION: &str = "cloud_delete_unknown_operation: no operation of yours has this ID";
const BUSY: &str = "cloud_delete_busy: another operation owns this cloud";
const AGENT_BUSY: &str = "cloud_delete_busy: an agent's create operation owns this cloud; read its status first";
const DELETE_BUSY: &str = "cloud_delete_busy: this cloud already has a delete operation; poll its status instead";
const NO_RECORD: &str = "cloud_delete_refused: Horizon has no deployment record for this cloud, so it has nothing to delete; a person removes its card";
const NOT_AN_AGENTS_CLOUD: &str = "cloud_delete_refused: not an agent's cloud; a person deletes this cloud on its card";
const NO_WORKER: &str = "cloud_delete_refused: no worker was requested; a person removes this cloud's card";
const CANCELLED: &str = "cloud_delete_cancelled: the delete may already have started";
const DONE: &str = "cloud_delete_done: the delete finished; cloud_list shows the cloud without a worker, and a person removes its card";
const IN_PROGRESS: &str = "a delete may already be in progress; a provider request that was sent cannot be recalled";
const ABSENT: &str = "the cloud's record and every provider journal confirm that nothing it created remains";
const UNCONFIRMED: &str = "the provider's records do not confirm absence; read the cloud's record, or check the provider with your own credential";
const CANCELLED_PAST_RECALL: &str = "the cancel arrived, but the delete had already gone past recall: the cloud's record and every provider journal confirm that nothing it created remains";
const CANCELLED_MAY_REMAIN: &str = "the cancel arrived, and a provider journal still holds something the cloud created";
const CANCELLED_STOPPED: &str = "the cancel arrived, and the delete stopped before the provider confirmed absence; the record keeps the step it reached";

#[derive(Default)]
pub(super) struct State {
    queued: Vec<UsageRequest>,
    /// The operations agents named, by the ID each chose. A lost answer is polled here.
    operations: BTreeMap<String, Operation>,
    channel: Option<(Sender<Message>, Receiver<Message>)>,
}

struct Operation {
    id: String,
    actor: String,
    cloud: String,
    /// The provider the cloud's card records, as the caller sees it.
    provider: String,
    /// The card the cloud shows on, so its own view follows the delete.
    card: u32,
    phase: Phase,
    /// The deletion step this operation last reported.
    stage: Option<Stage>,
    /// What the provider's own records prove about the cloud's resources.
    absent: Option<Absence>,
    /// Why the operation failed, or that a cancel asked for its stop.
    error: Option<String>,
    cancel: Option<Cancellation>,
    /// Whether the deletion thread reported its outcome. A cancel answers at once, but a
    /// provider request already sent runs on, so the facts about the cloud arrive later.
    finished: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Horizon accepted the operation and checks this cloud's record.
    Submitted,
    Deleting,
    Done,
    Failed,
    /// A cancel asked the deletion to stop, which cannot undo a request already sent.
    Cancelled,
}

impl Phase {
    const fn label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Deleting => "deleting",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    const fn done(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// What an operation's thread reports to the host that owns it.
enum Message {
    /// The record admits this caller and a worker was requested, so the provider steps run.
    Started { operation: String },
    /// The deletion moved to another step.
    Stage { operation: String, stage: Stage },
    /// The deletion ended. `state` is the record it left and `absent` what the provider
    /// confirmed; `error` says why it stopped instead.
    Finished {
        operation: String,
        state: Option<Box<Deployment>>,
        absent: Option<Absence>,
        error: Option<String>,
    },
}

impl Message {
    /// The operation this message belongs to.
    fn operation(&self) -> &str {
        match self {
            Self::Started { operation } | Self::Stage { operation, .. } | Self::Finished { operation, .. } => operation,
        }
    }
}

impl State {
    fn sender(&mut self) -> Sender<Message> {
        self.channel.get_or_insert_with(channel).0.clone()
    }

    fn messages(&self) -> Vec<Message> {
        self.channel
            .as_ref()
            .map_or_else(Vec::new, |(_, receiver)| receiver.try_iter().collect())
    }

    /// Whether an operation still waits for what its thread reports, which a cancelled
    /// operation does too, so its answers change until the deletion ends.
    fn running(&self) -> bool {
        self.operations.values().any(|operation| !operation.finished)
    }

    /// Whether this operation ended, or a cancel already asked it to stop. Nothing it
    /// still reports may change an answer after that.
    fn ended(&self, operation: &str) -> bool {
        self.operations.get(operation).is_none_or(|running| {
            running.phase.done() || running.cancel.as_ref().is_some_and(Cancellation::is_cancelled)
        })
    }
}

impl HorizonApp {
    pub(in crate::app) fn queue_cloud_delete_request(&mut self, request: UsageRequest) {
        self.cloud_prototype.production.delete_agent.queued.push(request);
    }

    /// Answers the queued delete requests and applies what the operations' threads report.
    pub(in crate::app::cloud_panel) fn poll_cloud_delete_requests(&mut self, ctx: &egui::Context) {
        for request in std::mem::take(&mut self.cloud_prototype.production.delete_agent.queued) {
            let answer = self.answer_cloud_delete(&request, ctx);
            complete(&request, answer);
        }
        for message in self.cloud_prototype.production.delete_agent.messages() {
            self.apply_delete_message(message);
        }
        if self.cloud_prototype.production.delete_agent.running() {
            ctx.request_repaint_after(POLL_REPAINT);
        }
    }

    fn answer_cloud_delete(&mut self, request: &UsageRequest, ctx: &Context) -> Result<Value, String> {
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&self.board, &request.actor))
            .flatten()
            .and_then(|panel| self.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or(UNAVAILABLE)?;
        let delete = request
            .cloud_delete
            .as_ref()
            .filter(|delete| delete.valid())
            .ok_or(INVALID)?;
        match delete.operation {
            CloudDeleteOperation::Status => self.delete_status(&request.actor, delete),
            CloudDeleteOperation::Cancel => self.delete_cancel(&request.actor, delete),
            CloudDeleteOperation::Submit => {
                // A caller that stopped waiting gets no delete it did not see.
                if manifest::now_millis() >= request.deadline_at_millis {
                    return Err(EXPIRED.into());
                }
                self.submit_delete(&request.actor, &workspace, delete, ctx)
            }
        }
    }

    fn submit_delete(
        &mut self,
        actor: &str,
        workspace: &str,
        delete: &CloudDeleteRequest,
        ctx: &Context,
    ) -> Result<Value, String> {
        let id = delete.operation_id().to_owned();
        if let Some(known) = self.cloud_prototype.production.delete_agent.operations.get(&id) {
            if known.actor != actor {
                return Err(UNKNOWN_OPERATION.into());
            }
            // The answer was lost and the caller sent the same operation again: no second
            // delete of the same cloud.
            return Ok(status(known));
        }
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("cloud_delete_unavailable: Horizon has no cloud state directory")?;
        let wanted = delete.cloud.as_deref().unwrap_or_default();
        let index = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .enumerate()
            .filter(|(_, group)| group.workspace == workspace)
            .find(|(_, group)| group.remote.as_ref().is_some_and(|remote| remote.id == wanted))
            .map(|(index, _)| index)
            .ok_or(UNKNOWN_CLOUD)?;
        // Two saved clouds with one ID, in any workspace, make the ID ambiguous.
        let same_id = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.remote.as_ref().is_some_and(|remote| remote.id == wanted))
            .count();
        if same_id != 1 {
            return Err(AMBIGUOUS.into());
        }
        let group = &self.cloud_prototype.groups.0[index];
        let card = group.issue;
        let provider = group
            .remote
            .as_ref()
            .map(|remote| remote.profile.provider.clone())
            .unwrap_or_default();
        // The cloud's own operation, its provider check and an agent's create own it first.
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&card)
            .is_some_and(Runtime::busy)
        {
            return Err(BUSY.into());
        }
        if self.cloud_prototype.production.create_agent.owns_cloud(wanted) {
            return Err(AGENT_BUSY.into());
        }
        if self
            .cloud_prototype
            .production
            .delete_agent
            .operations
            .values()
            .any(|running| running.cloud == wanted && !running.phase.done())
        {
            return Err(DELETE_BUSY.into());
        }
        let cancel = Cancellation::default();
        let state = &mut self.cloud_prototype.production.delete_agent;
        state.operations.insert(
            id.clone(),
            Operation {
                id: id.clone(),
                actor: actor.into(),
                cloud: wanted.into(),
                provider: provider.clone(),
                card,
                phase: Phase::Submitted,
                stage: None,
                absent: None,
                error: None,
                cancel: Some(cancel.clone()),
                finished: false,
            },
        );
        let sender = state.sender();
        let ctx = ctx.clone();
        let (thread_id, thread_cloud, thread_actor) = (id.clone(), wanted.to_owned(), actor.to_owned());
        // The cloud's record, its credentials and every provider request run off this
        // thread, so the frame that answers the caller never waits for them.
        std::thread::spawn(move || {
            delete_cloud(
                &root,
                (&thread_cloud, &thread_actor),
                (&thread_id, &cancel),
                (&sender, &ctx),
            );
        });
        Ok(json!({
            "operation_id": id,
            "phase": Phase::Submitted.label(),
            "cloud": wanted,
            "provider": provider,
            "in_progress": IN_PROGRESS,
        }))
    }

    fn apply_delete_message(&mut self, message: Message) {
        let operation = message.operation().to_owned();
        let ended = self.cloud_prototype.production.delete_agent.ended(&operation);
        let Some(running) = self
            .cloud_prototype
            .production
            .delete_agent
            .operations
            .get_mut(&operation)
        else {
            return;
        };
        let finished = match message {
            // A cancel, or an operation that already ended, keeps the answer it gave.
            Message::Started { .. } => {
                if !ended {
                    running.phase = Phase::Deleting;
                }
                None
            }
            Message::Stage { stage, .. } => {
                if !ended {
                    running.stage = Some(stage);
                }
                None
            }
            Message::Finished {
                state, absent, error, ..
            } => {
                // The step and what the provider's own records prove describe the cloud, not
                // the request, so they are recorded also when a cancel ended the request
                // first: a request already sent runs on to its end.
                running.finished = true;
                running.stage = state.as_ref().map(|state| state.stage).or(running.stage);
                running.absent = absent;
                running.cancel = None;
                if !ended {
                    running.phase = if error.is_some() { Phase::Failed } else { Phase::Done };
                    running.error = error;
                }
                // The record this deletion wrote is the truth on disk, also for an operation
                // that a cancel ended first, so the card takes it rather than its own copy.
                Some((running.card, state, running.error.clone()))
            }
        };
        let Some((card, state, error)) = finished else {
            return;
        };
        // A card that another operation holds keeps its own view until that operation ends.
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&card) else {
            return;
        };
        if runtime.busy() {
            return;
        }
        match state {
            Some(state) if saved_is_newer(&state, runtime.state.as_ref()) => runtime.adopt_snapshot(*state),
            Some(_) => {}
            None => runtime.error = error,
        }
    }

    fn delete_status(&self, actor: &str, delete: &CloudDeleteRequest) -> Result<Value, String> {
        let running = self
            .cloud_prototype
            .production
            .delete_agent
            .operations
            .get(delete.operation_id())
            .filter(|running| running.actor == actor)
            .ok_or(UNKNOWN_OPERATION)?;
        Ok(status(running))
    }

    fn delete_cancel(&mut self, actor: &str, delete: &CloudDeleteRequest) -> Result<Value, String> {
        let running = self
            .cloud_prototype
            .production
            .delete_agent
            .operations
            .get_mut(delete.operation_id())
            .filter(|running| running.actor == actor)
            .ok_or(UNKNOWN_OPERATION)?;
        if running.phase == Phase::Done {
            return Err(DONE.into());
        }
        // A cancel asks the deletion to stop where it still can; a provider request that
        // left this computer is never recalled, so the answer says what may have started.
        if !running.phase.done() {
            if let Some(cancel) = running.cancel.take() {
                cancel.cancel();
            }
            running.phase = Phase::Cancelled;
            running.error = Some(CANCELLED.into());
        }
        Ok(status(running))
    }
}

/// The answer one operation gives: where it stands, what it deletes, and what the
/// provider's own records prove.
fn status(running: &Operation) -> Value {
    json!({
        "operation_id": running.id,
        "phase": running.phase.label(),
        "done": running.phase.done() && running.finished,
        "cloud": running.cloud,
        "provider": running.provider,
        "stage": running.stage.map(Stage::label),
        "absent": running.absent == Some(Absence::Proven),
        "absence": absence_text(running),
        "error": running.error,
    })
}

/// What the provider confirmed, in words that never claim more than the record proves.
fn absence_text(running: &Operation) -> &'static str {
    // A cancel asks the deletion to stop, and a provider request already sent runs on, so
    // the answer names both facts instead of either one alone.
    if running.phase == Phase::Cancelled {
        return match (running.finished, running.absent) {
            (false, _) => IN_PROGRESS,
            (true, Some(Absence::Proven)) => CANCELLED_PAST_RECALL,
            (true, Some(Absence::MayRemain)) => CANCELLED_MAY_REMAIN,
            (true, Some(Absence::Unconfirmed) | None) => CANCELLED_STOPPED,
        };
    }
    match (running.absent, running.error.as_deref()) {
        (Some(Absence::Proven), _) => ABSENT,
        (Some(Absence::MayRemain), _) => "a provider journal still holds something the cloud created",
        (Some(Absence::Unconfirmed), _) => UNCONFIRMED,
        (None, Some(_)) => {
            "the delete stopped before the provider confirmed absence; the record keeps the step it reached"
        }
        (None, None) if running.phase.done() => UNCONFIRMED,
        (None, None) => IN_PROGRESS,
    }
}

/// One audit entry per answered call: who asked, for which cloud, the step it reached, the
/// provider, and what the provider's own records proved. An entry never holds a credential,
/// a provider token or a worker ID.
fn audit(request: &UsageRequest, answer: &Result<Value, String>) {
    let Some(delete) = &request.cloud_delete else {
        return;
    };
    let cloud = |key: &str| answer.as_ref().ok().map(|value| value[key].clone());
    tracing::info!(
        actor = %request.actor,
        operation = ?delete.operation,
        operation_id = %delete.operation_id(),
        cloud = %delete.cloud.as_deref().unwrap_or(""),
        answered_cloud = ?cloud("cloud"),
        phase = ?cloud("phase"),
        stage = ?cloud("stage"),
        provider = ?cloud("provider"),
        absent = ?cloud("absent"),
        error = ?answer.as_ref().err(),
        "cloud delete request answered"
    );
}

fn complete(request: &UsageRequest, answer: Result<Value, String>) {
    audit(request, &answer);
    let mut result = request.result(Vec::new(), None);
    match answer {
        Ok(value) => result.cloud_delete = Some(value),
        Err(error) => result.error = Some(error),
    }
    if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
        tracing::warn!(kind = ?error.kind(), "could not answer a cloud delete request");
    }
}

/// Whether the record this deletion saved is newer than what the card shows: it names the
/// same cloud and revision, and the card has not reached the deleted step.
fn saved_is_newer(saved: &Deployment, shown: Option<&Deployment>) -> bool {
    shown.is_none_or(|shown| {
        shown.cloud_id == saved.cloud_id && shown.revision == saved.revision && shown.stage != Stage::Deleted
    })
}

/// How long the operation waits before it reads the provider's records once more.
const ABSENCE_RETRY: Duration = Duration::from_millis(50);

/// What the provider's records prove about the cloud's resources. Another operation can hold
/// the record for the moment of the first read, which is no verdict, so the read is tried
/// once more before the answer says that absence is unproven.
fn absence(state_root: &Path) -> Option<Absence> {
    read_absence(|| deployment::absence(state_root))
}

fn read_absence(mut read: impl FnMut() -> cloud_runtime::Result<Absence>) -> Option<Absence> {
    if let Ok(absence) = read() {
        return Some(absence);
    }
    std::thread::sleep(ABSENCE_RETRY);
    read().ok()
}

/// Why this caller may not delete `cloud`, read from the cloud's own record: only the
/// agent panel that created a cloud deletes it, and only a cloud whose worker was
/// requested has provider resources to delete.
fn refusal(cloud: &Deployment, actor: &str) -> Option<String> {
    match &cloud.created_by {
        Some(Creator { actor: creator, .. }) if creator == actor => {}
        Some(_) | None => return Some(NOT_AN_AGENTS_CLOUD.into()),
    }
    cloud.spec.is_none().then(|| NO_WORKER.into())
}

/// Deletes one cloud for its agent and reports every step. The cloud's record is read
/// under its own lock, which is dropped before the deletion takes that lock again as the
/// card's own delete does.
fn delete_cloud(
    root: &Path,
    (cloud, actor): (&str, &str),
    (operation, cancel): (&str, &Cancellation),
    (sender, ctx): (&Sender<Message>, &Context),
) {
    let send = |message: Message| {
        let _ = sender.send(message);
        ctx.request_repaint();
    };
    let state_root = match cloud_runtime::state::cloud_directory(root, cloud) {
        Ok(state_root) => state_root,
        Err(error) => {
            send(Message::Finished {
                operation: operation.to_owned(),
                state: None,
                absent: None,
                error: Some(failed(error)),
            });
            return;
        }
    };
    let admitted = match Store::lock_existing(&state_root) {
        Ok(store) => match store.load() {
            Ok(Some(state)) => refusal(&state, actor).map_or(Ok(()), Err),
            Ok(None) => Err(NO_RECORD.to_owned()),
            Err(error) => Err(failed(error)),
        },
        // A cloud whose directory holds no record has no provider resource to delete.
        Err(cloud_runtime::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(NO_RECORD.to_owned())
        }
        Err(error) => Err(failed(error)),
    };
    if let Err(error) = admitted {
        send(Message::Finished {
            operation: operation.to_owned(),
            state: None,
            absent: None,
            error: Some(error),
        });
        return;
    }
    let settings = match Settings::load(&root.join("settings.json")) {
        Ok(settings) => settings,
        Err(error) => {
            send(Message::Finished {
                operation: operation.to_owned(),
                state: None,
                absent: None,
                error: Some(format!(
                    "cloud_delete_failed: the cloud settings could not be read: {error}"
                )),
            });
            return;
        }
    };
    let emit = |event: Event| {
        if let Event::Stage(stage, _) = event {
            let _ = sender.send(Message::Stage {
                operation: operation.to_owned(),
                stage,
            });
            ctx.request_repaint();
        }
    };
    send(Message::Started {
        operation: operation.to_owned(),
    });
    let outcome = deployment::terminate(&state_root, &settings, cancel, &emit);
    // The record the deletion left is the only durable evidence of what the provider
    // confirmed, and the card shows that record too.
    let state = Store::lock_existing(&state_root)
        .and_then(|store| store.load())
        .ok()
        .flatten();
    let absent = absence(&state_root);
    send(Message::Finished {
        operation: operation.to_owned(),
        state: state.map(Box::new),
        absent,
        error: outcome.err().map(failed),
    });
}

/// Why a delete stopped, in the vocabulary of this tool.
fn failed(error: cloud_runtime::Error) -> String {
    match error {
        cloud_runtime::Error::Busy => BUSY.to_owned(),
        error => format!("cloud_delete_failed: {error}"),
    }
}

#[cfg(test)]
mod tests;
