//! Cached companion presentation and cancellable background refreshes.
mod job;
#[cfg(test)]
mod tests;
mod view;

use super::HorizonApp;
use horizon_core::{
    cloud_panel::CloudGroups,
    cloud_runtime::{
        Cancellation,
        companions::{Action, Owner, Scope, Snapshot},
    },
};
use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

const REFRESH: Duration = Duration::from_secs(15);
const CONCURRENT_JOBS: usize = 4;

#[derive(Default)]
pub(super) struct State {
    session: Option<String>,
    entries: HashMap<String, Entry>,
    inventory: Vec<CloudIdentity>,
    retiring: Vec<Entry>,
}

#[derive(PartialEq, Eq)]
struct CloudIdentity {
    id: String,
    workspace: String,
    repository: std::path::PathBuf,
    revision: String,
    profile: String,
}

struct Entry {
    owner: Owner,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    job: Option<Job>,
    pending: Option<Action>,
    selecting: BTreeSet<String>,
    clearing: BTreeSet<String>,
    due: Instant,
    choices: HashMap<String, String>,
    blocked: bool,
}

struct Job {
    receiver: Receiver<job::Outcome>,
    cancel: Cancellation,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Entry {
    fn new(owner: Owner) -> Self {
        Self {
            owner,
            snapshot: None,
            error: None,
            job: None,
            pending: None,
            selecting: BTreeSet::new(),
            clearing: BTreeSet::new(),
            due: Instant::now(),
            choices: HashMap::new(),
            blocked: false,
        }
    }

    fn queue(&mut self, action: Action) {
        if let Action::Clear { alias } = action {
            self.selecting.remove(&alias);
            self.clearing.insert(alias);
            self.cancel_job();
            self.pending = None;
        } else {
            if let Action::Select { alias, .. } = &action {
                self.selecting.insert(alias.clone());
            }
            self.pending = Some(action);
        }
        self.due = Instant::now();
    }

    fn poll(&mut self) {
        let Some(job) = &self.job else { return };
        let cancelled = job.cancel.check().is_err();
        let outcome = match job.receiver.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => job::Outcome {
                snapshot: None,
                error: Some("Companion refresh ended without a result".into()),
            },
        };
        self.job = None;
        if cancelled {
            self.due = Instant::now();
            return;
        }
        if let Some(snapshot) = &outcome.snapshot {
            // Keep uncertain selections cancellable until an authoritative result arrives.
            self.selecting.clear();
            self.clearing.retain(|alias| {
                snapshot
                    .rows
                    .iter()
                    .any(|row| row.companion.alias == *alias && row.companion.selected)
            });
        }
        // On failure keep selections available for explicit clearing, but never advertise cached readiness.
        self.snapshot = if outcome.error.is_some() {
            outcome.snapshot.or_else(|| self.snapshot.take())
        } else {
            outcome.snapshot
        };
        self.error = outcome.error;
        self.due = Instant::now()
            + if self.clearing.is_empty() {
                REFRESH
            } else {
                Duration::from_secs(1)
            };
    }

    fn cancel_job(&self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
    }

    fn retire(&mut self) -> Option<Self> {
        let mut retired = std::mem::replace(self, Self::new(self.owner.clone()));
        retired.cancel_job();
        retired.pending = None;
        retired.selecting.clear();
        (retired.job.is_some() || !retired.clearing.is_empty()).then_some(retired)
    }
}

impl State {
    pub(super) fn set_session(&mut self, session: Option<&str>) -> bool {
        let changed = self.session.as_deref() != session;
        if changed {
            self.retiring
                .extend(self.entries.drain().filter_map(|(_, mut entry)| entry.retire()));
            self.inventory.clear();
            self.session = session.map(str::to_owned);
        }
        changed
    }

    fn sync(&mut self, session: Option<&str>, groups: &CloudGroups) {
        let session_changed = self.set_session(session);
        let Some(session) = session else { return };
        let changed = self.inventory.len() != groups.0.iter().filter(|group| group.remote.is_some()).count()
            || self
                .inventory
                .iter()
                .zip(groups.0.iter().filter(|group| group.remote.is_some()))
                .any(|(old, group)| {
                    group.remote.as_ref().is_none_or(|launch| {
                        old.id != launch.id
                            || old.workspace != group.workspace
                            || old.repository != group.cwd
                            || old.revision != launch.revision
                            || old.profile != launch.profile_name
                    })
                });
        if !changed && !session_changed {
            return;
        }
        if changed {
            self.inventory = groups
                .0
                .iter()
                .filter_map(|group| {
                    group.remote.as_ref().map(|launch| CloudIdentity {
                        id: launch.id.clone(),
                        workspace: group.workspace.clone(),
                        repository: group.cwd.clone(),
                        revision: launch.revision.clone(),
                        profile: launch.profile_name.clone(),
                    })
                })
                .collect();
            for entry in self.entries.values_mut() {
                entry.cancel_job();
                entry.pending = None;
                entry.error = Some("Cloud inventory changed; refreshing companion access".into());
                entry.due = Instant::now();
            }
        }
        self.entries.retain(|id, entry| {
            let keep = groups
                .0
                .iter()
                .any(|g| g.remote.as_ref().is_some_and(|launch| launch.id == *id));
            if !keep {
                self.retiring.extend(entry.retire());
            }
            keep
        });
        for group in &groups.0 {
            let Some(launch) = &group.remote else { continue };
            let owner = Owner {
                scope: Scope {
                    session_id: session.into(),
                    workspace_id: group.workspace.clone(),
                },
                cloud_id: launch.id.clone(),
            };
            let entry = self
                .entries
                .entry(launch.id.clone())
                .or_insert_with(|| Entry::new(owner.clone()));
            if entry.owner != owner {
                self.retiring.extend(entry.retire());
                *entry = Entry::new(owner);
            }
            entry.blocked = groups
                .0
                .iter()
                .filter(|group| group.remote.as_ref().is_some_and(|other| other.id == launch.id))
                .count()
                != 1;
            if entry.blocked {
                entry.cancel_job();
                entry.snapshot = None;
                entry.error = Some("Cloud identity is duplicated; companion access is unavailable".into());
            }
        }
    }

