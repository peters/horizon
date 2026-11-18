//! One live link of a server: the loop that reads the requests of its computer,
//! and the thread that writes the frames that wait for it.
//!
//! A link becomes a live link with `Open` as its first request after the greeting.
//! From then on it carries live requests only: any other frame ends it with a
//! typed error. The reading loop hands each request to the hub and never waits
//! for another link. The frames for this computer wait in its outbox, and one
//! thread hands them to the writing thread of the link one at a time, so the
//! frames that wait are counted in one place.
//!
//! A third thread is the sign of life of the link ([`Alive`]): it sends a
//! heartbeat while the link has nothing else to say, and it ends a link whose
//! computer sent nothing for the bound, so a computer that went away without
//! closing its transport leaves the list of the other computers.

use std::{
    io::Read,
    sync::{Arc, mpsc},
    thread,
};

use super::alive::Alive;
use super::outbox::{Next, Outbox, Tag};
use super::protocol::{LiveEvent, LiveRequest};
use crate::frame::FrameError;
use crate::protocol::{Frame, LinkErrorCode};
use crate::server::{LinkRegistry, LinkTicket, Outgoing, error_frame, next_frame, post};

/// Serves the live link that `first` opens, until its computer ends it or breaks
/// the protocol. `fresh` says that `first` is the first request of the link: a
/// link that answered a request of the store does not become a live link.
///
/// # Errors
/// Returns why the link failed.
pub(crate) fn serve<R: Read>(
    reader: &mut R,
    frames: &mpsc::Sender<Outgoing>,
    registry: &Arc<LinkRegistry>,
    ticket: &Arc<LinkTicket>,
    first: LiveRequest,
    fresh: bool,
) -> Result<(), FrameError> {
    let end = |code, reason| post(frames, error_frame(code, reason));
    let (LiveRequest::Open { label }, true) = (first, fresh) else {
        end(
            LinkErrorCode::NotLive,
            "a live link starts with `open` as the first request of a link",
        );
        return Ok(());
    };
    let outbox = Arc::new(Outbox::default());
    let writing = {
        let (outbox, frames) = (Arc::clone(&outbox), frames.clone());
        let (registry, ticket) = (Arc::clone(registry), Arc::clone(ticket));
        thread::Builder::new()
            .name("horizon-sync-live".to_owned())
            .spawn(move || write_waiting(&outbox, &frames, &registry, &ticket))
    };
    if writing.is_err() || !registry.live.open(ticket, &label, &outbox) {
        outbox.close();
        end(LinkErrorCode::LiveLimit, "This server cannot open one more live link.");
        return Ok(());
    }
    let alive = Arc::new(Alive::default());
    let sign = {
        let (alive, outbox, ticket) = (Arc::clone(&alive), Arc::clone(&outbox), Arc::clone(ticket));
        thread::Builder::new()
            .name("horizon-sync-live-sign".to_owned())
            .spawn(move || sign_of_life(&alive, &outbox, &ticket))
    };
    let result = loop {
        match next_frame(reader, frames) {
            Ok(Some(Frame::Live(request))) => {
                alive.read();
                registry.live.request(&ticket.device, ticket.id, request);
            }
            Ok(Some(_)) => {
                end(
                    LinkErrorCode::LiveLink,
                    "a live link carries live requests and no other frame",
                );
                break Ok(());
            }
            Ok(None) => {
                // The computer ended the link, so nothing more is for it: a frame
                // that is still being written to it must not hold the link open.
                ticket.shut();
                break Ok(());
            }
            Err(error) => break Err(error),
        }
    };
    registry.live.close(&ticket.device, ticket.id);
    outbox.close();
    if let Ok(sign) = sign {
        // The sign of life ends within one tick of the closed outbox.
        let _ = sign.join();
    }
    result
}

/// Keeps the sign of life of one live link: a heartbeat frame while the link has
/// nothing else to send, and the end of a link whose computer sent nothing for
/// the bound. The `end` waits for the error frame, so this has a thread of its
/// own, as the end of a link that the hub names has.
pub(crate) fn sign_of_life(alive: &Alive, outbox: &Outbox, ticket: &Arc<LinkTicket>) {
    loop {
        thread::sleep(alive.tick());
        if outbox.closed() {
            return;
        }
        if alive.quiet() {
            let message = "This computer sent no frame for thirty seconds.";
            ticket.end(LinkErrorCode::LiveSilent, message);
            return;
        }
        if alive.beat_due() {
            let _ = outbox.push(LiveEvent::Heartbeat, Tag::Kept);
            alive.sent();
        }
    }
}

/// Hands the frames that wait for one live link to its writing thread, one at a
/// time, and tells the hub when a link that fell behind is ready again.
fn write_waiting(outbox: &Outbox, frames: &mpsc::Sender<Outgoing>, registry: &LinkRegistry, ticket: &LinkTicket) {
    loop {
        match outbox.next() {
            Next::Closed => return,
            Next::Resume => registry.live.resume(&ticket.device, ticket.id),
            Next::Event(event) => {
                let (written, waiting) = mpsc::sync_channel(1);
                let frame = Frame::LiveEvent(event);
                let written = Some(written);
                if frames.send(Outgoing { frame, written }).is_err() || waiting.recv().is_err() {
                    // The writing side of the link failed, so the link is over:
                    // its reading side ends too, and the hub forgets it.
                    outbox.close();
                    ticket.shut();
                    return;
                }
                outbox.written();
            }
        }
    }
}
