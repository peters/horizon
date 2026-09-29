//! Agent requests for companion clouds. Only an explicit Ensure Ready or Stop reaches
//! the lifecycle service; loading, checking a box and restoring Horizon never start
//! a companion. A request's operation runs on the target cloud's card, as its own
//! buttons would, and the agent polls its status by operation ID.
use super::{CloudGroups, HorizonApp, Owner};
use crate::app::browser_requests::actor_panel;
use horizon_core::{
    browser::manifest::{
        self,
        provider_usage::{CompanionAction, CompanionRequest, UsageRequest},
    },
    cloud_runtime::{
        self, Cancellation, Event, Stage,
        companions::{
            self, Context, intent, inventory,
            lifecycle::{self, Operation, Phase},
        },
        settings::Settings,
        state::{OperationId, Store},
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::mpsc::{Receiver, Sender, channel},
    time::{Duration, Instant},
};

mod creation;
mod execution;
#[cfg(test)]
use execution::finish;

/// How long a job waits for a companion journal another job holds.
const BUSY_WAIT: Duration = Duration::from_secs(8);

#[derive(Default)]
pub(super) struct State {
    queued: Vec<UsageRequest>,
    channel: Option<(Sender<Message>, Receiver<Message>)>,
    /// Source clouds whose periodic refresh waits for agent requests to finish, with
    /// how many are running. An operation's execution keeps its request's hold.
    held: BTreeMap<String, usize>,
    /// Target clouds with an operation running on this Horizon.
    executing: BTreeSet<String>,
    /// Missing companions agents asked for, awaiting the owner on the source card.
    pub(super) creation: creation::State,
}

enum Message {
    Answered {
        request: Box<UsageRequest>,
        source: String,
        outcome: Result<Box<Answer>, String>,
    },
    Executed {
        source: String,
        target: String,
    },
}

enum Answer {
    Recorded(Submitted),
    /// An Ensure Ready for a declared companion that has no cloud to bind.
    Missing(Owner, companions::Declaration),
}

struct Submitted {
    operation: Operation,
    context: Context,
    alias: String,
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

    fn hold(&mut self, source: &str) {
        *self.held.entry(source.to_owned()).or_default() += 1;
    }

    fn release(&mut self, source: &str) {
        if let Some(count) = self.held.get_mut(source) {
            *count -= 1;
            if *count == 0 {
                self.held.remove(source);
            }
        }
    }

    /// Drops creations waiting on a card of the previous session, releasing their holds.
    pub(super) fn discard_creations(&mut self) {
        for source in self.creation.discard() {
            self.release(&source);
        }
    }

    pub(super) fn holds(&self, source: &str) -> bool {
        self.held.contains_key(source)
    }
}

impl HorizonApp {
    pub(in crate::app) fn queue_cloud_companion_request(&mut self, request: UsageRequest) {
        self.cloud_prototype.production.companions.agent.queued.push(request);
    }

    /// Starts queued agent requests and finishes the ones whose jobs answered.
    pub(in crate::app::cloud_panel) fn poll_cloud_companion_requests(&mut self, ctx: &egui::Context) {
        self.poll_companion_creations(ctx);
        for request in std::mem::take(&mut self.cloud_prototype.production.companions.agent.queued) {
            if let Err(error) = self.start_companion_request(&request, ctx) {
                complete(&request, Err(error));
            }
        }
        for message in self.cloud_prototype.production.companions.agent.messages() {
            match message {
                Message::Answered {
                    request,
                    source,
                    outcome,
                } => {
                    let (answer, started) = match outcome {
                        Ok(answer) => match *answer {
                            Answer::Recorded(submitted) => {
                                let (answer, started) = self.answer_submitted(&request, &source, submitted, ctx);
                                (Ok(answer), started)
                            }
                            Answer::Missing(owner, declaration) => {
                                (self.answer_missing(&request, &source, owner, declaration), false)
                            }
                        },
                        Err(error) => (Err(error), false),
                    };
                    // A started execution keeps this request's hold until it reports back.
                    if !started {
                        self.release_companion_source(&source);
                    }
                    complete(&request, answer);
                }
                Message::Executed { source, target } => {
                    self.cloud_prototype
                        .production
                        .companions
                        .agent
                        .executing
                        .remove(&target);
                    self.release_companion_source(&source);
                }
            }
        }
        if !self.cloud_prototype.production.companions.agent.held.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    /// Asks the owner to create a missing companion. A lookup that finished after the
    /// session changed carries the previous owner and is refused, never shown.
    fn answer_missing(
        &mut self,
        request: &UsageRequest,
        source: &str,
        owner: Owner,
        declaration: companions::Declaration,
    ) -> Result<Value, String> {
        let current = self
            .cloud_prototype
            .production
            .companions
            .entries
            .get(source)
            .is_some_and(|entry| entry.owner == owner);
        if !current {
            return Err("cloud_companion_unavailable: the workspace session changed; send the request again".into());
        }
        let alias = request
            .cloud_companion
            .as_ref()
            .and_then(|companion| companion.alias.clone())
            .unwrap_or_default();
        let id = operation_id(request)?;
        Ok(self.request_companion_creation(owner, &alias, declaration, id))
    }

    /// The answer to a recorded request, continuing its operation unless it is a
    /// status poll, which only reads. Returns whether an execution started.
    fn answer_submitted(
        &mut self,
        request: &UsageRequest,
        source: &str,
        submitted: Submitted,
        ctx: &egui::Context,
    ) -> (Value, bool) {
        if reads_only(request) {
            let operation = &submitted.operation;
            // A never-started card learns that it needs the owner's Start only when the
            // run sent to it ends, so a poll puts that offer on the card. It starts nothing.
            if operation.phase == Phase::ConfirmationRequired
                && let Some(asked) =
                    self.request_companion_start(source, &submitted.alias, operation, &submitted.context)
            {
                return (asked, false);
            }
            let mut answer = describe(source, &submitted.alias, operation);
            if answer["done"] == false
                && !self
                    .cloud_prototype
                    .production
                    .companions
                    .agent
                    .executing
                    .contains(&operation.intent.target_cloud_id)
            {
                resend(&mut answer, RESEND);
            }
            return (answer, false);
        }
        self.continue_operation(source, submitted, ctx)
    }

    fn release_companion_source(&mut self, source: &str) {
        let companions = &mut self.cloud_prototype.production.companions;
        companions.agent.release(source);
        if let Some(entry) = companions.entries.get_mut(source) {
            // Show what the request changed without waiting for the next refresh.
            entry.due = Instant::now();
        }
    }

    fn start_companion_request(&mut self, request: &UsageRequest, ctx: &egui::Context) -> Result<(), String> {
        let companion = request
            .cloud_companion
            .as_ref()
            .filter(|companion| companion.valid())
            .ok_or("cloud_companion_invalid_request")?;
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&self.board, &request.actor))
            .flatten()
            .and_then(|panel| self.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or("cloud_companion_unavailable: requires a Horizon agent panel")?;
        if companion.action == CompanionAction::List {
            complete(request, Ok(self.companion_listing(&workspace)));
            return Ok(());
        }
        let source = companion.cloud.clone().unwrap_or_default();
        let in_workspace = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .any(|group| group.workspace == workspace && group.remote.as_ref().is_some_and(|l| l.id == source));
        let companions = &mut self.cloud_prototype.production.companions;
        let entry = companions
            .entries
            .get_mut(&source)
            .filter(|entry| in_workspace && !entry.blocked)
            .ok_or("cloud_companion_unknown_cloud: no cloud with this ID in your workspace's saved session")?;
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("cloud_companion_unavailable: Horizon has no cloud state directory")?;
        let id = operation_id(request)?;
        expired(request)?;
        let action = match companion.action {
            CompanionAction::EnsureReady => Some(intent::Action::EnsureReady),
            CompanionAction::Stop => Some(intent::Action::Stop),
            CompanionAction::Status | CompanionAction::List => None,
        };
        let alias = companion.alias.clone().unwrap_or_default();
        if let Some(answer) = companions.agent.creation.answer(&source, &alias, action, id) {
            complete(request, answer);
            return Ok(());
        }
        // A refresh holds the source journal while it reaches workers, and may carry
        // the owner's checkbox change, so it finishes; the job waits for its lock.
        let owner = entry.owner.clone();
        companions.agent.hold(&source);
        let sender = companions.agent.sender();
        let groups = self.cloud_prototype.groups.clone();
        let request = Box::new(request.clone());
        let companion = companion.clone();
        let deadline = request.deadline_at_millis;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let outcome = submit(&root, &owner, &groups, (&companion, deadline), id).map(Box::new);
            let _ = sender.send(Message::Answered {
                request,
                source,
                outcome,
            });
            ctx.request_repaint();
        });
        Ok(())
    }

    fn companion_listing(&self, workspace: &str) -> Value {
        let companions = &self.cloud_prototype.production.companions;
        let clouds = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.workspace == workspace)
            .filter_map(|group| {
                let launch = group.remote.as_ref()?;
                let entry = companions.entries.get(&launch.id);
                let rows = entry
                    .and_then(|entry| entry.snapshot.as_ref())
                    .map_or_else(Vec::new, |snapshot| {
                        snapshot
                            .rows
                            .iter()
                            .map(|row| {
                                json!({
                                    "alias": row.companion.alias,
                                    "repository": row.companion.repository,
                                    "profile": row.companion.profile,
                                    "selected": row.companion.selected,
                                    "status": row.companion.status,
                                    "target_cloud_id": row.companion.target_cloud_id,
                                    "error": row.error,
                                })
                            })
                            .collect()
                    });
                Some(json!({
                    "cloud": launch.id,
                    "title": group.title,
                    "companions": rows,
                    "error": entry.and_then(|entry| entry.error.as_deref()),
                }))
            })
            .collect::<Vec<_>>();
        json!({ "clouds": clouds })
    }

    /// Runs a submitted operation on the target cloud's card when nothing else is
    /// running there. Returns the agent's answer and whether this started it.
    fn continue_operation(&mut self, source: &str, submitted: Submitted, ctx: &egui::Context) -> (Value, bool) {
        let Submitted {
            operation,
            context,
            alias,
        } = submitted;
        let target = operation.intent.target_cloud_id.clone();
        let mut answer = describe(source, &alias, &operation);
        if operation.phase == Phase::ConfirmationRequired
            && let Some(asked) = self.request_companion_start(source, &alias, &operation, &context)
        {
            return (asked, false);
        }
        if !operation.intent.state.pending() || operation.phase == Phase::ConfirmationRequired {
            return (answer, false);
        }
        if self
            .cloud_prototype
            .production
            .companions
            .agent
            .executing
            .contains(&target)
        {
            answer["executing"] = json!(true);
            return (answer, false);
        }
        if let Some(asked) = self.request_companion_recovery(source, &alias, &operation, &context) {
            return (asked, false);
        }
        let stop = operation.intent.action == intent::Action::Stop;
        match self.execute_on_card(
            source,
            &target,
            (&alias, stop),
            operation.intent.operation_id,
            context,
            ctx,
        ) {
            Ok(()) => {
                answer["executing"] = json!(true);
                (answer, true)
            }
            Err(reason) => {
                resend(&mut answer, &reason);
                (answer, false)
            }
        }
    }
}

