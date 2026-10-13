//! The host side of cloud browser uploads.
//!
//! The relay shows the pending file chooser of the worker browser as a request
//! of the panel's own picker, sends the files that the person picks or drops,
//! and gives the worker the staged batch in the next poll. The worker and its
//! pages are not trusted: their text is shortened, their limits only lower the
//! host limits, and the host never reads a path from the worker.
use super::transfer::{self, Progress, shorten};
use crate::browser::file_chooser::{FileChooserAnswer, FileChooserHandle};
use horizon_browser_protocol::cloud_view::{
    CloudChooserAnswer, CloudFileDrop, CloudStagedFiles, CloudUploadLimits, CloudUploadPoll, CloudUploadState,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// The most characters of the `accept` text of a page that the host uses.
const MAX_ACCEPT_CHARS: usize = 8192;

/// What the next poll gives the worker.
#[derive(Debug, Default)]
pub(super) struct Outgoing {
    answer: Option<CloudChooserAnswer>,
    drop: Option<CloudFileDrop>,
}

pub(super) type SharedOutgoing = Arc<Mutex<Outgoing>>;

/// Takes the answer and the drop for one poll.
pub(super) fn next_poll(outgoing: &SharedOutgoing) -> CloudUploadPoll {
    let mut outgoing = outgoing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    CloudUploadPoll {
        answer: outgoing.answer.take(),
        drop: outgoing.drop.take(),
        ..CloudUploadPoll::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Purpose {
    /// The answer to the worker request `worker`, shown as the local request `host`.
    Answer {
        worker: u64,
        host: u64,
    },
    Drop {
        page: u64,
        x: f64,
        y: f64,
    },
}

/// The SSH process of a transfer, while it runs.
pub(super) type ChildSlot = Arc<Mutex<Option<std::process::Child>>>;

/// One running transfer. Dropping it stops the transfer and its SSH process,
/// also while the worker does not answer.
pub(super) struct Transfer {
    pub(super) purpose: Purpose,
    pub(super) progress: Arc<Progress>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) child: ChildSlot,
    pub(super) done: mpsc::Receiver<Result<CloudStagedFiles, String>>,
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            let _ = child.kill();
        }
    }
}

/// Starts a transfer of the files with the effective limits.
pub(super) type Start<'a> = &'a dyn Fn(Vec<PathBuf>, CloudUploadLimits, Purpose) -> Transfer;

#[derive(Default)]
pub(super) struct Relay {
    pub(super) outgoing: SharedOutgoing,
    worker: Option<CloudUploadState>,
    /// The worker request and the local request that shows it.
    mirrored: Option<(u64, u64)>,
    /// `None` until the first state: earlier errors and notices are not shown again.
    errors_seen: Option<u64>,
    notices_seen: Option<u64>,
    transfer: Option<Transfer>,
}

impl Relay {
    /// Applies the upload state of one poll. Returns a notice for the person.
    pub(super) fn apply(&mut self, state: Option<CloudUploadState>, chooser: &FileChooserHandle) -> Option<String> {
        chooser.relay_supported(state.as_ref().is_some_and(|state| state.file_chooser));
        let request = state.as_ref().and_then(|state| state.request.as_ref());
        if request.map(|request| request.id) != self.mirrored.map(|(worker, _)| worker) {
            if let Some((_, host)) = self.mirrored.take() {
                chooser.relay_close(host);
                if matches!(self.transfer.as_ref().map(|transfer| transfer.purpose), Some(Purpose::Answer { host: busy, .. }) if busy == host)
                {
                    self.transfer = None;
                }
            }
            if let Some(request) = request {
                let accept: String = request.accept.chars().take(MAX_ACCEPT_CHARS).collect();
                let host = chooser.relay_open(request.multiple, accept, shorten(&request.origin));
                self.mirrored = Some((request.id, host));
            }
        }
        let error = state.as_ref().and_then(|state| state.error.as_ref());
        let errors = error.map_or(0, |error| error.sequence);
        if self.errors_seen.is_some_and(|seen| errors > seen)
            && let (Some(error), Some((worker, host))) = (error, self.mirrored)
            && error.request == worker
        {
            chooser.relay_fail(host, shorten(&error.message));
        }
        self.errors_seen = Some(errors);
        let notice = state.as_ref().and_then(|state| state.notice.as_ref());
        let notices = notice.map_or(0, |notice| notice.sequence);
        let shown = notice
            .filter(|_| self.notices_seen.is_some_and(|seen| notices > seen))
            .map(|notice| shorten(&notice.message));
        self.notices_seen = Some(notices);
        self.worker = state;
        shown
    }

