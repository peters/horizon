//! Control side of a mirroring session: launch the mirroring receiver, OFFER
//! the streams, and follow the session while the frames go over UDP.
use super::{EVENT_POLL, LiveState, NO_MEDIA_SESSION, RELAUNCH_ATTEMPTS, Session};
use crate::{
    Application, CastClient, Error, Result,
    mirror::{self, MIRRORING_RECEIVER, MirrorStream, NS_WEBRTC},
};
use std::time::{Duration, Instant};

/// Receivers answer an OFFER within a second or two.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// Receivers report on the streams several times a second while they play.
const FEEDBACK_TIMEOUT: Duration = Duration::from_secs(10);
const OFFER_SEQUENCE: u64 = 1;

impl Session {
    pub(super) fn cast_mirror(&self, stream: &MirrorStream) -> Result<()> {
        let Some(mut client) = self.connect()? else {
            return Ok(());
        };
        let mut relaunches = 0;
        loop {
            let app = client.launch(MIRRORING_RECEIVER)?;
            self.set(LiveState::Buffering);
            let result = self.mirror(&client, &app, stream);
            stream.disconnect();
            match result {
                Ok(true) => {
                    let _ = client.stop_application(&app.session_id);
                    return Ok(());
                }
                // The receiver dropped us, as a TV does when it restarts its
                // cast runtime: offer again on a fresh connection, but only to a
                // receiver showing nothing but its idle screen. A mirroring
                // session still running may now be another sender's.
                Err(Error::Closed) if !self.stopped() && relaunches < RELAUNCH_ATTEMPTS => {
                    relaunches += 1;
                    self.set(LiveState::Buffering);
                    let Some(fresh) = self.connect()? else {
                        return Ok(());
                    };
                    let status = fresh.receiver_status()?;
                    let busy = status.applications.iter().any(|running| !running.is_idle_screen);
                    if status.is_stand_by || busy {
                        return Ok(());
                    }
                    tracing::debug!(relaunches, "receiver dropped the mirroring session; offering again");
                    client = fresh;
                }
                // Ended by the receiver or another sender, or failed: leave the
                // receiver alone. Another sender may have joined the mirroring
                // session, which shows as our feedback stopping.
                result => return result.map(|_| ()),
            }
        }
    }

    /// Offers the streams and follows the session until the host stops it
    /// (`Ok(true)`) or the receiver ends it (`Ok(false)`).
    fn mirror(&self, client: &CastClient, app: &Application, stream: &MirrorStream) -> Result<bool> {
        let proposal = stream.propose()?;
        let message = mirror::offer(OFFER_SEQUENCE, &proposal.streams, self.options.playout_delay);
        client.send(&app.transport_id, NS_WEBRTC, &message)?;
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        let answer = loop {
            if self.stopped() {
                return Ok(true);
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(Error::Timeout("OFFER".to_owned()));
            };
            let Some(event) = client.next_event(left.min(EVENT_POLL))? else {
                continue;
            };
            if event.namespace == NS_WEBRTC {
                if let Some(answer) = mirror::answer(&event.payload, OFFER_SEQUENCE) {
                    break answer?;
                }
            } else if !self.follow_confirmed(client, &event, app, NO_MEDIA_SESSION)? {
                return Ok(false);
            }
        };
        let sender = stream.connect(self.receiver, &proposal, &answer, self.options.playout_delay)?;
        let sending = Instant::now();
        loop {
            if self.stopped() {
                return Ok(true);
            }
            // Feedback shows the receiver gets our frames; silence means it stopped.
            let heard = sender.last_feedback().unwrap_or(sending);
            if heard.elapsed() > FEEDBACK_TIMEOUT {
                return Err(Error::Timeout("mirroring feedback".to_owned()));
            }
            if sender.last_feedback().is_some() && *super::lock(&self.state) != LiveState::Playing {
                self.set(LiveState::Playing);
            }
            if let Some(event) = client.next_event(EVENT_POLL)?
                && event.namespace != NS_WEBRTC
                && !self.follow_confirmed(client, &event, app, NO_MEDIA_SESSION)?
            {
                return Ok(false);
            }
        }
    }
}