/// Records the request, binding a checked companion on its first request. Reads
/// never bind or submit anything.
fn submit(
    root: &Path,
    owner: &Owner,
    groups: &CloudGroups,
    (companion, deadline): (&CompanionRequest, i64),
    id: OperationId,
) -> Result<Answer, String> {
    let context = inventory::prepare(owner, groups, &Cancellation::default()).map_err(|error| error.to_string())?;
    let alias = companion.alias.clone().unwrap_or_default();
    let request = lifecycle::Request {
        root,
        owner,
        context: &context,
        alias: &alias,
    };
    if companion.action != CompanionAction::Status && manifest::now_millis() >= deadline {
        return Err(EXPIRED.into());
    }
    if companion.action == CompanionAction::EnsureReady
        && let Err(error) = retry_busy(|| lifecycle::bind_selected(&request))
    {
        // No cloud matches the declaration: the owner may create one on the card.
        return match missing(&context, &alias) {
            Some(declaration) => Ok(Answer::Missing(owner.clone(), declaration)),
            None => Err(error.to_string()),
        };
    }
    let operation = match companion.action {
        CompanionAction::Status => retry_busy(|| lifecycle::status(&request, id)),
        CompanionAction::EnsureReady | CompanionAction::Stop => retry_busy(|| {
            // Reading the workspace may take a while; nothing is recorded after the
            // caller has stopped waiting.
            if manifest::now_millis() >= deadline {
                return Err(cloud_runtime::Error::Invalid(EXPIRED));
            }
            lifecycle::bind_selected(&request)?;
            let action = if companion.action == CompanionAction::Stop {
                intent::Action::Stop
            } else {
                intent::Action::EnsureReady
            };
            lifecycle::submit(&request, action, id)
        }),
        CompanionAction::List => return Err("cloud_companion_invalid_request".into()),
    }
    .map_err(|error| match error {
        cloud_runtime::Error::Busy => {
            "cloud_companion_busy: another operation holds this companion; poll its status".to_owned()
        }
        error => error.to_string(),
    })?;
    Ok(Answer::Recorded(Submitted {
        operation,
        context,
        alias,
    }))
}