    /// Sends the person's answer to the mirrored request, after its files arrived.
    pub(super) fn take_answer(&mut self, chooser: &FileChooserHandle, start: Start<'_>) {
        let Some((worker, host)) = self.mirrored else {
            return;
        };
        match chooser.relay_take_answer(host) {
            None => {}
            Some(FileChooserAnswer::Cancel) => self.send_answer(worker, None),
            Some(FileChooserAnswer::Files(paths)) => match self.start(paths, Purpose::Answer { worker, host }, start) {
                Ok(()) => {}
                Err(message) => chooser.relay_fail(host, message),
            },
        }
    }

    /// Sends dropped files, then drops them at the point of the page that the person saw.
    pub(super) fn drop_files(&mut self, paths: Vec<PathBuf>, x: f64, y: f64, start: Start<'_>) -> Result<(), String> {
        let state = self
            .worker
            .as_ref()
            .ok_or("Wait until the cloud browser is connected, then drop the files again.")?;
        if !state.drops {
            return Err(
                "This cloud browser does not accept dropped files. Rebuild the cloud with a newer image.".into(),
            );
        }
        let page = state.page;
        self.start(paths, Purpose::Drop { page, x, y }, start)
    }

    /// Handles a finished transfer. Returns a notice for the person.
    pub(super) fn finish(&mut self, chooser: &FileChooserHandle) -> Option<String> {
        let result = match self.transfer.as_ref()?.done.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("The upload stopped".into()),
        };
        let purpose = self.transfer.take()?.purpose;
        match (purpose, result) {
            (Purpose::Answer { worker, host }, Ok(files)) => {
                // A replaced request keeps nothing: the worker removes unused files.
                if self.mirrored == Some((worker, host)) {
                    self.send_answer(worker, Some(files));
                }
                None
            }
            (Purpose::Answer { host, .. }, Err(message)) => {
                chooser.relay_fail(host, message);
                None
            }
            (Purpose::Drop { page, x, y }, Ok(files)) => {
                self.outgoing
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .drop = Some(CloudFileDrop { page, x, y, files });
                None
            }
            (Purpose::Drop { .. }, Err(message)) => Some(format!("File drop failed: {message}")),
        }
    }

    /// Ends the mirrored request and the transfer before the connection is replaced.
    pub(super) fn close(&mut self, chooser: &FileChooserHandle) {
        if let Some((_, host)) = self.mirrored.take() {
            chooser.relay_close(host);
        }
        self.transfer = None;
    }

    /// Bytes sent and bytes in all, while a transfer runs.
    pub(super) fn progress(&self) -> Option<(u64, u64)> {
        self.transfer.as_ref().map(|transfer| {
            (
                transfer.progress.sent.load(Ordering::Relaxed),
                transfer.progress.total.load(Ordering::Relaxed),
            )
        })
    }

    pub(super) fn drops(&self) -> bool {
        self.worker.as_ref().is_some_and(|state| state.drops)
    }

    fn start(&mut self, paths: Vec<PathBuf>, purpose: Purpose, start: Start<'_>) -> Result<(), String> {
        if self.transfer.is_some() {
            return Err("Wait for the current upload to finish, then try again.".into());
        }
        let limits = self
            .worker
            .as_ref()
            .ok_or("This cloud browser does not accept uploads")
            .map_err(str::to_owned)
            .and_then(|state| transfer::effective_limits(state.limits))?;
        self.transfer = Some(start(paths, limits, purpose));
        Ok(())
    }

    fn send_answer(&self, request: u64, files: Option<CloudStagedFiles>) {
        self.outgoing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .answer = Some(CloudChooserAnswer { request, files });
    }
}

#[cfg(test)]
mod tests;
