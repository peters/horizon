//! Agent requests for Upkeep through `cloud_upkeep`: the Upkeep in the caller's workspace,
//! the repositories it could keep moving, proposals that the person confirms in New Upkeep
//! or the repository list, and the board's Remove, Run now and Pause. The boundary is in
//! `docs/upkeep.md`, "Trust boundary of agent requests".
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use horizon_core::WorkspaceId;
use horizon_core::browser::manifest::{
    self,
    provider_usage::{UpkeepOperation, UpkeepRequest, UsageRequest},
};
use horizon_core::upkeep::worker::{self, Checkouts, Config};
use horizon_core::upkeep::{AgentChange, AgentChangeKind, DEFAULT_INSTRUCTIONS};
use serde_json::{Value, json};

use super::super::HorizonApp;
use status::status;

mod caller;
mod status;

/// Agent requests on their way: queued for this frame, or at work on their own thread.
pub(in crate::app) struct Requests {
    queued: Vec<UsageRequest>,
    steps: Sender<Step>,
    finished: Receiver<Step>,
    /// What agents changed, newest first, by cloud; the boards of each cloud show it.
    changes: HashMap<String, Vec<AgentChange>>,
}

impl Default for Requests {
    fn default() -> Self {
        let (steps, finished) = std::sync::mpsc::channel();
        Self {
            queued: Vec::new(),
            steps,
            finished,
            changes: HashMap::new(),
        }
    }
}

/// What a request's thread found: the answer, or a change only the UI thread can make.
struct Step {
    request: UsageRequest,
    next: Next,
    /// The local id of the caller's workspace when the request arrived.
    home: String,
}

enum Next {
    Answer(Result<Value, String>),
    /// Read the status of this Upkeep; only `decide` gives it, and `work` answers it.
    Status {
        cloud: String,
        config: Config,
        running: Option<bool>,
    },
    /// Remove these repositories; only `decide` gives it, and the UI thread starts the save
    /// once the caller still stands where it asked from.
    Remove {
        cloud: String,
        names: Vec<String>,
    },
    /// These repositories left the saved settings of this cloud; the answer follows.
    Saved {
        cloud: String,
        value: Value,
        removed: Vec<String>,
    },
    Signal {
        cloud: String,
        run_now: bool,
        paused: Option<bool>,
    },
    ProposeNew(Config),
    /// The editor of `cloud`, over its settings as `decide` read them, with `repositories`
    /// ticked.
    ProposeRepositories {
        cloud: String,
        saved: Config,
        repositories: Vec<String>,
    },
}

/// What the UI thread knows of the caller's workspace when the request arrives.
struct Caller {
    /// The local id of the workspace: a session switch can give another workspace the same
    /// `WorkspaceId`, never the same local id.
    home: String,
    /// The clouds in the workspace; the Upkeep is the one with saved settings.
    clouds: Vec<String>,
    /// Whether the worker loop of each cloud runs, as the last exchange found.
    loops: Vec<(String, Option<bool>)>,
    /// The clouds of the workspace whose ID another saved cloud also has: which Upkeep
    /// such an ID names is not certain, so a request for it is refused.
    ambiguous: Vec<String>,
}

impl HorizonApp {
    pub(in crate::app) fn queue_upkeep_request(&mut self, request: UsageRequest) {
        self.upkeep.agent.queued.push(request);
    }

    /// Starts the queued Upkeep requests and finishes the ones whose thread is done.
    pub(in crate::app) fn answer_upkeep_requests(&mut self, ctx: &egui::Context) {
        for request in std::mem::take(&mut self.upkeep.agent.queued) {
            match self.caller(&request) {
                Ok((upkeep, caller)) => self.start(request, upkeep, caller, ctx),
                Err(error) => complete(request, Err(error)),
            }
        }
        let mut finished = false;
        while let Ok(step) = self.upkeep.agent.finished.try_recv() {
            self.finish(ctx, step);
            finished = true;
        }
        // This frame drew the dialogs and boards already: draw what the requests changed.
        if finished {
            ctx.request_repaint();
        }
        self.show_agent_changes();
    }

    fn record_agent_change(&mut self, cloud: &str, kind: AgentChangeKind) {
        let changes = self.upkeep.agent.changes.entry(cloud.to_owned()).or_default();
        horizon_core::upkeep::record_agent_change(changes, AgentChange::now(kind));
    }

    /// Gives each board of a cloud with agent changes the latest ones, also a board that a
    /// session switch or a new panel brought. A board knows its cloud once it is linked to
    /// a saved Upkeep; the check allocates nothing.
    fn show_agent_changes(&mut self) {
        let changes = &self.upkeep.agent.changes;
        if changes.is_empty() {
            return;
        }
        for panel in &mut self.board.panels {
            if let Some(board) = panel.content.upkeep_mut()
                && let Some(latest) = board.cloud.as_deref().and_then(|cloud| changes.get(cloud))
                && board.agent_changes != *latest
            {
                board.agent_changes.clone_from(latest);
            }
        }
    }

