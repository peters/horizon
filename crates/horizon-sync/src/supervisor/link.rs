//! The frame loop of one open link: what this computer sends, the frames that the
//! server answers, and the frame that ends the link.
//!
//! A turn of the loop sends what the queue holds, one question of the window, and
//! one request for changes, and then waits for an answer. A frame that arrived
//! while the last turn did its work is answered first: the server writes the typed
//! error that ends a link before it closes the link, so that error is here when the
//! next write of this computer fails, and it decides how the link ended.

use std::{
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

use super::pull::{accept_pushed, apply_changes};
use super::secrets::fill_secrets;
use super::state::{ServerReply, ServerRequest};
use super::{LINK_SLICE, LinkBounds, SupervisorConfig, SyncHandle, logged_out};
use crate::frame::write_frame;
use crate::protocol::Frame;

/// How long a turn whose write failed waits for the frames that the server sent
/// before it closed the link.
///
/// The server writes the typed error that ends a link before it closes the link,
/// so that error is on its way to this computer when the next write of this
/// computer fails. The wait ends at once when the reader of the link sees the end
/// of it, so a lost link costs one slice of this bound and no more.
const END_DRAIN_BOUND: Duration = Duration::from_secs(2);

/// How one link ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum End {
    /// The link failed, reached its bound or ended, so the next attempt starts on
    /// the delays of a lost link.
    Link,
    /// The server does not know the token of this computer, so no attempt can
    /// succeed until a person joins it again.
    LoggedOut,
}

/// The two halves of one open link, and the answers of the server.
struct Link<'a> {
    config: &'a SupervisorConfig,
    handle: &'a SyncHandle,
    stop: &'a AtomicBool,
    bounds: &'a LinkBounds,
    /// The frames of this computer leave here.
    input: &'a mut (dyn Write + Send),
    /// The frames of the server arrive here.
    answers: &'a Receiver<Frame>,
    /// A local change, so this turn sends what the queue holds.
    kicked: &'a Receiver<()>,
    /// One question of the window or of a worker.
    asked: &'a Receiver<(ServerRequest, mpsc::Sender<ServerReply>)>,
    /// When the open wait left, so its answer is due at its bound.
    waiting: Option<Instant>,
    /// The index that the open request reads from, so the answer is known to be the
    /// one that a mark set back or not.
    asking_from: u64,
    /// When the open push left, so its answer is due at once.
    sending: Option<Instant>,
    /// The first request of a link is a pull: the server answers it at once, while a
    /// wait stays open at its bound when nothing changed yet.
    first: bool,
    /// The one question that waits for its answer.
    question: Asked,
}

