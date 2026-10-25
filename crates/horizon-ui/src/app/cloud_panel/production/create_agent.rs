//! Cloud create requests of agents. Horizon rents one quick-start cloud in the caller's
//! workspace, on this machine's own provider credentials, and shows it on the card a
//! person's deployment uses. The deployment runs on a thread this host owns; the agent
//! polls it with the operation ID it chose and can only ask for a stop.
use super::preparation::Durability;
use super::{HorizonApp, Runtime, Stage, cloud_runtime, deployment, presentation};
use crate::app::browser_requests::actor_panel;
use egui::Context;
use horizon_core::browser::manifest::{
    self,
    provider_usage::{CloudCreateOperation, CloudCreateRequest, UsageRequest},
};
use horizon_core::cloud_panel::{CloudLaunch, Placement};
use horizon_core::cloud_runtime::prices::Profile;
use horizon_core::cloud_runtime::{
    Cancellation, Event,
    deployment::Request,
    github::Prompt,
    prices,
    repository::launch::{self, Configuration},
    settings::{AgentLifecycleRefusal, Settings},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::mpsc::{Receiver, Sender, channel},
    time::Duration,
};

/// How many lines of its output an operation's status carries.
const RING_LINES: usize = 200;
/// A status answer shows a moving stage, so a frame during an operation repaints sooner.
const POLL_REPAINT: Duration = Duration::from_millis(200);

const REFUSED: &str = "cloud_create_refused: no agent lifecycle policy is configured";
const NEEDS_PERSON: &str = "cloud_create_needs_person: the worker asked for a GitHub sign-in and the policy does not allow continuing without one; a worker may already exist, so run cloud_list and stop it if it exists, then sign in on its card";
const SKIPPED_SIGN_IN: &str = "Continuing without GitHub access, as this machine's agent policy allows";
const CANCELLED: &str =
    "cloud_create_cancelled: the worker may have been rented; run cloud_list and stop it if it exists";
const CARD_REMOVED: &str =
    "cloud_create_card_removed: the cloud's card was removed before Horizon rented its worker; nothing was rented";
const NEEDS_SESSION: &str = "cloud_create_needs_persistent_session: a quick-start cloud belongs to a saved session; save this workspace in a Horizon session first";
const INVALID: &str = "cloud_create_invalid_request: operation_id is one lowercase UUID, and submit needs a repository and quick_start true";
const UNAVAILABLE: &str = "cloud_create_unavailable: requires a Horizon agent panel";

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
    workspace: String,
    cloud: String,
    name: String,
    phase: Phase,
    /// The card the deployment shows on, once the cloud is registered.
    card: Option<u32>,
    /// What the ready cloud is: its worker, rate and image.
    ready: Option<Value>,
    /// Why the operation failed, or that a cancel asked for its stop.
    error: Option<String>,
    /// Stops the reading or the deployment of this operation.
    cancel: Option<Cancellation>,
    /// Whether this machine's policy continues a cloud whose worker asks for a GitHub
    /// sign-in, instead of failing the operation.
    skip_sign_in: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Horizon accepted the operation and reads the repository.
    Submitted,
    /// The cloud's card shows the deployment of its worker.
    RollingOut,
    Ready,
    Failed,
    /// A cancel asked for the stop; the worker may still have been rented.
    Cancelled,
}

impl Phase {
    const fn label(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::RollingOut => "rolling_out",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    const fn done(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Cancelled)
    }
}

/// What an operation's threads report to the host that owns it.
enum Message {
    /// The repository, its commit, the profile and the price are read and the policy
    /// admits them; the deployment request and that policy's GitHub answer are ready for
    /// the cloud's card.
    Prepared {
        operation: String,
        request: Box<Request>,
        skip_sign_in: bool,
    },
    /// The session that names the cloud is durable, so a worker may be rented for it.
    Durable { operation: String, request: Box<Request> },
    /// Nothing was rented, and why.
    Refused { operation: String, error: String },
    /// The deployment ended: the ready cloud, or why it failed.
    Finished {
        operation: String,
        ready: Option<Box<Value>>,
        error: Option<String>,
    },
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

