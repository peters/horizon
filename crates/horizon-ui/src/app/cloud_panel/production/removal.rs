//! Background record checks and worker-owned custody through a UI removal.
use std::{collections::BTreeMap, path::PathBuf, sync::mpsc};

use horizon_core::{cloud_panel::CloudLaunch, cloud_runtime};

use cloud_runtime::{Cancellation, state::Store};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    root: Option<PathBuf>,
    cloud: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Owner {
    pub root: Option<PathBuf>,
    pub session: Option<String>,
    pub issue: u32,
    pub workspace: String,
    pub launch: CloudLaunch,
    pub generation: u64,
}

impl Owner {
    fn key(&self) -> Key {
        Key {
            root: self.root.clone(),
            cloud: self.launch.id.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Purpose {
    Display,
    Remove,
    RemoveAnyway,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    owner: Owner,
    purpose: Purpose,
}

#[derive(Debug)]
pub(super) enum Snapshot {
    Empty,
    Holds,
    Busy,
    Unreadable(String),
}

#[derive(Debug)]
pub(super) enum Refusal {
    Holds,
    Busy,
    Unreadable(String),
}

/// Dropping this sender releases the worker, never a Store on the UI thread.
pub(super) struct Custody {
    _release: mpsc::Sender<()>,
}

pub(super) enum Outcome {
    Display(Snapshot),
    Removal(Result<Custody, Refusal>),
}

pub(super) struct Completion {
    pub owner: Owner,
    pub purpose: Purpose,
    pub outcome: Outcome,
}

struct Job {
    request: Request,
    receiver: mpsc::Receiver<Outcome>,
    cancelled: Cancellation,
    reported: bool,
}

/// A cancelled OS read stays tracked: reopening cannot spawn another for its record.
#[derive(Default)]
pub(super) struct Jobs {
    running: BTreeMap<Key, Job>,
    wanted: BTreeMap<Key, Request>,
}

impl Jobs {
    pub fn request(&mut self, owner: Owner, purpose: Purpose, ctx: &egui::Context) {
        let key = owner.key();
        let request = Request { owner, purpose };
        if let Some(job) = self.running.get(&key)
            && job.request != request
        {
            job.cancelled.cancel();
        }
        self.wanted.insert(key, request);
        self.start_waiting(ctx);
    }

    pub fn cancel(&mut self, issue: u32) {
        self.wanted.retain(|_, request| request.owner.issue != issue);
        for job in self.running.values().filter(|job| job.request.owner.issue == issue) {
            job.cancelled.cancel();
        }
    }

    pub fn reset(&mut self) {
        self.wanted.clear();
        for job in self.running.values() {
            job.cancelled.cancel();
        }
    }

    pub fn pending(&self, issue: u32) -> bool {
        self.wanted.values().any(|request| request.owner.issue == issue)
            || self
                .running
                .values()
                .any(|job| job.request.owner.issue == issue && !job.cancelled.is_cancelled())
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<Completion> {
        let mut completed = Vec::new();
        self.running.retain(|key, job| {
            loop {
                match job.receiver.try_recv() {
                    Ok(outcome) => {
                        job.reported = true;
                        if self.wanted.get(key) == Some(&job.request) && !job.cancelled.is_cancelled() {
                            self.wanted.remove(key);
                            completed.push(Completion {
                                owner: job.request.owner.clone(),
                                purpose: job.request.purpose,
                                outcome,
                            });
                        }
                        // A stale custody value drops here, releasing its original worker.
                    }
                    Err(mpsc::TryRecvError::Empty) => return true,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if !job.reported && self.wanted.get(key) == Some(&job.request) {
                            self.wanted.remove(key);
                            let message = "Cloud record check was interrupted".into();
                            let outcome = if job.request.purpose == Purpose::Display {
                                Outcome::Display(Snapshot::Unreadable(message))
                            } else {
                                Outcome::Removal(Err(Refusal::Unreadable(message)))
                            };
                            completed.push(Completion {
                                owner: job.request.owner.clone(),
                                purpose: job.request.purpose,
                                outcome,
                            });
                        }
                        return false;
                    }
                }
            }
        });
        self.start_waiting(ctx);
        if !self.running.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        completed
    }

    #[cfg(test)]
    pub(super) fn idle(&self) -> bool {
        self.running.is_empty() && self.wanted.is_empty()
    }

    fn start_waiting(&mut self, ctx: &egui::Context) {
        for (key, request) in &self.wanted {
            if self.running.contains_key(key) {
                continue;
            }
            let request = request.clone();
            let cancelled = Cancellation::default();
            let worker_cancel = cancelled.clone();
            let (sender, receiver) = mpsc::channel();
            let worker_request = request.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || run(&worker_request, &worker_cancel, &sender, &ctx));
            self.running.insert(
                key.clone(),
                Job {
                    request,
                    receiver,
                    cancelled,
                    reported: false,
                },
            );
        }
    }
}

fn run(request: &Request, cancelled: &Cancellation, sender: &mpsc::Sender<Outcome>, ctx: &egui::Context) {
    if request.purpose == Purpose::Display {
        let snapshot = snapshot(&request.owner);
        if !cancelled.is_cancelled() {
            let _ = sender.send(Outcome::Display(snapshot));
        }
    } else {
        let result = removal_store(&request.owner, request.purpose);
        if !cancelled.is_cancelled() {
            match result {
                Ok(store) => {
                    let (release, released) = mpsc::channel();
                    if sender.send(Outcome::Removal(Ok(Custody { _release: release }))).is_ok() {
                        ctx.request_repaint();
                        // No deadline can unlock a decision that the UI is about to apply.
                        // Cancellation drops queued custody on poll; app drop closes the channel.
                        let _ = released.recv();
                    }
                    drop(store);
                }
                Err(error) => {
                    let _ = sender.send(Outcome::Removal(Err(error)));
                }
            }
        }
    }
    ctx.request_repaint();
}

fn snapshot(owner: &Owner) -> Snapshot {
    let Some(root) = &owner.root else {
        return if owner.launch.deployment_started {
            Snapshot::Unreadable("No saved deployment record".into())
        } else {
            Snapshot::Empty
        };
    };
    if !recorded(root, &owner.launch) {
        return if owner.launch.deployment_started {
            Snapshot::Unreadable("No saved deployment record".into())
        } else {
            Snapshot::Empty
        };
    }
    match removable(root, &owner.launch) {
        Ok(Checked { present: false, .. }) if owner.launch.deployment_started => {
            Snapshot::Unreadable("No saved deployment record".into())
        }
        Ok(Checked { allowed: true, .. }) => Snapshot::Empty,
        Ok(_) => Snapshot::Holds,
        Err(cloud_runtime::Error::Busy) => Snapshot::Busy,
        Err(error) => Snapshot::Unreadable(error.to_string()),
    }
}

fn removal_store(owner: &Owner, purpose: Purpose) -> Result<Option<Store>, Refusal> {
    let Some(root) = &owner.root else {
        return if !owner.launch.deployment_started || purpose == Purpose::RemoveAnyway {
            Ok(None)
        } else {
            Err(Refusal::Holds)
        };
    };
    if purpose == Purpose::RemoveAnyway {
        if !Store::supported() {
            return Ok(None);
        }
        return match cloud_runtime::state::cloud_directory(root, &owner.launch.id) {
            Ok(path) => Store::lock(&path).map(Some).map_err(refusal),
            // An invalid identity cannot name a record; only the local card is discarded.
            Err(_) => Ok(None),
        };
    }
    if !Store::supported() && !owner.launch.deployment_started && !recorded(root, &owner.launch) {
        return Ok(None);
    }
    match removable(root, &owner.launch).map_err(refusal)? {
        Checked {
            store, allowed: true, ..
        } => Ok(Some(store)),
        Checked { allowed: false, .. } => Err(Refusal::Holds),
    }
}

fn refusal(error: cloud_runtime::Error) -> Refusal {
    match error {
        cloud_runtime::Error::Busy => Refusal::Busy,
        error => Refusal::Unreadable(error.to_string()),
    }
}

fn recorded(root: &std::path::Path, launch: &CloudLaunch) -> bool {
    cloud_runtime::state::cloud_directory(root, &launch.id).map_or(true, |path| path.try_exists().unwrap_or(true))
}

struct Checked {
    store: Store,
    allowed: bool,
    present: bool,
}

fn removable(root: &std::path::Path, launch: &CloudLaunch) -> cloud_runtime::Result<Checked> {
    let store = cloud_runtime::state::cloud_directory(root, &launch.id).and_then(|path| Store::lock(&path))?;
    let state = store.load()?;
    let present = state.is_some();
    let allowed = match state {
        Some(state) => cloud_runtime::lifecycle::can_remove(&store, &state)?,
        None => !launch.deployment_started && cloud_runtime::lifecycle::can_remove_without_deployment(&store)?,
    };
    Ok(Checked {
        store,
        allowed,
        present,
    })
}

#[cfg(test)]
mod tests;
