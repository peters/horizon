//! Background destination reads and local writes for one reviewed creation scope.
use horizon_core::cloud_runtime::{
    self as runtime, Cancellation, Event,
    github::Prompt,
    publishing::{Selection, management},
};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, Instant},
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) struct Key {
    pub root: PathBuf,
    pub repository: PathBuf,
    pub raw_repository: String,
    pub profile_name: String,
    pub profile: horizon_core::cloud_panel::Profile,
}

impl Key {
    fn same_destination_inputs(&self, other: &Self) -> bool {
        if self.root != other.root
            || self.repository != other.repository
            || self.raw_repository != other.raw_repository
            || self.profile_name != other.profile_name
        {
            return false;
        }
        let mut profile = self.profile.clone();
        profile.storage.clone_from(&other.profile.storage);
        profile == other.profile
    }
}

pub(super) use horizon_core::cloud_runtime::publishing::management::Info;

enum Report {
    Loaded(Result<Info, String>),
    Prompt(Prompt),
    Finished(Result<(), String>),
}

#[derive(Default)]
pub(in crate::app::cloud_panel::production) struct State {
    pub(super) key: Option<Key>,
    receiver: Option<Receiver<Report>>,
    refresh_at: Option<Instant>,
    cancel: Cancellation,
    pub(super) info: Option<Info>,
    pub error: Option<String>,
    pub(super) notice: Option<String>,
    pub prompt: Option<Prompt>,
    pub changing: bool,
    signing_in: Option<String>,
    awaiting_choice: bool,
    setup_target: Option<String>,
    opener: Option<Receiver<std::io::Result<()>>>,
    pub(super) open_error: Option<String>,
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl State {
    pub fn sync(&mut self, key: Option<Key>, ctx: &egui::Context) {
        if self.key != key {
            if self
                .key
                .as_ref()
                .zip(key.as_ref())
                .is_some_and(|(previous, current)| previous.same_destination_inputs(current))
            {
                // Storage does not change a destination read or sign-in. Confirmation still
                // retains the complete updated profile, so an earlier queued Start is stale.
                self.key = key;
            } else {
                self.cancel.cancel();
                *self = Self::default();
                self.key = key;
                self.reload(ctx);
            }
        }
        self.poll(ctx);
    }

    pub fn matches(
        &self,
        root: &std::path::Path,
        repository: &str,
        name: &str,
        profile: &horizon_core::cloud_panel::Profile,
    ) -> bool {
        self.key.as_ref().is_some_and(|key| key.root == root) && self.matches_profile(repository, name, profile)
    }

    pub fn matches_profile(&self, repository: &str, name: &str, profile: &horizon_core::cloud_panel::Profile) -> bool {
        self.key
            .as_ref()
            .is_some_and(|key| key.raw_repository == repository && key.profile_name == name && key.profile == *profile)
    }

    pub fn refreshing(&self) -> bool {
        self.busy() || self.metadata_expired() || (self.info.is_none() && self.error.is_none())
    }

    pub fn matches_confirmation(&self, confirmation: &super::Confirmation) -> bool {
        self.key.as_ref() == Some(&confirmation.scope)
            && self.info.as_ref().and_then(|info| info.selection.as_ref()) == Some(&confirmation.selection)
    }

    pub fn snapshot(&self) -> Option<super::Confirmation> {
        self.ready().then_some(())?;
        self.reviewed_snapshot()
    }

    pub fn reviewed_snapshot(&self) -> Option<super::Confirmation> {
        self.reviewable().then_some(())?;
        Some(super::Confirmation {
            scope: self.key.clone()?,
            selection: self.info.as_ref()?.selection.clone()?,
        })
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        self.poll_opener();
        if !self.busy() && self.metadata_expired() {
            self.reload(ctx);
            return;
        }
        for _ in 0..16 {
            let report = self.receiver.as_ref().map(Receiver::try_recv);
            match report {
                Some(Ok(Report::Loaded(result))) => {
                    self.receiver = None;
                    match result {
                        Ok(info) => {
                            self.awaiting_choice = false;
                            self.error.clone_from(&info.selection_error);
                            self.info = Some(info);
                            self.refresh_at = Some(Instant::now() + REFRESH_INTERVAL);
                            ctx.request_repaint_after(REFRESH_INTERVAL);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Some(Ok(Report::Prompt(prompt))) => {
                    self.prompt = match prompt {
                        Prompt::Published { .. } => None,
                        other => Some(other),
                    };
                }
                Some(Ok(Report::Finished(result))) => {
                    self.receiver = None;
                    self.prompt = None;
                    self.signing_in = None;
                    match result {
                        Ok(()) => self.reload(ctx),
                        Err(error) => self.error = Some(error),
                    }
                }
                Some(Err(TryRecvError::Disconnected)) => {
                    self.receiver = None;
                    self.prompt = None;
                    self.signing_in = None;
                    self.error =
                        Some("Image destination preparation ended without a result. Reload to try again.".into());
                }
                _ => return,
            }
        }
        if self.receiver.is_some() {
            ctx.request_repaint();
        }
    }

    pub fn active(&self) -> bool {
        self.key.is_some()
    }

    pub fn acknowledge(&mut self) {
        self.notice = None;
    }

    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn selected(&self) -> Option<&Selection> {
        self.info.as_ref().and_then(|info| info.selection.as_ref())
    }

    pub fn opening_url(&self) -> bool {
        self.opener.is_some()
    }

    pub fn open_sign_in_url(&mut self, url: String, ctx: &egui::Context) {
        self.open_with(url, ctx, horizon_core::open_url);
    }

    fn open_with(
        &mut self,
        url: String,
        ctx: &egui::Context,
        open: impl FnOnce(&str) -> std::io::Result<()> + Send + 'static,
    ) {
        if self.opening_url() {
            return;
        }
        let (tx, rx) = channel();
        self.opener = Some(rx);
        self.open_error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open(&url));
            ctx.request_repaint();
        });
    }

    fn poll_opener(&mut self) {
        match self.opener.as_ref().map(Receiver::try_recv) {
            Some(Ok(result)) => {
                self.opener = None;
                self.open_error = result
                    .err()
                    .map(|error| format!("Could not open the sign-in page: {error}"));
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.opener = None;
                self.open_error = Some("The sign-in page could not be opened. Copy the code and try again.".into());
            }
            _ => {}
        }
    }

    pub fn request_setup(&mut self, target: String) {
        self.setup_target = Some(target);
    }
    pub fn take_setup(&mut self) -> Option<String> {
        self.setup_target.take()
    }

    pub fn ready(&self) -> bool {
        !self.busy() && !self.metadata_expired() && self.reviewable()
    }

    pub fn reviewable(&self) -> bool {
        if self.error.is_some() || self.awaiting_choice || self.signing_in.is_some() || self.prompt.is_some() {
            return false;
        }
        self.info
            .as_ref()
            .filter(|info| info.selection_error.is_none())
            .is_some_and(|info| match info.selection.as_ref() {
                None | Some(Selection::Automatic { .. }) => false,
                Some(Selection::Personal { .. }) => info.enabled && info.pull_saved,
                Some(Selection::Registry { .. }) => info.enabled,
                Some(Selection::Recipe { .. }) => true,
            })
    }

    fn metadata_expired(&self) -> bool {
        self.refresh_at.is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn reset_operation(&mut self) {
        self.cancel.cancel();
        self.cancel = Cancellation::default();
        self.error = None;
        self.prompt = None;
        self.signing_in = None;
        self.refresh_at = None;
        self.opener = None;
        self.open_error = None;
    }

    pub fn reload(&mut self, ctx: &egui::Context) {
        let Some(key) = self.key.clone() else {
            return;
        };
        self.reset_operation();
        let (tx, rx) = channel();
        self.receiver = Some(rx);
        let ctx = ctx.clone();
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            let result = load(&key, &cancel).map_err(|error| error.to_string());
            let _ = tx.send(Report::Loaded(result));
            ctx.request_repaint();
        });
    }