    /// Whether an agent's create operation owns this cloud's deployment. The card's own
    /// start paths refuse while one does, so a cloud is never deployed twice.
    pub(super) fn owns(&self, card: u32) -> bool {
        self.operations
            .values()
            .any(|operation| operation.card == Some(card) && !operation.phase.done())
    }

    fn running(&self) -> bool {
        self.operations.values().any(|operation| !operation.phase.done())
    }

    /// Whether this operation ended, or a cancel already asked it to stop. Nothing it
    /// still reports may register a cloud, take a card or rent a worker after that.
    fn ended(&self, operation: &str) -> bool {
        self.operations.get(operation).is_none_or(|running| {
            running.phase.done() || running.cancel.as_ref().is_some_and(Cancellation::is_cancelled)
        })
    }
}

impl HorizonApp {
    pub(in crate::app) fn queue_cloud_create_request(&mut self, request: UsageRequest) {
        self.cloud_prototype.production.create_agent.queued.push(request);
    }

    /// Answers the queued create requests and applies what the operations' threads report.
    pub(in crate::app::cloud_panel) fn poll_cloud_create_requests(&mut self, ctx: &egui::Context) {
        for request in std::mem::take(&mut self.cloud_prototype.production.create_agent.queued) {
            let answer = self.answer_cloud_create(&request, ctx);
            complete(&request, answer);
        }
        for message in self.cloud_prototype.production.create_agent.messages() {
            self.apply_create_message(message, ctx);
        }
        if self.cloud_prototype.production.create_agent.running() {
            ctx.request_repaint_after(POLL_REPAINT);
        }
    }

