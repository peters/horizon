//! Runs a cloud's image rebuild, or continues or cancels a pending one, off the UI
//! thread. The build, switch and reconnect live in `deployment::replacement`.
use super::{
    Confirmation, Event, HorizonApp, Request, Runtime, Settings, Stage, cloud_runtime, lifecycle::Action, presentation,
};
use horizon_core::cloud_runtime::deployment::replacement;
use std::{
    cell::Cell,
    path::PathBuf,
    sync::mpsc::{Sender, channel},
    time::Instant,
};

#[cfg(test)]
pub(super) mod tests;

/// Notes kept per attempt; lost sessions are listed one per line.
const NOTE_LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Rebuild,
    Continue,
    Cancel,
}

impl Kind {
    pub(super) const fn of(action: Action) -> Option<Self> {
        match action {
            Action::Rebuild => Some(Self::Rebuild),
            Action::ContinueRebuild => Some(Self::Continue),
            Action::CancelRebuild => Some(Self::Cancel),
            _ => None,
        }
    }

    pub(super) const fn heading(self) -> &'static str {
        match self {
            Self::Rebuild => "Rebuilding image",
            Self::Continue => "Continuing image rebuild",
            Self::Cancel => "Cancelling image rebuild",
        }
    }
}

/// The latest rebuild operation on a cloud, kept until another deployment starts so
/// its steps and outcome stay on the card.
pub(super) struct Attempt {
    pub(super) kind: Kind,
    pub(super) started: Instant,
    pub(super) notes: Vec<Note>,
}

pub(super) struct Note {
    pub(super) text: String,
    pub(super) warning: bool,
}

impl Attempt {
    pub(super) fn new(kind: Kind) -> Self {
        Self {
            kind,
            started: Instant::now(),
            notes: Vec::new(),
        }
    }

    /// Keeps the outcomes `deployment::replacement` reports as output lines; the rest
    /// of the output stays in the verbose log only.
    fn observe(&mut self, line: &str) {
        let warning = line.starts_with("Session ") && line.ends_with("was not relaunched");
        let notable = warning
            || line.starts_with("Image unchanged")
            || line.starts_with("Image replacement cancelled")
            || line.starts_with("Agent CLI releases:");
        if notable && self.notes.len() < NOTE_LIMIT {
            self.notes.push(Note {
                text: line.to_owned(),
                warning,
            });
        }
    }
}

impl Runtime {
    pub(super) fn observe_rebuild(&mut self, event: &Event) {
        if let (Some(attempt), Event::Output(line)) = (&mut self.rebuild, event) {
            attempt.observe(line);
        }
    }

    /// The rebuild restarts the worker, so the presentation watch and desktop tunnel
    /// stop first; a successful attempt ends in `Ready` and reattaches them.
    fn start_rebuild(&mut self, request: Request, kind: Kind, profile_name: String, ctx: &egui::Context) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.desktop = None;
        self.confirmation = Confirmation::None;
        self.progress.reset();
        self.error = None;
        self.rebuild = Some(Attempt::new(kind));
        // Core reports its first stage at once; until then the first step stands in.
        self.stage = Some(Stage::Validate);
        let (tx, rx) = channel();
        let cancel = cloud_runtime::Cancellation::default();
        self.cancel = Some(cancel.clone());
        self.receiver = Some(rx);
        self.sender = Some(tx.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || run(&request, kind, &profile_name, &cancel, &tx, &ctx));
    }
}

impl HorizonApp {
    pub(super) fn start_production_rebuild(&mut self, id: u32, kind: Kind, ctx: &egui::Context) {
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == id) else {
            return;
        };
        let Some(launch) = group.remote.clone() else { return };
        let repository = group.cwd.clone();
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        if runtime.busy() || runtime.state_unavailable {
            return;
        }
        let profile_name = launch.profile_name.clone();
        match request(&root, launch, repository) {
            Ok(request) => runtime.start_rebuild(request, kind, profile_name, ctx),
            Err(error) => runtime.error = Some(error.to_string()),
        }
    }
}

fn request(
    root: &std::path::Path,
    launch: horizon_core::cloud_panel::CloudLaunch,
    repository: PathBuf,
) -> cloud_runtime::Result<Request> {
    Ok(Request {
        state_root: cloud_runtime::state::cloud_directory(root, &launch.id)?,
        settings: Settings::load(&root.join("settings.json"))?,
        cloud_id: launch.id,
        repository,
        revision: launch.revision,
        profile: launch.profile,
    })
}

fn run(
    request: &Request,
    kind: Kind,
    profile_name: &str,
    cancel: &cloud_runtime::Cancellation,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) {
    let ready = Cell::new(false);
    let emit = |event: Event| {
        ready.set(ready.get() || matches!(event, Event::Ready(..)));
        let _ = tx.send(event);
        ctx.request_repaint();
    };
    let result = match kind {
        Kind::Rebuild => replacement::rebuild(request, profile_name, cancel, &emit),
        Kind::Continue => replacement::continue_replacement(request, cancel, &emit),
        Kind::Cancel => replacement::cancel_replacement(request, cancel, &emit),
    };
    match result {
        Ok(state) if ready.get() => {
            presentation::watch(&state, &request.settings, &request.state_root, cancel, tx, ctx);
        }
        // Dropping an unsent replacement leaves the worker as it was without
        // reconnecting; reconnect to restore the presentation stopped for it.
        Ok(_) => super::run_deployment(request, cancel, tx, ctx),
        Err(error) => super::report_failure(&request.state_root, &error, &emit),
    }
}
