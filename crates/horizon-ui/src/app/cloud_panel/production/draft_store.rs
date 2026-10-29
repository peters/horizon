//! The parked drafts' choices as the frames use them: one map in memory, read from
//! `drafts.json` on a worker thread once for each session, and saved on a worker thread. A
//! frame opens, writes and renames no file, so a slow disk cannot stall the window.
use egui::Context;
use horizon_core::cloud_runtime::{
    Result,
    draft::{Drafts, Kept},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
};

/// Where a frame says that a save of the drafts failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    /// The card of the draft that was parked stays in this session only, and the board says so.
    Board,
    /// Only a later start would miss the record, so the failure goes to the log.
    Log,
}

/// A map of drafts that the writer thread writes under `root`.
struct Job {
    root: PathBuf,
    drafts: Drafts,
    failure: Failure,
}

/// What the writer thread did with a map that a frame handed it.
struct Written {
    drafts: Drafts,
    failure: Failure,
    /// Why the map is not on the file, which the frame reports where `failure` says.
    error: Option<String>,
}

/// The thread that writes the maps a frame keeps, and the answers a frame collects.
struct Writer {
    jobs: Sender<Job>,
    answers: Receiver<Written>,
}

/// Which session's saved drafts this window read, so that another session reads the file again
/// and puts its own card back.
#[derive(Default, PartialEq, Eq)]
enum Read {
    /// This window read nothing yet.
    #[default]
    Never,
    /// A worker thread reads the records of this session; `None` for a window without one.
    Reading(Option<String>),
    /// The records of this session arrived; the card they hold waits for a frame with its board.
    Waiting(Option<String>),
    /// This session's card is placed, or nothing of this session was kept.
    Shown(Option<String>),
}

/// The parked drafts' choices: the map in memory, the read that fills it once for each session,
/// and the writer thread that saves it.
#[derive(Default)]
pub(super) struct Store {
    /// The drafts in memory: what a card shows and what the writer thread saves.
    kept: Drafts,
    /// The records this window changed and no save confirmed, so that a read of a file that may
    /// lag them neither drops one nor brings a forgotten one back.
    changed: BTreeMap<String, Option<Kept>>,
    read: Read,
    loading: Option<Receiver<Drafts>>,
    writer: Option<Writer>,
    /// Whether a write of a map that is not the one in memory still runs.
    writing: bool,
    /// The context the worker threads wake, once a frame gave it.
    wake: Option<Context>,
    /// A test's grip on the writer thread, which holds each write until the test lets it go
    /// ([`Store::hold_saves`]).
    gate: Option<Receiver<()>>,
}

impl Store {
    /// The drafts in memory, which a card and a frame's decisions read without touching the file.
    pub(super) fn kept(&self) -> &Drafts {
        &self.kept
    }

    /// Gives the worker threads the context that wakes this window when they finish: a read takes
    /// it as it is asked, and the writer thread takes it when it starts. A frame calls this before
    /// it asks for work, so that an answer never waits for the next input.
    pub(super) fn wake_with(&mut self, ctx: &Context) {
        if self.wake.is_none() {
            self.wake = Some(ctx.clone());
        }
    }

