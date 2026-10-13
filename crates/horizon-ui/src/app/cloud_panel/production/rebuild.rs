//! Runs a cloud's image rebuild, or continues or cancels a pending one, off the UI
//! thread, and reads what the rebuild changes for its confirmation. The plan, build,
//! switch and reconnect live in `deployment::replacement`.
use super::{
    Confirmation, Event, HorizonApp, Request, Runtime, Settings, Stage, Store, cloud_runtime, lifecycle::Action,
    presentation,
};
use horizon_core::{
    cloud_panel::Profile,
    cloud_runtime::deployment::replacement::{self, Plan},
};
use std::{
    cell::Cell,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, TryRecvError, channel},
    time::Instant,
};

#[cfg(test)]
pub(super) mod tests;

/// Notes kept per attempt; lost sessions are listed one per line.
const NOTE_LIMIT: usize = 20;
/// Starts the note for a rebuild refused before anything was journaled.
const REFUSED: &str = "Rebuild refused:";
const UNREAD: &str = "The latest commit could not be read; close this and try again";

/// What the latest commit changes in a cloud's settings, read off the UI thread while
/// the rebuild's confirmation is open, so the person confirms exactly that.
#[derive(Default)]
pub(super) struct Preview {
    reading: Option<Receiver<Result<Plan, String>>>,
    read: Option<Result<Plan, String>>,
}

impl Preview {
    /// The plan or the reason it was refused, once read.
    pub(super) fn read(&mut self) -> Option<&Result<Plan, String>> {
        if let Some(reading) = &self.reading {
            match reading.try_recv() {
                Ok(read) => self.read = Some(read),
                Err(TryRecvError::Disconnected) => self.read = Some(Err(UNREAD.into())),
                Err(TryRecvError::Empty) => return None,
            }
            self.reading = None;
        }
        self.read.as_ref()
    }

    /// The profile the person confirms by starting the rebuild.
    fn confirmed(&self) -> Option<Profile> {
        Some(self.read.as_ref()?.as_ref().ok()?.profile.clone())
    }

    #[cfg(test)]
    pub(super) fn of(read: Result<Plan, String>) -> Box<Self> {
        Box::new(Self {
            reading: None,
            read: Some(read),
        })
    }
}

/// Two previews are equal while they show the same plan; one being read equals another.
impl PartialEq for Preview {
    fn eq(&self, other: &Self) -> bool {
        self.read == other.read && self.reading.is_some() == other.reading.is_some()
    }
}

impl Eq for Preview {}

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
        let warning =
            (line.starts_with("Session ") && line.ends_with("was not relaunched")) || line.starts_with(REFUSED);
        let notable = warning
            || line.starts_with(replacement::PROFILE_CHANGE)
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
    fn start_rebuild(
        &mut self,
        request: Request,
        kind: Kind,
        profile_name: String,
        confirmed: Option<Profile>,
        ctx: &egui::Context,
    ) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.desktop = None;
        self.confirmation = Confirmation::None;
        self.progress.reset();
        self.operation = None;
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
        let idle = self.listen_idle();
        std::thread::spawn(move || {
            let operation = Operation {
                kind,
                profile_name: &profile_name,
                confirmed: confirmed.as_ref(),
            };
            run(&request, &operation, &cancel, &tx, idle, &ctx);
        });
    }
}

impl HorizonApp {
    /// Starts what a rebuild `action` asks for; `false` for any other action.
    pub(super) fn rebuild_action(&mut self, id: u32, action: Action, ctx: &egui::Context) -> bool {
        if action == Action::PreviewRebuild {
            self.preview_production_rebuild(id, ctx);
            return true;
        }
        let Some(kind) = Kind::of(action) else {
            return false;
        };
        self.start_production_rebuild(id, kind, ctx);
        true
    }

