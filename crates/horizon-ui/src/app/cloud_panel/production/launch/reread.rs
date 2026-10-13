//! Reads New cloud's settings again when the working copy of `.horizon/cloud.yml` or the
//! checked-out commit changes, so an edit shows without a button and keeps the choices
//! the edit leaves valid.
use super::{HorizonApp, cloud_runtime};
use egui::Context;
use horizon_core::cloud_panel::CloudConfig;
use horizon_core::cloud_runtime::repository::launch::{Fingerprint, fingerprint};
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, channel},
    },
    time::Duration,
};

/// How often the open dialog looks at the checkout.
const INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct State {
    watcher: Option<Watcher>,
    /// What the newest read saw, set by its thread before it read anything.
    pub read: Arc<Mutex<Option<Fingerprint>>>,
    /// What the shown settings were read from.
    applied: Option<Fingerprint>,
    /// What the watcher saw last.
    seen: Option<Fingerprint>,
    /// The read in flight was started by a change, not by the person: it shows no spinner
    /// and keeps the shown settings until it ends.
    pub quiet: bool,
    /// The settings as last read, before the person edited them in the dialog.
    as_read: Option<CloudConfig>,
}

impl State {
    /// Records the fingerprint of the read that just ended.
    pub fn finished(&mut self) {
        self.applied = self
            .read
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

struct Watcher {
    directory: String,
    receiver: Receiver<Fingerprint>,
    cancel: cloud_runtime::Cancellation,
}

impl Watcher {
    fn start(ctx: &Context, directory: &str) -> Self {
        let cancel = cloud_runtime::Cancellation::default();
        let (sender, receiver) = channel();
        let (watched, stop, ctx) = (directory.to_owned(), cancel.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut last = None;
            while !stop.is_cancelled() {
                let current = fingerprint(&watched, &stop);
                if last.as_ref() != Some(&current) {
                    if sender.send(current.clone()).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                    last = Some(current);
                }
                std::thread::sleep(INTERVAL);
            }
        });
        Self {
            directory: directory.to_owned(),
            receiver,
            cancel,
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl HorizonApp {
    /// Starts a quiet read once the checkout differs from what the shown settings were read
    /// from, unless the person is starting the cloud or a setup agent works on the repository.
    pub(super) fn reread_cloud_profiles_on_change(&mut self, ctx: &Context) {
        let form = &mut self.cloud_prototype.production;
        let reread = &mut form.launch.reread;
        if !form.creating || form.repository.trim().is_empty() {
            reread.watcher = None;
            return;
        }
        if reread
            .watcher
            .as_ref()
            .is_none_or(|watcher| watcher.directory != form.repository)
        {
            reread.watcher = Some(Watcher::start(ctx, &form.repository));
        }
        if let Some(seen) = reread
            .watcher
            .as_ref()
            .and_then(|watcher| watcher.receiver.try_iter().last())
        {
            reread.seen = Some(seen);
        }
        let changed = reread.applied.is_some() && reread.seen.is_some() && reread.seen != reread.applied;
        if changed && !form.launch.loading_any() && !busy(form) {
            self.start_cloud_profiles_read(ctx, true);
        }
    }
}

/// Whether a read must not change the settings now: the person started the cloud or a stock
/// watch with what is shown, or a setup agent is writing the repository's settings.
pub(super) fn busy(form: &super::super::Production) -> bool {
    form.launch.submitted
        || form.pending_creation.is_some()
        || form.launch.watch.is_some()
        || form.setup_starting.is_some()
        || super::super::repository_setup::running_here(form)
}

/// Keeps a storage size the person changed in the dialog while the file's own value for it
/// stays what it was, and remembers `config` as read.
pub(super) fn keep_edits(state: &mut State, shown: Option<&CloudConfig>, config: &mut CloudConfig) {
    let read = std::mem::replace(&mut state.as_read, Some(config.clone()));
    let (Some(shown), Some(read)) = (shown, read) else {
        return;
    };
    for (name, profile) in &mut config.profiles {
        if let (Some(shown), Some(read)) = (shown.profiles.get(name), read.profiles.get(name))
            && shown.storage != read.storage
            && profile.storage == read.storage
        {
            profile.storage = shown.storage.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_storage_edit_survives_a_reread_until_the_file_changes_that_value() {
        let yaml = |volume: u16| {
            CloudConfig::parse(&format!(
                "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    storage:\n      container_gb: 20\n      volume_gb: {volume}\n"
            ))
            .unwrap()
        };
        let mut state = State::default();
        let mut first = yaml(20);
        keep_edits(&mut state, None, &mut first);
        let mut shown = first.clone();
        shown.profiles.get_mut("dev").unwrap().storage.container_gb = 50;
        let mut unchanged = yaml(20);
        keep_edits(&mut state, Some(&shown), &mut unchanged);
        assert_eq!(unchanged.profiles["dev"].storage.container_gb, 50, "the edit stays");
        let mut resized = yaml(40);
        keep_edits(&mut state, Some(&unchanged), &mut resized);
        assert_eq!(
            resized.profiles["dev"].storage.volume_gb, 40,
            "the file's new storage wins"
        );
        assert_eq!(resized.profiles["dev"].storage.container_gb, 20);
    }
}