    fn answer_cloud_create(&mut self, request: &UsageRequest, ctx: &Context) -> Result<Value, String> {
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&self.board, &request.actor))
            .flatten()
            .and_then(|panel| self.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or(UNAVAILABLE)?;
        let create = request
            .cloud_create
            .as_ref()
            .filter(|create| create.valid())
            .ok_or(INVALID)?;
        match create.operation {
            CloudCreateOperation::Status => self.create_status(&request.actor, create),
            CloudCreateOperation::Cancel => self.create_cancel(&request.actor, create),
            CloudCreateOperation::Submit => {
                // A caller that stopped waiting gets no cloud it did not see.
                if manifest::now_millis() >= request.deadline_at_millis {
                    return Err(
                        "cloud_create_expired: the request expired before Horizon answered it; nothing was rented"
                            .into(),
                    );
                }
                self.submit_create(&request.actor, &workspace, create, ctx)
            }
        }
    }

    fn submit_create(
        &mut self,
        actor: &str,
        workspace: &str,
        create: &CloudCreateRequest,
        ctx: &Context,
    ) -> Result<Value, String> {
        let id = create.operation_id().to_owned();
        if let Some(known) = self.cloud_prototype.production.create_agent.operations.get(&id) {
            if known.actor != actor {
                return Err("cloud_create_unknown_operation: this operation belongs to another agent".into());
            }
            // The answer was lost and the caller sent the same operation again: no second cloud.
            return Ok(status(known, None));
        }
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("cloud_create_unavailable: Horizon has no cloud state directory")?;
        if !self.active_session.as_ref().is_some_and(|session| session.persistent) {
            return Err(NEEDS_SESSION.into());
        }
        let repository = create.repository.clone().unwrap_or_default();
        let cloud = cloud_runtime::new_id();
        let name = create.name.clone().unwrap_or_else(|| folder_name(&repository));
        let cancel = Cancellation::default();
        let state = &mut self.cloud_prototype.production.create_agent;
        state.operations.insert(
            id.clone(),
            Operation {
                id: id.clone(),
                actor: actor.into(),
                workspace: workspace.into(),
                cloud: cloud.clone(),
                name,
                phase: Phase::Submitted,
                card: None,
                ready: None,
                error: None,
                cancel: Some(cancel.clone()),
                skip_sign_in: false,
            },
        );
        let sender = state.sender();
        let ctx = ctx.clone();
        let (thread_id, thread_cloud) = (id.clone(), cloud.clone());
        // The settings, the repository, its commit and its price are read off this thread.
        std::thread::spawn(move || {
            let answer = prepare(&root, (&repository, thread_cloud), &cancel);
            let message = match answer {
                Ok((request, skip_sign_in)) => Message::Prepared {
                    operation: thread_id,
                    request,
                    skip_sign_in,
                },
                Err(error) => Message::Refused {
                    operation: thread_id,
                    error,
                },
            };
            let _ = sender.send(message);
            ctx.request_repaint();
        });
        Ok(json!({ "operation_id": id, "phase": Phase::Submitted.label(), "cloud": cloud }))
    }

    fn apply_create_message(&mut self, message: Message, ctx: &Context) {
        match message {
            Message::Prepared {
                operation,
                request,
                skip_sign_in,
            } => {
                // A caller that cancelled while the repository was read rents nothing.
                if self.cloud_prototype.production.create_agent.ended(&operation) {
                    return;
                }
                if let Some(running) = self
                    .cloud_prototype
                    .production
                    .create_agent
                    .operations
                    .get_mut(&operation)
                {
                    running.skip_sign_in = skip_sign_in;
                }
                match self.register_create_cloud(&operation, &request) {
                    Ok((_card, durability)) => {
                        let sender = self.cloud_prototype.production.create_agent.sender();
                        let ctx = ctx.clone();
                        // The session that names the cloud is durable before any worker is rented.
                        std::thread::spawn(move || {
                            let message = match durability.sync() {
                                Ok(()) => Message::Durable { operation, request },
                                Err(error) => Message::Refused { operation, error },
                            };
                            let _ = sender.send(message);
                            ctx.request_repaint();
                        });
                    }
                    Err(error) => self.fail_create(&operation, error),
                }
            }
            Message::Refused { operation, error } => self.fail_create(&operation, error),
            Message::Durable { operation, request } => self.start_create_deployment(&operation, request, ctx),
            Message::Finished {
                operation,
                ready,
                error,
            } => {
                let Some(running) = self
                    .cloud_prototype
                    .production
                    .create_agent
                    .operations
                    .get_mut(&operation)
                else {
                    return;
                };
                // The first outcome stands: a cancel, or an earlier refusal.
                if running.phase.done() {
                    return;
                }
                running.phase = if error.is_some() { Phase::Failed } else { Phase::Ready };
                running.ready = ready.map(|ready| *ready);
                running.error = error;
                running.cancel = None;
            }
        }
    }

    /// Registers the cloud on the card a person's deployment shows on, in the caller's
    /// workspace, and takes the session fence that must hold before a worker is rented.
    fn register_create_cloud(&mut self, operation: &str, request: &Request) -> Result<(u32, Durability), String> {
        let Some(running) = self.cloud_prototype.production.create_agent.operations.get(operation) else {
            return Err("cloud_create_unknown_operation: no operation of yours has this ID".into());
        };
        let (workspace, name) = (running.workspace.clone(), running.name.clone());
        let duplicate = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .any(|group| group.workspace == workspace && group.remote.is_some() && group.cwd == request.repository);
        if duplicate {
            return Err(format!(
                "cloud_create_busy: your workspace already has a cloud for {}",
                request.repository.display()
            ));
        }
        let launch = CloudLaunch {
            publishing_selection: None,
            // The deployment starts here, so the card must never start its own.
            deployment_started: true,
            id: request.cloud_id.clone(),
            revision: request.revision.clone(),
            profile_name: request.profile_name.clone().unwrap_or_default(),
            profile: request.profile.clone(),
            placement: Placement::default(),
        };
        let card = self
            .add_cloud_group(name, workspace, request.repository.clone(), launch, Vec::new(), None)
            .map_err(|error| format!("cloud_create_failed: {error}"))?;
        if let Some(running) = self
            .cloud_prototype
            .production
            .create_agent
            .operations
            .get_mut(operation)
        {
            running.card = Some(card);
        }
        let durability = self
            .save_cloud_before_allocation(card)
            .ok_or("cloud_create_needs_persistent_session: Horizon could not save this workspace before renting")?;
        Ok((card, durability))
    }

    fn fail_create(&mut self, operation: &str, error: String) {
        let discard = {
            let Some(running) = self
                .cloud_prototype
                .production
                .create_agent
                .operations
                .get_mut(operation)
            else {
                return;
            };
            // The first outcome stands: a cancel, or the refusal whose thread reported first.
            if running.phase.done() {
                return;
            }
            // A cloud that never started its deployment leaves no card to remove later.
            let discard = (running.phase == Phase::Submitted).then_some(running.card).flatten();
            running.phase = Phase::Failed;
            running.error = Some(error);
            running.cancel = None;
            discard
        };
        if let Some(card) = discard {
            self.discard_create_card(operation, card);
        }
    }

    /// Removes the card an operation created before its deployment started, so a cloud
    /// that never rented a worker leaves nothing the person must remove later, and the
    /// same repository may be submitted again.
    fn discard_create_card(&mut self, operation: &str, card: u32) {
        if let Some(running) = self
            .cloud_prototype
            .production
            .create_agent
            .operations
            .get_mut(operation)
        {
            running.card = None;
        }
        let groups = &mut self.cloud_prototype.groups.0;
        let before = groups.len();
        groups.retain(|group| group.issue != card);
        if groups.len() == before {
            return;
        }
        self.cloud_prototype.groups.reconcile(&mut self.board);
        self.cloud_prototype.production.runtimes.remove(&card);
        self.save_cloud_prototype();
    }

    /// Starts the deployment on the cloud's card, as the card's own Start does, with the
    /// events of this operation reaching both the card and its status.
    fn start_create_deployment(&mut self, operation: &str, request: Box<Request>, ctx: &Context) {
        let Some((card, skip, workspace)) = self
            .cloud_prototype
            .production
            .create_agent
            .operations
            .get(operation)
            .and_then(|running| Some((running.card?, running.skip_sign_in, running.workspace.clone())))
        else {
            return;
        };
        // A cancel during the fence, or a later message of an operation that ended, rents
        // nothing: its token is never replaced by the fresh one a deployment starts with.
        if self.cloud_prototype.production.create_agent.ended(operation) {
            return;
        }
        // The person may have removed the card while the session was made durable.
        if !self
            .cloud_prototype
            .groups
            .0
            .iter()
            .any(|group| group.issue == card && group.workspace == workspace)
        {
            if let Some(running) = self
                .cloud_prototype
                .production
                .create_agent
                .operations
                .get_mut(operation)
                && let Some(cancel) = running.cancel.take()
            {
                cancel.cancel();
            }
            self.fail_create(operation, CARD_REMOVED.into());
            return;
        }
        let runtime = self.cloud_prototype.production.runtimes.entry(card).or_default();
        let (tx, cancel) = runtime.begin_deployment(&request);
        if let Some(running) = self
            .cloud_prototype
            .production
            .create_agent
            .operations
            .get_mut(operation)
        {
            running.phase = Phase::RollingOut;
            running.cancel = Some(cancel.clone());
        }
        let sender = self.cloud_prototype.production.create_agent.sender();
        let ctx = ctx.clone();
        let operation = operation.to_owned();
        std::thread::spawn(move || {
            deploy(&request, &cancel, (&tx, &ctx), (&sender, &operation), skip);
            ctx.request_repaint();
        });
    }

    fn create_status(&self, actor: &str, create: &CloudCreateRequest) -> Result<Value, String> {
        let state = &self.cloud_prototype.production.create_agent;
        let running = state
            .operations
            .get(create.operation_id())
            .filter(|running| running.actor == actor)
            .ok_or("cloud_create_unknown_operation: no operation of yours has this ID")?;
        let runtime = running
            .card
            .and_then(|card| self.cloud_prototype.production.runtimes.get(&card));
        Ok(status(running, runtime))
    }

    fn create_cancel(&mut self, actor: &str, create: &CloudCreateRequest) -> Result<Value, String> {
        let (answer, discard) = {
            let state = &mut self.cloud_prototype.production.create_agent;
            let running = state
                .operations
                .get_mut(create.operation_id())
                .filter(|running| running.actor == actor)
                .ok_or("cloud_create_unknown_operation: no operation of yours has this ID")?;
            if running.phase == Phase::Ready {
                return Err("cloud_create_done: the cloud is ready; cloud_list shows it, and its card stops it".into());
            }
            let mut discard = None;
            if !running.phase.done() {
                if let Some(cancel) = running.cancel.take() {
                    cancel.cancel();
                }
                // Nothing was rented yet, so the card this operation made goes again.
                discard = (running.phase == Phase::Submitted).then_some(running.card).flatten();
                running.phase = Phase::Cancelled;
                running.error = Some(CANCELLED.into());
            }
            (
                json!({
                    "operation_id": running.id,
                    "phase": running.phase.label(),
                    "cloud": running.cloud,
                    "error": running.error,
                }),
                discard,
            )
        };
        if let Some(card) = discard {
            self.discard_create_card(create.operation_id(), card);
        }
        Ok(answer)
    }
}