    /// Reads, for the open confirmation of cloud `id`, what a rebuild would change. The
    /// cloud's record names its repository and its bound profile.
    fn preview_production_rebuild(&mut self, id: u32, ctx: &egui::Context) {
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == id) else {
            return;
        };
        let Some(profile_name) = group.remote.as_ref().map(|launch| launch.profile_name.clone()) else {
            return;
        };
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
            return;
        };
        let (Some(state), Confirmation::Rebuild(preview)) = (&runtime.state, &mut runtime.confirmation) else {
            return;
        };
        let (repository, bound) = (state.repository.clone(), state.profile.clone());
        let (tx, rx) = channel();
        **preview = Preview {
            reading: Some(rx),
            read: None,
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let cancel = cloud_runtime::Cancellation::default();
            let read =
                replacement::plan(&repository, &bound, &profile_name, &cancel).map_err(|error| error.to_string());
            let _ = tx.send(read);
            ctx.request_repaint();
        });
    }

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
        // A rebuild starts only with the changes its person confirmed.
        let confirmed = match &runtime.confirmation {
            Confirmation::Rebuild(preview) => preview.confirmed(),
            _ => None,
        };
        if kind == Kind::Rebuild && confirmed.is_none() {
            return;
        }
        let profile_name = launch.profile_name.clone();
        match request(&root, launch, repository) {
            Ok(request) => runtime.start_rebuild(request, kind, profile_name, confirmed, ctx),
            Err(error) => runtime.fail_preflight(Stage::Validate, error.to_string()),
        }
    }
}

fn request(
    root: &std::path::Path,
    launch: horizon_core::cloud_panel::CloudLaunch,
    repository: PathBuf,
) -> cloud_runtime::Result<Request> {
    let state_root = cloud_runtime::state::cloud_directory(root, &launch.id)?;
    let settings = Settings::for_cloud(&root.join("settings.json"), &launch.placement)?;
    Ok(Request::new(
        launch.id,
        repository,
        launch.revision,
        launch.profile,
        state_root,
        settings,
    ))
}

/// A rebuild operation as its person started it.
struct Operation<'a> {
    kind: Kind,
    profile_name: &'a str,
    /// For a rebuild, the profile the person confirmed.
    confirmed: Option<&'a Profile>,
}

fn run(
    request: &Request,
    operation: &Operation<'_>,
    cancel: &cloud_runtime::Cancellation,
    tx: &Sender<Event>,
    idle: Sender<super::idle::Report>,
    ctx: &egui::Context,
) {
    let ready = Cell::new(false);
    let emit = |event: Event| {
        ready.set(ready.get() || matches!(event, Event::Ready(..)));
        let _ = tx.send(event);
        ctx.request_repaint();
    };
    let result = match operation.kind {
        Kind::Rebuild => replacement::rebuild(request, operation.profile_name, operation.confirmed, cancel, &emit),
        Kind::Continue => replacement::continue_replacement(request, cancel, &emit),
        Kind::Cancel => replacement::cancel_replacement(request, cancel, &emit),
    };
    match result {
        Ok(state) if ready.get() => {
            super::idle::watch(&state, &request.settings, &request.state_root, cancel, idle, ctx);
            presentation::watch(&state, &request.settings, &request.state_root, cancel, tx, ctx);
        }
        // Dropping an unsent replacement leaves the worker as it was without
        // reconnecting; reconnect to restore the presentation stopped for it.
        Ok(_) => super::run_deployment(request, cancel, tx, idle, ctx),
        // Refused before anything was journaled, so the worker runs as recorded: reconnect
        // to restore the presentation stopped for the rebuild, and keep the reason. An
        // explicit cancel ends here instead; the card then offers Reconnect.
        Err(error) if !cancel.is_cancelled() && untouched(&request.state_root) => {
            emit(Event::Output(format!("{REFUSED} {error}")));
            super::run_deployment(request, cancel, tx, idle, ctx);
        }
        Err(error) => {
            super::report_failure(&request.state_root, &error, &emit);
        }
    }
}

/// The saved record is still Ready with nothing journaled or pending.
fn untouched(state_root: &std::path::Path) -> bool {
    Store::lock(state_root)
        .ok()
        .and_then(|store| store.load().ok().flatten())
        .is_some_and(|state| {
            state.stage == Stage::Ready && state.image_replacement.is_none() && state.session_restart.is_none()
        })
}