/// The declaration of a separate-cloud companion that no cloud in the workspace matches.
/// A same-worker sibling never has a cloud of its own, so it is never missing.
fn missing(context: &Context, alias: &str) -> Option<companions::Declaration> {
    let declaration = context
        .declarations
        .get(alias)
        .filter(|declaration| declaration.placement.is_cloud())?;
    (!context
        .inventory
        .iter()
        .any(|target| target.cloud_id != context.source.cloud_id && target.declaration.matches(declaration)))
    .then(|| declaration.clone())
}

fn operation_id(request: &UsageRequest) -> Result<OperationId, String> {
    serde_json::from_value::<OperationId>(json!(
        request
            .cloud_companion
            .as_ref()
            .and_then(|companion| companion.operation_id.clone())
    ))
    .map_err(|_| "cloud_companion_invalid_request: operation_id must be a UUID".to_owned())
}

const EXPIRED: &str =
    "cloud_companion_expired: the request expired before Horizon recorded it; nothing was started or stopped";

/// An Ensure Ready or Stop whose caller has stopped waiting is refused before it is
/// recorded; a status poll is always answered.
fn expired(request: &UsageRequest) -> Result<(), String> {
    let lifecycle = request
        .cloud_companion
        .as_ref()
        .is_some_and(|companion| matches!(companion.action, CompanionAction::EnsureReady | CompanionAction::Stop));
    if lifecycle && manifest::now_millis() >= request.deadline_at_millis {
        return Err(EXPIRED.into());
    }
    Ok(())
}