    pub fn save(&mut self, destination: Option<Selection>, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (Some(key), Some(info)) = (self.key.clone(), self.info.as_ref()) else {
            return;
        };
        let shown = info.shown_override.clone();
        self.acknowledge();
        self.reset_operation();
        self.awaiting_choice = true;
        let cancel = self.cancel.clone();
        let (tx, rx) = channel();
        self.receiver = Some(rx);
        self.changing = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let action = destination.map_or(management::Action::Reset { shown: shown.clone() }, |destination| {
                management::Action::Select { shown, destination }
            });
            let runner = runtime::command::Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            let result = management::manage_prepared(
                &key.root,
                &management::Request {
                    repository: key.repository,
                    profile: key.profile_name,
                    action,
                },
                &key.profile,
                &runtime::new_id(),
                &runner,
            )
            .map(drop)
            .map_err(|error| error.to_string());
            let _ = tx.send(Report::Finished(result));
            ctx.request_repaint();
        });
    }

    pub fn authenticate(&mut self, switch: bool, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (Some(key), Some(info)) = (self.key.clone(), self.info.as_ref()) else {
            return;
        };
        let shown = info.shown_override.clone();
        self.acknowledge();
        self.reset_operation();
        let cancel = self.cancel.clone();
        let id = runtime::new_id();
        self.awaiting_choice = true;
        self.signing_in = Some(id.clone());
        self.error = None;
        let (tx, rx) = channel();
        self.receiver = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let send_ctx = ctx.clone();
            let emit = |event| {
                if let Event::GitHub(prompt) = event {
                    let _ = tx.send(Report::Prompt(prompt));
                    send_ctx.request_repaint();
                }
            };
            let runner = runtime::command::Runner {
                cancel: &cancel,
                emit: &emit,
                secrets: Vec::new(),
            };
            let result = management::manage_prepared(
                &key.root,
                &management::Request {
                    repository: key.repository,
                    profile: key.profile_name,
                    action: management::Action::Authenticate { shown, switch },
                },
                &key.profile,
                &id,
                &runner,
            )
            .map(drop)
            .map_err(|error| error.to_string());
            let _ = tx.send(Report::Finished(result));
            ctx.request_repaint();
        });
    }

    pub fn skip_sign_in(&self) {
        if self.signing_in.is_some() {
            self.cancel.cancel();
        }
    }
}

fn load(key: &Key, cancel: &Cancellation) -> runtime::Result<Info> {
    management::inspect(&key.root, &key.repository, &key.profile_name, &key.profile, cancel)
}

#[cfg(test)]
mod tests;