    fn tick(&mut self, root: &Path, groups: &CloudGroups, ctx: &egui::Context) {
        for entry in self.entries.values_mut() {
            entry.poll();
        }
        if self.finish_retired(root, ctx) {
            return;
        }
        let active = self.entries.values().filter(|entry| entry.job.is_some()).count();
        // Explicit checkbox actions precede periodic probes.
        let mut ready = self
            .entries
            .iter()
            .filter(|(_, e)| !e.blocked && e.job.is_none() && Instant::now() >= e.due)
            .map(|(id, e)| (e.clearing.is_empty(), e.pending.is_none(), e.due, id.clone()))
            .collect::<Vec<_>>();
        ready.sort();
        for (_, _, _, id) in ready.into_iter().take(CONCURRENT_JOBS.saturating_sub(active)) {
            let Some(entry) = self.entries.get_mut(&id) else {
                continue;
            };
            let action = entry.clearing.first().map_or_else(
                || entry.pending.take().unwrap_or_default(),
                |alias| Action::Clear { alias: alias.clone() },
            );
            entry.job = Some(job::start(
                root.to_owned(),
                entry.owner.clone(),
                groups.clone(),
                action,
                ctx.clone(),
            ));
        }
        if !self.entries.is_empty() {
            ctx.request_repaint_after(if self.entries.values().any(|entry| entry.job.is_some()) {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(1)
            });
        }
    }

    fn finish_retired(&mut self, root: &Path, ctx: &egui::Context) -> bool {
        for entry in &mut self.retiring {
            entry.poll();
        }
        self.retiring
            .retain(|entry| entry.job.is_some() || !entry.clearing.is_empty());
        let mut active = self
            .entries
            .values()
            .chain(&self.retiring)
            .filter(|entry| entry.job.is_some())
            .count();
        for entry in &mut self.retiring {
            if active >= CONCURRENT_JOBS {
                break;
            }
            if entry.job.is_some() || Instant::now() < entry.due {
                continue;
            }
            if let Some(alias) = entry.clearing.first() {
                entry.job = Some(job::start(
                    root.to_owned(),
                    entry.owner.clone(),
                    CloudGroups::default(),
                    Action::Clear { alias: alias.clone() },
                    ctx.clone(),
                ));
                active += 1;
            }
        }
        // Finish explicit revocations before another owner can acquire the same journals.
        if !self.retiring.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        !self.retiring.is_empty()
    }

    pub(super) fn render(&mut self, ui: &mut egui::Ui, cloud: &str) {
        if !self.retiring.is_empty() {
            ui.small("Removing previous companion access…");
            if let Some(error) = self.retiring.iter().find_map(|entry| entry.error.as_deref()) {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
        }
        let Some(entry) = self.entries.get_mut(cloud) else {
            ui.small("Open a saved session to select companion clouds.");
            return;
        };
        view::render(ui, entry);
    }
}

impl HorizonApp {
    pub(in crate::app) fn sync_cloud_companion_session(&mut self, ctx: &egui::Context) {
        let state = &mut self.cloud_prototype.production.companions;
        state.set_session(persistent_session(self.active_session.as_ref()));
        if let Some(root) = &self.cloud_prototype.root {
            state.finish_retired(root, ctx);
        }
    }

    pub(super) fn prepare_cloud_companions(&mut self, ctx: &egui::Context) {
        let state = &mut self.cloud_prototype.production.companions;
        state.sync(
            persistent_session(self.active_session.as_ref()),
            &self.cloud_prototype.groups,
        );
        if let Some(root) = &self.cloud_prototype.root {
            state.tick(root, &self.cloud_prototype.groups, ctx);
        }
    }
}

fn persistent_session(session: Option<&crate::app::ActiveSession>) -> Option<&str> {
    session
        .filter(|session| session.persistent)
        .map(|session| session.session_id.as_str())
}