/// The answer one operation gives: what it is, where it stands, and its last output.
fn status(running: &Operation, runtime: Option<&Runtime>) -> Value {
    json!({
        "operation_id": running.id,
        "phase": running.phase.label(),
        "done": running.phase.done(),
        "cloud": running.cloud,
        "name": running.name,
        "stage": runtime.and_then(|runtime| runtime.stage).map(Stage::label),
        "lines": runtime.map_or_else(Vec::new, |runtime| {
            runtime.log_tail(RING_LINES)
        }),
        "ready": running.ready,
        "error": running.error,
    })
}

fn complete(request: &UsageRequest, answer: Result<Value, String>) {
    let mut result = request.result(Vec::new(), None);
    match answer {
        Ok(value) => result.cloud_create = Some(value),
        Err(error) => result.error = Some(error),
    }
    if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
        tracing::warn!(kind = ?error.kind(), "could not answer a cloud create request");
    }
}

/// The cloud's name on its card: the name the agent chose, or the repository's folder.
fn folder_name(repository: &str) -> String {
    Path::new(repository)
        .file_name()
        .map_or_else(|| repository.to_owned(), |name| name.to_string_lossy().into_owned())
}

/// Reads this machine's settings, the repository, its commit, the profile and its price,
/// and refuses a cloud this machine's policy does not admit. Nothing is recorded and
/// nothing is rented. It runs on a worker thread, because it reads files.
fn prepare(
    root: &Path,
    (repository, cloud): (&str, String),
    cancel: &Cancellation,
) -> Result<(Box<Request>, bool), String> {
    let settings = Settings::for_cloud(&root.join("settings.json"), &Placement::default())
        .map_err(|error| format!("cloud_create_refused: the cloud settings could not be read: {error}"))?;
    let policy = settings.agent_lifecycle.clone().ok_or(REFUSED)?;
    let runner = cloud_runtime::command::Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let prepared = launch::prepare_with_configuration(repository, "HEAD", Configuration::QuickStart, &runner).map_err(
        |error| {
            if launch::quick_start::is_refused(&error) {
                "cloud_create_needs_profile: this commit has its own .horizon/cloud.yml, so quick start does not apply"
                    .to_owned()
            } else {
                format!("cloud_create_failed: {error}")
            }
        },
    )?;
    let profile_name = prepared.config.default.clone();
    let profile = prepared
        .config
        .profiles
        .get(&profile_name)
        .cloned()
        .ok_or("cloud_create_needs_profile: the built-in quick-start profile is invalid")?;
    let hourly = quote(&profile, &settings, cancel);
    policy
        .admits(&prepared.repository, &profile.provider, hourly)
        .map_err(|refusal| refused(refusal, hourly, policy.max_hourly))?;
    let state_root = cloud_runtime::state::cloud_directory(root, &cloud).map_err(|error| error.to_string())?;
    let skip_sign_in = policy.allow_github_sign_in_skip;
    let mut request = Request::new(
        cloud,
        prepared.repository,
        prepared.revision,
        profile,
        state_root,
        settings,
    );
    request.profile_name = Some(profile_name);
    Ok((Box::new(request), skip_sign_in))
}