fn reads_only(request: &UsageRequest) -> bool {
    request
        .cloud_companion
        .as_ref()
        .is_none_or(|companion| companion.action == CompanionAction::Status)
}

fn retry_busy<T>(mut attempt: impl FnMut() -> cloud_runtime::Result<T>) -> cloud_runtime::Result<T> {
    let started = Instant::now();
    loop {
        match attempt() {
            Err(cloud_runtime::Error::Busy) if started.elapsed() < BUSY_WAIT => {
                std::thread::sleep(Duration::from_millis(250));
            }
            result => return result,
        }
    }
}

fn describe(source: &str, alias: &str, operation: &Operation) -> Value {
    json!({
        "operation_id": operation.intent.operation_id,
        "action": operation.intent.action,
        "cloud": source,
        "alias": alias,
        "target_cloud_id": operation.intent.target_cloud_id,
        "phase": operation.phase,
        "done": !operation.intent.state.pending() || operation.phase == Phase::ConfirmationRequired,
        "resend": false,
        "message": hint(operation.phase),
    })
}

const RESEND: &str = "Nothing is running this operation; send the same Ensure Ready or Stop again to continue it. It reconciles and never repeats a provider change";

/// Marks a pending operation that polling alone cannot move forward: only the same
/// Ensure Ready or Stop sent again continues it.
fn resend(answer: &mut Value, reason: &str) {
    answer["resend"] = json!(true);
    answer["message"] = json!(reason);
}

fn hint(phase: Phase) -> &'static str {
    match phase {
        Phase::Submitted | Phase::Running | Phase::Inspecting | Phase::Settling | Phase::VerifyingAccess => {
            "In progress; poll its status with this operation_id"
        }
        Phase::ConfirmationRequired => "This companion's cloud was never started; the owner starts it from its card",
        Phase::ReconcileRequired => {
            "The provider's outcome is uncertain; the next Ensure Ready or Stop reconciles it and never repeats it"
        }
        Phase::RetryRequired => "Nothing is pending; send a new request to try again",
        Phase::Ready => "Ready: SSH access to the companion and its repository environment is verified",
        Phase::Stopped => "Stopped; it stays stopped until an explicit Ensure Ready",
        Phase::Refused => "Refused: the companion is deleted, being deleted, lost or changed; nothing was started",
    }
}

fn complete(request: &UsageRequest, answer: Result<Value, String>) {
    let mut result = request.result(Vec::new(), None);
    match answer {
        Ok(value) => result.companion = Some(value),
        Err(error) => result.error = Some(error),
    }
    if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
        tracing::warn!(kind = ?error.kind(), "could not answer a cloud companion request");
    }
}

#[cfg(test)]
mod tests;