    /// An Upkeep may have started in `workspace` while the request read the files: ask the
    /// saved Upkeeps as the app knows them now.
    fn still_without_upkeep(&self, workspace: WorkspaceId) -> Result<(), String> {
        let saved = self
            .upkeep
            .saved
            .as_ref()
            .ok_or("upkeep_unavailable: Horizon is reading the saved Upkeeps; try again")?;
        let local_id = self
            .board
            .workspace(workspace)
            .map(|held| held.local_id.as_str())
            .ok_or("upkeep_unavailable: the workspace of the request is gone")?;
        let started = self.cloud_prototype.groups.0.iter().any(|group| {
            group.workspace == local_id
                && group
                    .remote
                    .as_ref()
                    .is_some_and(|remote| saved.iter().any(|(cloud, _)| *cloud == remote.id))
        });
        if started {
            return Err("upkeep_exists: this workspace has an Upkeep; use add to change its repositories".into());
        }
        Ok(())
    }

    fn start(&mut self, request: UsageRequest, upkeep: UpkeepRequest, caller: Caller, ctx: &egui::Context) {
        let root = (upkeep.operation == UpkeepOperation::Discover)
            .then(|| self.upkeep_root())
            .flatten();
        let steps = self.upkeep.agent.steps.clone();
        let repaint = ctx.clone();
        let unanswered = request.clone();
        let spawned = std::thread::Builder::new()
            .name("upkeep-agent-request".into())
            .spawn(move || {
                let next = work(&request, &upkeep, &caller, root.as_deref());
                let home = caller.home;
                let _ = steps.send(Step { request, next, home });
                repaint.request_repaint();
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start an Upkeep request");
            complete(
                unanswered,
                Err("upkeep_unavailable: Horizon could not start the request".into()),
            );
        }
    }

    /// Makes the change a request's thread asked for, then answers. A change happens only
    /// while the caller still is an agent panel in the workspace it asked from: a session
    /// switch or a closed panel ends the request.
    fn finish(&mut self, ctx: &egui::Context, step: Step) {
        let Step { request, next, home } = step;
        let answer = match next {
            Next::Answer(answer) => answer,
            Next::Status { .. } => Err("upkeep_unavailable: the request stopped".into()),
            Next::Remove { cloud, names } => {
                match self
                    .still_caller(&request, &home)
                    .and_then(|_| self.cloud_in(&home, &cloud))
                {
                    Ok(()) => {
                        self.save_removal(ctx, request, home, cloud, names);
                        return;
                    }
                    Err(error) => Err(error),
                }
            }
            // The save is done: the board shows it whatever happened meanwhile.
            Next::Saved { cloud, value, removed } => {
                self.upkeep_settings_saved(&cloud);
                self.record_agent_change(&cloud, AgentChangeKind::Removed(removed));
                Ok(value)
            }
            Next::Signal { cloud, run_now, paused } => self
                .still_caller(&request, &home)
                .and_then(|_| self.cloud_in(&home, &cloud))
                .and_then(|()| expired(&request))
                .map(|()| {
                    self.record_agent_change(
                        &cloud,
                        match paused {
                            Some(true) => AgentChangeKind::Paused,
                            Some(false) => AgentChangeKind::Resumed,
                            None => AgentChangeKind::RunRequested,
                        },
                    );
                    self.signal_upkeep(&cloud, run_now, paused);
                    // The worker gets the signal with the next exchange; `status` shows when.
                    match paused {
                        Some(true) => json!({ "pause": "requested" }),
                        Some(false) => json!({ "resume": "requested" }),
                        None => json!({ "run": "requested" }),
                    }
                }),
            Next::ProposeNew(config) => self.still_caller(&request, &home).and_then(|workspace| {
                expired(&request)?;
                self.still_without_upkeep(workspace)?;
                self.propose_new_upkeep(workspace, &config)?;
                Ok(awaiting("New Upkeep"))
            }),
            Next::ProposeRepositories {
                cloud,
                saved,
                repositories,
            } => self.still_caller(&request, &home).and_then(|workspace| {
                expired(&request)?;
                self.propose_upkeep_repositories(ctx, workspace, &cloud, &saved, &repositories)?;
                Ok(awaiting("the Upkeep's repository list"))
            }),
        };
        complete(request, answer);
    }

    /// Saves a removal on its own thread; its answer comes back as a step.
    fn save_removal(
        &mut self,
        ctx: &egui::Context,
        request: UsageRequest,
        home: String,
        cloud: String,
        names: Vec<String>,
    ) {
        let steps = self.upkeep.agent.steps.clone();
        let repaint = ctx.clone();
        let unanswered = request.clone();
        let spawned = std::thread::Builder::new()
            .name("upkeep-agent-remove".into())
            .spawn(move || {
                let next = remove(&request, cloud, &names);
                let _ = steps.send(Step { request, next, home });
                repaint.request_repaint();
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not save an Upkeep removal");
            complete(
                unanswered,
                Err("upkeep_unavailable: Horizon could not start the request".into()),
            );
        }
    }
}

/// The part of a request that reads or writes files, on the request's own thread.
fn work(request: &UsageRequest, upkeep: &UpkeepRequest, caller: &Caller, root: Option<&std::path::Path>) -> Next {
    if upkeep.operation == UpkeepOperation::Discover {
        return Next::Answer(
            root.ok_or_else(|| "upkeep_unavailable: cloud workspaces are not ready on this computer".to_string())
                .and_then(|root| discover(root, &authors(upkeep))),
        );
    }
    // A folder that cannot be read says nothing about which Upkeeps exist.
    let Ok(saved) = worker::read_saved(true) else {
        return Next::Answer(Err(
            "upkeep_unavailable: could not read the saved Upkeeps; try again".into()
        ));
    };
    // The reader leaves out a choice it cannot parse: that Upkeep still exists.
    let unreadable = caller.clouds.iter().any(|cloud| {
        // A file that cannot even be looked at may be there too.
        !saved.iter().any(|(held, _)| held == cloud)
            && worker::directory(cloud)
                .join("upkeep.json")
                .try_exists()
                .unwrap_or(true)
    });
    if unreadable {
        return Next::Answer(Err(
            "upkeep_unavailable: the saved settings of this workspace's Upkeep cannot be read".into(),
        ));
    }
    match decide(request, upkeep, caller, saved) {
        Ok(Next::Status { cloud, config, running }) => Next::Answer(Ok(status(&cloud, &config, running))),
        Ok(next) => next,
        Err(error) => Next::Answer(Err(error)),
    }
}

/// Removes `names` from the saved settings as they are now: [`worker::update`] holds the
/// Upkeep's settings lock from the read to the save, so a change made meanwhile stays.
fn remove(request: &UsageRequest, cloud: String, names: &[String]) -> Next {
    let mut outcome = None;
    let saved = worker::update(&cloud, |config| outcome = Some(remove_from(config, names, request)));
    match (saved, outcome) {
        (_, Some(Err(error))) => Next::Answer(Err(error)),
        // No change was tried: the saved choice could not be read.
        (Err(error), None) => Next::Answer(Err(format!("upkeep_unavailable: could not read the Upkeep: {error}"))),
        (Err(error), Some(Ok(_))) => Next::Answer(Err(format!("upkeep_unavailable: could not save: {error}"))),
        (Ok(_), None) => Next::Answer(Err("upkeep_unavailable: the request stopped".into())),
        (Ok(config), Some(Ok(removed))) if removed.is_empty() => {
            Next::Answer(Ok(json!({ "removed": 0, "repositories": config.repositories })))
        }
        (Ok(config), Some(Ok(removed))) => Next::Saved {
            value: json!({ "removed": removed.len(), "repositories": config.repositories }),
            cloud,
            removed,
        },
    }
}

/// Removes `names` from `config` and returns the removed names as the settings spell them.
/// An Upkeep keeps at least one repository, and a request past its deadline changes
/// nothing: both leave `config` as it was.
fn remove_from(config: &mut Config, names: &[String], request: &UsageRequest) -> Result<Vec<String>, String> {
    let before = config.clone();
    let outcome = if config.remove(names) == 0 {
        Ok(Vec::new())
    } else if config.repositories.is_empty() {
        Err("upkeep_invalid_request: an Upkeep keeps at least one repository; pause it instead".to_owned())
    } else {
        expired(request).map(|()| {
            before
                .repositories
                .iter()
                .filter(|held| !config.repositories.contains(held))
                .cloned()
                .collect()
        })
    };
    if outcome.is_err() {
        *config = before;
    }
    outcome
}

fn discover(root: &std::path::Path, authors: &[String]) -> Result<Value, String> {
    let catalog =
        horizon_core::upkeep::discover(root, authors).map_err(|error| format!("upkeep_discover_failed: {error}"))?;
    let repositories: Vec<Value> = catalog
        .repositories
        .iter()
        .map(|repository| json!({ "repository": repository.name, "open_pull_requests": repository.open }))
        .collect();
    let mut answer = json!({ "repositories": repositories });
    // The list stands; only the counts are missing.
    if let Some(error) = catalog.search_error {
        answer["search_error"] = Value::String(error);
    }
    Ok(answer)
}

/// What a request does with the saved Upkeeps, without touching a file.
fn decide(
    request: &UsageRequest,
    upkeep: &UpkeepRequest,
    caller: &Caller,
    saved: Vec<(String, Config)>,
) -> Result<Next, String> {
    let found = saved.into_iter().find(|(cloud, _)| caller.clouds.contains(cloud));
    if upkeep.operation == UpkeepOperation::Create {
        if found.is_some() {
            return Err("upkeep_exists: this workspace has an Upkeep; use add to change its repositories".into());
        }
        let config = Config {
            repositories: upkeep.repositories.clone(),
            authors: authors(upkeep),
            agent: worker::AGENT.into(),
            every_minutes: upkeep.every_minutes.unwrap_or(60),
            instructions: upkeep
                .instructions
                .clone()
                .unwrap_or_else(|| DEFAULT_INSTRUCTIONS.into()),
            worker: String::new(),
            checkouts: upkeep
                .checkouts
                .as_deref()
                .and_then(Checkouts::from_name)
                .unwrap_or_default(),
            order: Vec::new(),
        };
        config.validate().map_err(|error| invalid(&error))?;
        return Ok(Next::ProposeNew(config));
    }
    let (cloud, mut config) =
        found.ok_or("upkeep_not_found: this workspace has no Upkeep; use create to propose one")?;
    if caller.ambiguous.contains(&cloud) {
        return Err("upkeep_ambiguous_cloud: more than one cloud has the ID of this Upkeep; use its board".into());
    }
    match upkeep.operation {
        UpkeepOperation::Status => {
            let running = caller
                .loops
                .iter()
                .find(|(held, _)| *held == cloud)
                .and_then(|(_, running)| *running);
            Ok(Next::Status { cloud, config, running })
        }
        UpkeepOperation::Add => {
            let saved = config.clone();
            let added = config.add(&upkeep.repositories);
            if added == 0 {
                return Ok(Next::Answer(Ok(
                    json!({ "added": 0, "repositories": config.repositories }),
                )));
            }
            config.validate().map_err(|error| invalid(&error))?;
            let repositories = config.repositories[config.repositories.len() - added..].to_vec();
            Ok(Next::ProposeRepositories {
                cloud,
                saved,
                repositories,
            })
        }
        UpkeepOperation::Remove => {
            // A first answer from the list read now; `remove` decides again under the lock.
            if remove_from(&mut config, &upkeep.repositories, request)?.is_empty() {
                return Ok(Next::Answer(Ok(
                    json!({ "removed": 0, "repositories": config.repositories }),
                )));
            }
            config.validate().map_err(|error| invalid(&error))?;
            Ok(Next::Remove {
                cloud,
                names: upkeep.repositories.clone(),
            })
        }
        UpkeepOperation::RunNow => Ok(Next::Signal {
            cloud,
            run_now: true,
            paused: None,
        }),
        UpkeepOperation::Pause | UpkeepOperation::Resume => Ok(Next::Signal {
            cloud,
            run_now: false,
            paused: Some(upkeep.operation == UpkeepOperation::Pause),
        }),
        UpkeepOperation::Discover | UpkeepOperation::Create => Err("upkeep_invalid_request".into()),
    }
}

/// A change after the caller stopped waiting would be one it never saw.
fn expired(request: &UsageRequest) -> Result<(), String> {
    if manifest::now_millis() >= request.deadline_at_millis {
        Err("upkeep_expired: the request expired before Horizon answered it; nothing changed".into())
    } else {
        Ok(())
    }
}

fn invalid(error: &horizon_core::cloud_runtime::Error) -> String {
    format!("upkeep_invalid_request: {error}")
}

fn awaiting(dialog: &str) -> Value {
    json!({
        "phase": "awaiting_person",
        "message": format!("Horizon shows {dialog} with your proposal. Nothing changes until the person confirms it; read status later."),
    })
}

/// The bots asked for, or Dependabot.
fn authors(upkeep: &UpkeepRequest) -> Vec<String> {
    if upkeep.authors.is_empty() {
        vec!["app/dependabot".into()]
    } else {
        upkeep.authors.clone()
    }
}

/// Writes the answer off the UI thread.
fn complete(request: UsageRequest, answer: Result<Value, String>) {
    let write = move || {
        let mut result = request.result(Vec::new(), None);
        match answer {
            Ok(value) => result.upkeep = Some(value),
            Err(error) => result.error = Some(error),
        }
        if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
            tracing::warn!(kind = ?error.kind(), "could not answer an Upkeep request");
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("upkeep-agent-answer".into())
        .spawn(write)
    {
        tracing::warn!(%error, "could not answer an Upkeep request");
    }
}

#[cfg(test)]
mod tests;