/// What this machine may be charged for `profile` per hour, from the provider's own
/// prices. A cloud cannot be checked against the policy limit without one.
fn quote(profile: &Profile, settings: &Settings, cancel: &Cancellation) -> Option<f64> {
    let (list, preferences) = prices::price_list(settings, cancel).ok()?;
    prices::watch::Selection {
        profile: profile.clone(),
        placement: Placement::default(),
    }
    .hourly(&list, &preferences)
}

fn refused(refusal: AgentLifecycleRefusal, hourly: Option<f64>, max_hourly: f64) -> String {
    match (refusal, hourly) {
        (AgentLifecycleRefusal::Above, Some(hourly)) => format!(
            "cloud_create_refused: {} ({hourly:.2} per hour is above {max_hourly:.2})",
            refusal.message()
        ),
        _ => format!("cloud_create_refused: {}", refusal.message()),
    }
}

/// What this operation answers when a worker asks for a GitHub sign-in: the line the card's
/// output shows, and whether it continues without GitHub access. `None` for an event that
/// asks nothing of the person, such as a worker that already holds access.
fn sign_in(event: &Event, skip_sign_in: bool) -> Option<(&'static str, bool)> {
    if !matches!(event, Event::GitHub(Prompt::Web { .. } | Prompt::Device { .. })) {
        return None;
    }
    Some(if skip_sign_in {
        (SKIPPED_SIGN_IN, true)
    } else {
        (NEEDS_PERSON, false)
    })
}