impl Link<'_> {
    /// Answers the frames of one open link until it ends, the stop flag is set, or a
    /// request stays without an answer for its bound.
    ///
    /// A bound is what an idle computer needs most: a server that dies leaves the
    /// socket of the link open for as long as the transport lives, so a wait that
    /// never answers is the only sign of a link that is over. The answer of a push, a
    /// pull and a question is due at once, and the answer of a wait is due at the
    /// bound of the server that holds it open.
    fn serve(&mut self) -> End {
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return End::Link;
            }
            // A frame that arrived while the last turn did its work comes first: a
            // removal that the server wrote just before it closed the link is
            // already here, and the next write of this computer may fail before
            // this link would read it.
            if let Some(end) = self.answer_what_arrived() {
                return end;
            }
            if !self.push() || !self.ask() || !self.pull() {
                return self.end_after_a_failed_write();
            }
            match self.answers.recv_timeout(LINK_SLICE) {
                Ok(frame) => {
                    if let Some(end) = self.answered(frame) {
                        return end;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(end) = self.timed_out() {
                        return end;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return End::Link,
            }
            // A local change is the only work of a turn that the server did not
            // answer; the links of both are read on the next pass of the loop.
            while self.kicked.try_recv().is_ok() {}
        }
    }

    /// Sends what the queue holds, unless a push already waits for its answer.
    fn push(&mut self) -> bool {
        if self.sending.is_some() {
            return true;
        }
        let chunk = {
            let state = self.handle.lock();
            state.client.next_push()
        };
        let Some(chunk) = chunk else {
            return true;
        };
        let intents = fill_secrets(self.handle, chunk);
        if intents.is_empty() {
            return true;
        }
        let frame = Frame::Push(crate::protocol::Push { intents });
        if let Err(error) = write_frame(self.input, &frame) {
            tracing::warn!(%error, "the link could not send a push");
            return false;
        }
        self.sending = Some(Instant::now());
        true
    }

    /// Sends one question of the window, unless a question waits for its answer.
    fn ask(&mut self) -> bool {
        if !self.question.waits() {
            return true;
        }
        let Ok((request, reply)) = self.asked.try_recv() else {
            return true;
        };
        match self.question.ask(request, reply) {
            Ok(frame) => {
                if let Err(error) = write_frame(self.input, &frame) {
                    tracing::warn!(%error, "the link could not send a question");
                    return false;
                }
                true
            }
            Err(reply) => {
                let _ = reply.send(ServerReply::Failed(
                    "This computer has no link to the server now.".to_owned(),
                ));
                true
            }
        }
    }

    /// Asks for the changes of the server, unless one request waits for its answer.
    fn pull(&mut self) -> bool {
        if self.waiting.is_some() {
            return true;
        }
        let frame = {
            let state = self.handle.lock();
            self.asking_from = state.client.read_from();
            if self.first {
                state.client.pull_frame()
            } else {
                state.client.wait_frame()
            }
        };
        if let Err(error) = write_frame(self.input, &frame) {
            tracing::warn!(%error, "the link could not ask for changes");
            return false;
        }
        self.waiting = Some(Instant::now());
        // The state publishes the instant that the bound of this request runs
        // from, so a reader measures the same bound from the same start.
        self.handle.lock().request_since = self.waiting;
        true
    }

    /// Answers the frames that already arrived. Answers the end of the link when
    /// one of them ended it.
    fn answer_what_arrived(&mut self) -> Option<End> {
        while let Ok(frame) = self.answers.try_recv() {
            if let Some(end) = self.answered(frame) {
                return Some(end);
            }
        }
        None
    }

    /// The end of a link whose write failed. A typed error that the server wrote
    /// before it closed the link is on its way here, and it is the end of this
    /// link: a removal must not read as a link that was lost.
    fn end_after_a_failed_write(&mut self) -> End {
        let deadline = Instant::now() + END_DRAIN_BOUND;
        loop {
            if let Some(end) = self.answer_what_arrived() {
                return end;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return End::Link;
            }
            match self.answers.recv_timeout(remaining.min(LINK_SLICE)) {
                Ok(frame) => {
                    if let Some(end) = self.answered(frame) {
                        return end;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // The reader of the link is gone and every frame that it read is
                // answered, so this link has no more answer to give.
                Err(RecvTimeoutError::Disconnected) => return End::Link,
            }
        }
    }

    /// Applies one answer of the server. Answers `None` to continue the link, and the
    /// end of it when the answer ends it.
    fn answered(&mut self, frame: Frame) -> Option<End> {
        match frame {
            Frame::Pushed(reply) => {
                self.sending = None;
                accept_pushed(self.handle, &reply);
                None
            }
            Frame::Changes(changes) => {
                self.waiting = None;
                self.handle.lock().request_since = None;
                self.first = false;
                if let Err(error) = apply_changes(self.handle, &changes, self.asking_from) {
                    tracing::warn!(%error, "the changes of the server were not applied");
                    return Some(End::Link);
                }
                (self.handle.wake)();
                None
            }
            Frame::Invited(_) | Frame::DeviceList(_) | Frame::DeviceRemoved(_) => {
                if self.question.answer(&frame).is_none() {
                    tracing::warn!("the server answered a question that this computer did not ask");
                    return Some(End::Link);
                }
                None
            }
            Frame::Error(error) => {
                tracing::warn!(code = error.code.as_str(), reason = %error.message, "the server ended the link");
                if matches!(
                    error.code,
                    crate::protocol::LinkErrorCode::Removed | crate::protocol::LinkErrorCode::NotLoggedIn
                ) {
                    logged_out(self.config, self.handle, error.code);
                    return Some(End::LoggedOut);
                }
                Some(End::Link)
            }
            _other => {
                tracing::warn!("the server answered a frame that this computer did not ask for");
                Some(End::Link)
            }
        }
    }

    /// Answers the end of a link whose request stayed without an answer for its
    /// bound, or `None` while every request is inside its bound.
    fn timed_out(&mut self) -> Option<End> {
        let answer_due = if self.first {
            self.bounds.request
        } else {
            self.bounds.wait
        };
        if self.question.past_bound(self.bounds.request) {
            tracing::warn!(
                bound_secs = self.bounds.request.as_secs(),
                "the server did not answer a question in time, so the link is lost"
            );
            return Some(End::Link);
        }
        if past_bound(self.sending, self.bounds.request) {
            tracing::warn!(
                bound_secs = self.bounds.request.as_secs(),
                "the link did not answer a push in time, so it is lost"
            );
            return Some(End::Link);
        }
        if past_bound(self.waiting, answer_due) {
            tracing::warn!(
                bound_secs = answer_due.as_secs(),
                "the link did not answer a wait in time, so it is lost"
            );
            return Some(End::Link);
        }
        None
    }
}

/// The settings, the state and the channels of one open link.
pub(super) struct LinkParts<'a> {
    pub(super) config: &'a SupervisorConfig,
    pub(super) handle: &'a SyncHandle,
    pub(super) stop: &'a AtomicBool,
    pub(super) bounds: &'a LinkBounds,
    pub(super) input: &'a mut (dyn Write + Send),
    pub(super) answers: &'a Receiver<Frame>,
    pub(super) kicked: &'a Receiver<()>,
    pub(super) asked: &'a Receiver<(ServerRequest, mpsc::Sender<ServerReply>)>,
}

/// Serves one open link with the transport and the channels of `parts`.
pub(super) fn serve_link(parts: LinkParts<'_>) -> End {
    let LinkParts {
        config,
        handle,
        stop,
        bounds,
        input,
        answers,
        kicked,
        asked,
    } = parts;
    Link {
        config,
        handle,
        stop,
        bounds,
        input,
        answers,
        kicked,
        asked,
        waiting: None,
        asking_from: 0,
        sending: None,
        first: true,
        question: Asked::default(),
    }
    .serve()
}

/// The one question that a link has asked and not yet had answered.
///
/// The server answers each question with one frame of its own, so a link that
/// keeps one question in flight cannot confuse an answer with the changes of a
/// wait or with the result of a push.
#[derive(Default)]
struct Asked {
    waiting: Option<(Instant, ServerRequest, mpsc::Sender<ServerReply>)>,
}

impl Asked {
    /// Whether no question waits for its answer.
    fn waits(&self) -> bool {
        self.waiting.is_none()
    }

    /// Sends one question, or answers the one that waits with the reason that
    /// this computer has no link now.
    fn ask(
        &mut self,
        request: ServerRequest,
        reply: mpsc::Sender<ServerReply>,
    ) -> Result<Frame, mpsc::Sender<ServerReply>> {
        if self.waiting.is_some() {
            return Err(reply);
        }
        let frame = match &request {
            ServerRequest::Invite => Frame::Invite(crate::protocol::Invite {}),
            ServerRequest::Devices => Frame::Devices(crate::protocol::Devices {}),
            ServerRequest::Remove(device) => {
                Frame::DeviceRemove(crate::protocol::DeviceRemove { device: device.clone() })
            }
        };
        self.waiting = Some((Instant::now(), request, reply));
        Ok(frame)
    }

    /// Answers the question that waits, when the frame is its answer.
    fn answer(&mut self, frame: &Frame) -> Option<mpsc::Sender<ServerReply>> {
        let (_, request, reply) = self.waiting.take()?;
        let answer = match (frame, &request) {
            (Frame::Invited(invited), ServerRequest::Invite) => ServerReply::Invited(invited.clone()),
            (Frame::DeviceList(list), ServerRequest::Devices) => ServerReply::Devices {
                rows: list.rows.clone(),
                now_millis: list.now_millis,
            },
            (Frame::DeviceRemoved(removed), ServerRequest::Remove(_)) => ServerReply::Removed {
                device: removed.device.clone(),
                removed: removed.removed,
            },
            _ => ServerReply::Failed("The server answered another question than the one asked.".to_owned()),
        };
        let _ = reply.send(answer);
        Some(reply)
    }

    /// Whether the question that waits has waited longer than `bound`, and answers
    /// it with the reason that the server did not answer.
    fn past_bound(&mut self, bound: Duration) -> bool {
        let Some((at, _, _)) = self.waiting.as_ref() else {
            return false;
        };
        if at.elapsed() < bound {
            return false;
        }
        if let Some((_, _, reply)) = self.waiting.take() {
            let _ = reply.send(ServerReply::Failed(
                "The server did not answer this computer in time.".to_owned(),
            ));
        }
        true
    }
}

/// Whether a request that was sent at `since` has waited for its answer longer
/// than `bound`.
fn past_bound(since: Option<Instant>, bound: Duration) -> bool {
    since.is_some_and(|since| since.elapsed() >= bound)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::test_support::keys_of_a_test;

    use super::*;

    /// A writing side whose peer is gone, for the link of a test that the server
    /// ended one frame ago.
    struct GonePipe;

    impl std::io::Write for GonePipe {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The server writes the error that ends a link before it closes the link, so
    /// the error is on its way here while this computer still sends its next frame.
    /// That frame fails first, and the error that already arrived decides the end:
    /// a removal must read as a removal and not as a link that was lost.
    #[test]
    fn an_error_that_arrived_before_a_failed_write_ends_the_link_with_its_reason() {
        use crate::protocol::{Frame, LinkError, LinkErrorCode};

        let directory = tempfile::tempdir().expect("a directory");
        let keys = horizon_core::MachineKeys::new(directory.path().join("keys"));
        let mut config = SupervisorConfig::for_server(
            "server-vm",
            "/opt/horizon --server-link",
            directory.path(),
            keys_of_a_test(keys),
        )
        .expect("the settings of the link");
        config.address = Some("127.0.0.1:9".to_owned());
        let handle = SyncHandle::open(&config, Arc::new(|| {})).expect("the handle of a link");

        let (server, answers) = mpsc::channel();
        server
            .send(Frame::Error(LinkError::new(
                LinkErrorCode::Removed,
                "This computer was removed from the server.",
            )))
            .expect("the frame of the removal");
        drop(server);
        let (kick, kicked) = mpsc::channel();
        let (ask, asked) = mpsc::channel();
        let stop = AtomicBool::new(false);
        let mut input = GonePipe;
        let mut link = Link {
            config: &config,
            handle: &handle,
            stop: &stop,
            bounds: &LinkBounds::STANDARD,
            input: &mut input,
            answers: &answers,
            kicked: &kicked,
            asked: &asked,
            waiting: None,
            asking_from: 0,
            sending: None,
            first: false,
            question: Asked::default(),
        };

        assert_eq!(
            link.serve(),
            End::LoggedOut,
            "the answer of the server ends the link, and no later attempt starts"
        );
        assert_eq!(
            handle.state(),
            crate::client::LinkState::Removed,
            "the person reads that the server removed this computer"
        );
        drop((kick, ask));
    }
}
