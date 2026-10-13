//! The page file chooser of a worker browser, relayed to the host that presents it.
//!
//! A chooser waits only while a host that relays uploads keeps polling. Without
//! such a host the service cancels the chooser, so that neither the page nor an
//! agent waits for a person who cannot answer. A chooser also ends after
//! [`MAX_PENDING`].
use super::staging::Staging;
use horizon_browser::{BrowserCommand, FileChooserAnswer, FileChooserHandle};
use horizon_browser_protocol::cloud_view::{
    CLOUD_UPLOAD_VERSION, CloudBrowserNotice, CloudChooserAnswer, CloudChooserError, CloudChooserRequest,
    CloudFileDrop, CloudUploadLimits, CloudUploadState,
};
use std::time::{Duration, Instant};

/// How long a chooser waits after the last poll of a host that relays uploads.
const PRESENTER_GRACE: Duration = Duration::from_secs(10);
const MAX_PENDING: Duration = Duration::from_mins(20);

#[derive(Debug, Default)]
pub(super) struct Relay {
    presenter: Option<Instant>,
    /// The pending request and when the relay first saw it.
    pending: Option<(u64, Instant)>,
    error: Option<CloudChooserError>,
    errors: u64,
    notice: Option<CloudBrowserNotice>,
    notices: u64,
    /// The page generation: grows with each top-level navigation.
    page: u64,
}

impl Relay {
    pub(super) fn presented(&mut self, now: Instant) {
        self.presenter = Some(now);
    }

    /// Runs on each service loop.
    pub(super) fn tick(&mut self, chooser: &FileChooserHandle, now: Instant) {
        let request = chooser.request().map(|request| request.id);
        if request != self.pending.map(|(id, _)| id) {
            self.pending = request.map(|id| (id, now));
            self.error = None;
        }
        if let Some(message) = chooser.take_error()
            && let Some(id) = request
        {
            self.fail(id, message);
        }
        if let Some((id, since)) = self.pending {
            let presented = self
                .presenter
                .is_some_and(|seen| now.saturating_duration_since(seen) < PRESENTER_GRACE);
            if !presented || now.saturating_duration_since(since) >= MAX_PENDING {
                let _ = chooser.respond(id, FileChooserAnswer::Cancel);
            }
        }
    }

    pub(super) fn navigated(&mut self) {
        self.page = self.page.wrapping_add(1);
    }

    pub(super) fn notice(&mut self, message: String) {
        self.notices += 1;
        self.notice = Some(CloudBrowserNotice {
            sequence: self.notices,
            message,
        });
    }

    pub(super) fn answer(
        &mut self,
        chooser: &FileChooserHandle,
        staging: &mut Staging,
        browser: &str,
        answer: CloudChooserAnswer,
        now: Instant,
    ) {
        let Some(files) = answer.files else {
            let _ = chooser.respond(answer.request, FileChooserAnswer::Cancel);
            return;
        };
        match staging.resolve(browser, &files) {
            Ok(paths) => {
                if chooser.respond(answer.request, FileChooserAnswer::Files(paths)) {
                    staging.mark_used(browser, &files.batch, now);
                } else {
                    self.notice(
                        "The page closed the file selection before the files arrived. Choose the files again.".into(),
                    );
                }
            }
            Err(message) => self.fail(answer.request, message),
        }
    }

    pub(super) fn drop_files(
        &mut self,
        staging: &mut Staging,
        browser: &str,
        drop: &CloudFileDrop,
        now: Instant,
        send: impl FnOnce(BrowserCommand) -> bool,
    ) {
        if !drop.x.is_finite() || !drop.y.is_finite() || drop.x < 0.0 || drop.y < 0.0 {
            return self.notice("File drop failed: the drop point is outside the page".into());
        }
        if drop.page != self.page {
            return self
                .notice("File drop failed: the page changed before the files arrived. Drop the files again.".into());
        }
        match staging.resolve(browser, &drop.files) {
            Ok(paths) => {
                if send(BrowserCommand::DropFiles {
                    x: drop.x,
                    y: drop.y,
                    paths,
                }) {
                    staging.mark_used(browser, &drop.files.batch, now);
                } else {
                    self.notice("File drop failed: the worker browser is busy. Drop the files again.".into());
                }
            }
            Err(message) => self.notice(format!("File drop failed: {message}")),
        }
    }

    pub(super) fn state(
        &self,
        chooser: &FileChooserHandle,
        drops: bool,
        limits: CloudUploadLimits,
    ) -> CloudUploadState {
        CloudUploadState {
            version: CLOUD_UPLOAD_VERSION,
            limits,
            file_chooser: chooser.supported(),
            drops,
            page: self.page,
            request: chooser.request().map(|request| CloudChooserRequest {
                id: request.id,
                multiple: request.multiple,
                accept: request.accept.clone(),
                origin: request.origin.clone(),
            }),
            error: self.error.clone(),
            notice: self.notice.clone(),
        }
    }

    /// A failure of the pending request goes back to its picker; any other
    /// failure becomes a notice.
    fn fail(&mut self, request: u64, message: String) {
        if self.pending.is_some_and(|(id, _)| id == request) {
            self.errors += 1;
            self.error = Some(CloudChooserError {
                request,
                sequence: self.errors,
                message,
            });
        } else {
            self.notice(message);
        }
    }
}

#[cfg(test)]
mod tests;