    /// Reads `drafts.json` on a worker thread once for `session`. The frame that collects the
    /// records merges them into the map in memory, and puts the card of this session's record.
    pub(super) fn read(&mut self, root: &Path, session: Option<&str>) {
        let session = session.map(str::to_owned);
        let asked = match &self.read {
            Read::Never => false,
            Read::Reading(read) | Read::Waiting(read) | Read::Shown(read) => read == &session,
        };
        if asked {
            return;
        }
        let (sender, loading) = mpsc::channel();
        let root = root.to_path_buf();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            // A missing or damaged file keeps nothing, as a start that saved no draft does.
            let _ = sender.send(Drafts::load(&root));
            if let Some(ctx) = wake {
                ctx.request_repaint();
            }
        });
        self.read = Read::Reading(session);
        self.loading = Some(loading);
    }

    /// Collects what the worker threads finished, without waiting. Reports why a save failed:
    /// the frame says so wherever the record the worker wrote belongs.
    pub(super) fn poll(&mut self) -> Option<(Failure, String)> {
        let read = self.loading.as_ref().and_then(|loading| match loading.try_recv() {
            Ok(drafts) => Some(Some(drafts)),
            // A thread that stopped before it answered says nothing about this session's
            // records, which then are the ones in memory.
            Err(TryRecvError::Disconnected) => Some(None),
            Err(TryRecvError::Empty) => None,
        });
        if let Some(drafts) = read {
            self.loading = None;
            if let Some(drafts) = drafts {
                self.merge(drafts);
            }
            self.read = match std::mem::take(&mut self.read) {
                Read::Reading(session) => Read::Waiting(session),
                other => other,
            };
        }
        let written: Vec<Written> = self
            .writer
            .as_ref()
            .map(|writer| writer.answers.try_iter().collect())
            .unwrap_or_default();
        written.into_iter().find_map(|written| self.applied(written))
    }

    /// Whether a save of the map in memory still runs on the writer thread.
    pub(super) fn saving(&self) -> bool {
        self.writing
    }

    /// Whether this session's records arrived and the card they hold is not placed yet.
    pub(super) fn restore_pending(&self) -> bool {
        matches!(self.read, Read::Waiting(_))
    }

    /// Whether the records of this session are being read, or the card they hold is not placed
    /// yet. A frame keeps the empty workspaces that those records hold a card for until then.
    pub(super) fn unsettled(&self) -> bool {
        matches!(self.read, Read::Reading(_) | Read::Waiting(_))
    }

    /// Marks this session's read as placed: the form comes back from the records once for the
    /// session they belong to, not again on every frame that ends a card.
    pub(super) fn restored(&mut self) {
        if let Read::Waiting(session) = std::mem::take(&mut self.read) {
            self.read = Read::Shown(session);
        }
    }

    /// Whether the records of `session` are read and no card of them waits for a frame with its
    /// board.
    #[cfg(test)]
    pub(super) fn settled(&self, session: Option<&str>) -> bool {
        matches!(&self.read, Read::Shown(read) if read.as_deref() == session)
    }

    /// Keeps `kept` for `workspace` in memory, or forgets that draft with `None`, and asks the
    /// writer thread to save the map. The frame itself writes nothing.
    ///
    /// # Errors
    /// The workspace has no id, or more drafts are kept than fit beside the cloud settings.
    pub(super) fn keep(&mut self, root: &Path, workspace: &str, kept: Option<Kept>, failure: Failure) -> Result<()> {
        if self.kept.keep(workspace, kept.clone())? {
            self.changed.insert(workspace.trim().to_owned(), kept);
            self.save(root, failure);
        }
        Ok(())
    }

    /// Hands the map in memory to the writer thread, which writes the newest map of a burst: the
    /// file holds every record either way, so the maps between need no write of their own.
    fn save(&mut self, root: &Path, failure: Failure) {
        if self.writer.is_none() {
            let (wake, gate) = (self.wake.clone(), self.gate.take());
            self.writer = Some(spawn(wake, gate));
        }
        let job = Job {
            root: root.to_path_buf(),
            drafts: self.kept.clone(),
            failure,
        };
        if let Some(writer) = &self.writer
            && writer.jobs.send(job).is_ok()
        {
            self.writing = true;
        }
    }

    /// Takes the records a worker thread read, with this window's unconfirmed changes on top.
    fn merge(&mut self, drafts: Drafts) {
        let mut merged = drafts;
        for (workspace, kept) in &self.changed {
            // Each one was checked when this window applied it, so this cannot fail.
            if let Err(error) = merged.keep(workspace, kept.clone()) {
                tracing::warn!(%error, %workspace, "a changed cloud draft is not merged");
            }
        }
        self.kept = merged;
    }

    /// Applies the map a worker thread finished with, and reports why it is not on the file.
    fn applied(&mut self, written: Written) -> Option<(Failure, String)> {
        // A write of an older map leaves the newest one for the next frame's job.
        self.writing = written.drafts != self.kept;
        if written.error.is_none() {
            self.changed
                .retain(|workspace, kept| written.drafts.get(workspace) != kept.as_ref());
        }
        written.error.map(|error| (written.failure, error))
    }

    /// Waits for the saves the frames asked for, so that a test reads the file they wrote.
    #[cfg(test)]
    pub(super) fn settle(&mut self) {
        while self.writing {
            let Some(written) = self
                .writer
                .as_ref()
                .and_then(|writer| writer.answers.recv_timeout(std::time::Duration::from_secs(10)).ok())
            else {
                return;
            };
            self.applied(written);
        }
    }

    /// Makes the writer thread hold each write until the test sends on the returned channel, so
    /// that a test can show a frame returns before the file is written. Call it before the first
    /// save: the thread takes the grip that exists when it starts.
    #[cfg(test)]
    pub(super) fn hold_saves(&mut self) -> Sender<()> {
        let (release, gate) = mpsc::channel();
        self.gate = Some(gate);
        release
    }
}

/// Starts the thread that writes the maps a frame hands it, in order.
fn spawn(wake: Option<Context>, gate: Option<Receiver<()>>) -> Writer {
    let (jobs, queue): (Sender<Job>, Receiver<Job>) = mpsc::channel();
    let (results, answers): (Sender<Written>, Receiver<Written>) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(mut job) = queue.recv() {
            // The newest map of a burst holds every record the earlier maps of it held.
            while let Ok(next) = queue.try_recv() {
                job = next;
            }
            if let Some(gate) = &gate {
                // A test holds the write here, after the frame that asked for it returned.
                let _ = gate.recv();
            }
            let error = job.drafts.save(&job.root).err().map(|error| error.to_string());
            let written = Written {
                drafts: job.drafts,
                failure: job.failure,
                error,
            };
            if results.send(written).is_err() {
                return;
            }
            if let Some(ctx) = &wake {
                ctx.request_repaint();
            }
        }
    });
    Writer { jobs, answers }
}
