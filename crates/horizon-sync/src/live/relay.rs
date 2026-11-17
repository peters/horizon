//! One live link of a server: the loop that reads the requests of its computer,
//! and the thread that writes the frames that wait for it.
//!
//! A link becomes a live link with `Open` as its first request after the greeting.
//! From then on it carries live requests only: any other frame ends it with a
//! typed error. The reading loop hands each request to the hub and never waits
//! for another link. The frames for this computer wait in its outbox, and one
//! thread hands them to the writing thread of the link one at a time, so the
//! frames that wait are counted in one place.

use std::{
    io::Read,
    sync::{Arc, mpsc},
    thread,
};

use super::outbox::{Next, Outbox};
use super::protocol::LiveRequest;
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
    let result = loop {
        match next_frame(reader, frames) {
            Ok(Some(Frame::Live(request))) => registry.live.request(&ticket.device, ticket.id, request),
            Ok(Some(_)) => {
                end(
                    LinkErrorCode::LiveLink,
                    "a live link carries live requests and no other frame",
                );
                break Ok(());
            }
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        }
    };
    registry.live.close(&ticket.device, ticket.id);
    outbox.close();
    result
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
