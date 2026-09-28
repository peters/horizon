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
            Context, intent, inventory,
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
}

enum Message {
    Answered {
        request: Box<UsageRequest>,
        source: String,
        outcome: Result<Box<Submitted>, String>,
    },
    Executed {
        source: String,
        target: String,
    },
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
                        Ok(submitted) => {
                            let (answer, started) = self.continue_operation(&source, *submitted, ctx);
                            (Ok(answer), started)
                        }
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
        let id = serde_json::from_value::<OperationId>(json!(companion.operation_id))
            .map_err(|_| "cloud_companion_invalid_request: operation_id must be a UUID")?;
        // A refresh holds the source journal while it reaches workers, and may carry
        // the owner's checkbox change, so it finishes; the job waits for its lock.
        let owner = entry.owner.clone();
        companions.agent.hold(&source);
        let sender = companions.agent.sender();
        let groups = self.cloud_prototype.groups.clone();
        let request = Box::new(request.clone());
        let companion = companion.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let outcome = submit(&root, &owner, &groups, &companion, id).map(Box::new);
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
                answer["message"] = json!(reason);
                (answer, false)
            }
        }
    }

    fn execute_on_card(
        &mut self,
        source: &str,
        target: &str,
        (alias, stop): (&str, bool),
        id: OperationId,
        context: Context,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let group = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.as_ref().is_some_and(|launch| launch.id == target))
            .ok_or("The companion cloud is not open in this Horizon; nothing was started")?;
        let card = group.issue;
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("Horizon has no cloud state directory")?;
        let settings = group
            .remote
            .as_ref()
            .map(|launch| Settings::for_cloud(&root.join("settings.json"), &launch.placement))
            .ok_or("The companion cloud has no deployment")?
            .map_err(|error| error.to_string())?;
        let owner = self
            .cloud_prototype
            .production
            .companions
            .entries
            .get(source)
            .map(|entry| entry.owner.clone())
            .ok_or("The source cloud closed; nothing was started")?;
        let runtime = self.cloud_prototype.production.runtimes.entry(card).or_default();
        if runtime.busy() {
            return Err("The companion cloud's card is running another operation; send the request again once it finishes, and it continues this operation".into());
        }
        // A connected card keeps its connection for an Ensure Ready; a Stop ends it
        // first, as the card's own Stop does.
        let tx = match runtime.sender.clone() {
            Some(sender) if !stop && runtime.receiver.is_some() && runtime.stage == Some(Stage::Ready) => sender,
            _ => {
                if let Some(cancel) = runtime.cancel.take() {
                    cancel.cancel();
                }
                runtime.idle_reports = None;
                runtime.desktop = None;
                runtime.confirmation = super::super::Confirmation::None;
                runtime.rebuild = None;
                if runtime.progress.is_deletion() {
                    runtime.progress.reset();
                }
                runtime.error = None;
                runtime.stage = Some(if stop { Stage::Stopping } else { Stage::Provision });
                let (tx, rx) = channel();
                runtime.receiver = Some(rx);
                runtime.sender = Some(tx.clone());
                runtime.cancel = Some(Cancellation::default());
                tx
            }
        };
        let connected = runtime.stage == Some(Stage::Ready) && !stop;
        let cancel = runtime.cancel.clone().unwrap_or_default();
        runtime.push_log(format!(
            "An agent asked to {} this cloud as companion `{alias}`",
            if stop { "stop" } else { "start" }
        ));
        let agent = &mut self.cloud_prototype.production.companions.agent;
        agent.executing.insert(target.to_owned());
        let sender = agent.sender();
        let (source, target, alias) = (source.to_owned(), target.to_owned(), alias.to_owned());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let emit = |event: Event| {
                if matches!(event, Event::Output(_))
                    || (!connected && matches!(event, Event::Stage(..) | Event::Progress(_) | Event::Snapshot(_)))
                {
                    let _ = tx.send(event);
                    ctx.request_repaint();
                }
            };
            let request = lifecycle::Request {
                root: &root,
                owner: &owner,
                context: &context,
                alias: &alias,
            };
            let result = lifecycle::execute(&request, id, &settings, &cancel, &emit);
            let state_root = cloud_runtime::state::cloud_directory(&root, &target).ok();
            let saved = state_root.and_then(|path| Store::lock(&path).and_then(|store| store.load()).ok().flatten());
            for event in finish(result, saved, connected) {
                let _ = tx.send(event);
            }
            let _ = sender.send(Message::Executed { source, target });
            ctx.request_repaint();
        });
        Ok(())
    }
}

/// The events that leave the card where the operation left its cloud.
fn finish(
    result: cloud_runtime::Result<Operation>,
    saved: Option<cloud_runtime::state::Deployment>,
    connected: bool,
) -> Vec<Event> {
    let mut events = Vec::new();
    match result {
        // A card that stayed connected already shows a ready cloud.
        Ok(operation) if operation.phase == Phase::Ready && connected => {}
        // The card connects the ready cloud's sessions as its own Resume does.
        Ok(operation) if operation.phase == Phase::Ready => events.push(Event::Resumed),
        Ok(operation) if operation.phase == Phase::Stopped => match saved {
            Some(state) => events.push(Event::Stopped(Box::new(state))),
            None => events.push(Event::failed("Stopped, but the cloud's record is unavailable".into())),
        },
        Ok(operation) => {
            events.extend(saved.map(|state| Event::Snapshot(Box::new(state))));
            events.push(Event::failed(hint(operation.phase).into()));
        }
        Err(error) => {
            events.extend(saved.map(|state| Event::Snapshot(Box::new(state))));
            events.push(Event::failed(error.to_string()));
        }
    }
    events
}

/// Records the request, binding a checked companion on its first request. Reads
/// never bind or submit anything.
fn submit(
    root: &Path,
    owner: &Owner,
    groups: &CloudGroups,
    companion: &CompanionRequest,
    id: OperationId,
) -> Result<Submitted, String> {
    let context = inventory::prepare(owner, groups, &Cancellation::default()).map_err(|error| error.to_string())?;
    let alias = companion.alias.clone().unwrap_or_default();
    let request = lifecycle::Request {
        root,
        owner,
        context: &context,
        alias: &alias,
    };
    let operation = match companion.action {
        CompanionAction::Status => retry_busy(|| lifecycle::status(&request, id)),
        CompanionAction::EnsureReady | CompanionAction::Stop => retry_busy(|| {
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
    Ok(Submitted {
        operation,
        context,
        alias,
    })
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
        "message": hint(operation.phase),
    })
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