/// Ends one deployment for its agent. The outcome goes to the agent first, so a ready
/// cloud is done for it, and the card's presentation then keeps its watch on this thread,
/// as the card's own deployment does. Without that order a ready cloud would stay
/// `rolling_out` and hold its card for as long as the watch runs.
fn finish_deployment(
    outcome: Result<cloud_runtime::state::Deployment, String>,
    report: impl FnOnce(Option<Value>, Option<String>),
    watch: impl FnOnce(&cloud_runtime::state::Deployment),
) -> Result<cloud_runtime::state::Deployment, String> {
    match &outcome {
        Ok(state) => {
            report(Some(ready_facts(state)), None);
            watch(state);
        }
        Err(error) => report(None, Some(error.clone())),
    }
    outcome
}

/// Deploys the cloud and keeps its card and its status fed. A worker that asks for a
/// GitHub sign-in never waits for a person here: this operation either continues without
/// GitHub access, when the policy allows it, or fails without using the worker.
fn deploy(
    request: &Request,
    cancel: &Cancellation,
    (tx, ctx): (&Sender<Event>, &Context),
    (sender, operation): (&Sender<Message>, &str),
    skip_sign_in: bool,
) {
    let refused = std::cell::Cell::new(false);
    let emit = |event: Event| {
        if let Some((line, continue_without)) = sign_in(&event, skip_sign_in) {
            if continue_without {
                // The non-UI answer the card offers the person, without the card.
                cloud_runtime::github::skip(&request.cloud_id);
            } else {
                refused.set(true);
                cancel.cancel();
            }
            // No sign-in prompt reaches a card for a cloud nobody watched being created.
            let _ = tx.send(Event::Output(line.to_owned()));
            ctx.request_repaint();
            return;
        }
        let _ = tx.send(event);
        ctx.request_repaint();
    };
    let failed = |error: &cloud_runtime::Error| {
        super::report_failure(&request.state_root, error, &emit);
        if refused.get() {
            NEEDS_PERSON.to_owned()
        } else {
            error.to_string()
        }
    };
    let report = |ready: Option<Value>, error: Option<String>| {
        let _ = sender.send(Message::Finished {
            operation: operation.to_owned(),
            ready: ready.map(Box::new),
            error,
        });
        ctx.request_repaint();
    };
    let _ = finish_deployment(
        deployment::deploy(request, cancel, &emit).map_err(|error| failed(&error)),
        report,
        // As the card's own deployment: its sessions, desktop and browsers connect, and
        // this watch runs on this thread for as long as the cloud is live.
        |state| presentation::watch(state, &request.settings, &request.state_root, cancel, tx, ctx),
    );
}

/// What an agent needs to know about a ready cloud, and to stop it later.
fn ready_facts(state: &cloud_runtime::state::Deployment) -> Value {
    let worker = state.worker.as_ref();
    json!({
        "cloud": state.cloud_id,
        "worker": worker.map(|worker| worker.id.clone()),
        "hourly": worker.and_then(cloud_runtime::cost::hourly_rate),
        "image": state.profile.image,
        "repository": state.repository,
        "revision": state.revision,
    })
}

#[cfg(test)]
mod tests;
