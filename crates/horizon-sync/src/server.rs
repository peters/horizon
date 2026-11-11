//! One link of one computer: the frames of a connection after its hello.
//!
//! The server reads a frame, answers it, and never blocks on a slow answer: every
//! frame that goes out passes through one writing thread of the connection, so a
//! [`Wait`] that is still open cannot hold a push. The device name of the hello is
//! the name of the link, and an intent that names another computer is refused, so
//! one computer cannot spend the intent counters of another.
//!
//! Limits of one link: a frame of at most 256 KiB, 256 intents in one push, and 32
//! open waits. The first two close the link with a typed error, because the frame
//! breaks the contract of the protocol; the third answers that one wait and keeps
//! the link.

use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::error::StoreError;
use crate::frame::{FrameError, FrameWriter, read_frame, write_frame};
use crate::intent::MAX_INTENTS_PER_PUSH;
use crate::protocol::{Frame, Hello, HelloOk, LinkError, LinkErrorCode, PROTOCOL_VERSION, check_device_name};
use crate::store::Store;

/// The most waits that one link holds open at once.
pub const MAX_OPEN_WAITS: usize = 32;

/// How long a wait stays open without a change.
pub const WAIT_BOUND: Duration = Duration::from_secs(50);

/// How often an open wait looks at the link before it gives up, so a link that
/// ends does not keep a thread for its whole bound.
const WAIT_SLICE: Duration = Duration::from_millis(200);

/// Answers one connection until its computer ends it.
///
/// # Errors
/// Returns why the link failed, for the caller to log. A computer that ends its
/// link answers `Ok(())`.
pub fn serve<R, W>(mut reader: R, writer: W, store: &Store) -> Result<(), FrameError>
where
    R: Read,
    W: FrameWriter + Send + 'static,
{
    let (frames, outgoing) = mpsc::channel::<Frame>();
    let closed = Arc::new(AtomicBool::new(false));
    let writing = thread::Builder::new()
        .name("horizon-sync-link".to_owned())
        .spawn(move || {
            let mut writer = writer;
            for frame in outgoing {
                if write_frame(&mut writer, &frame).is_err() {
                    return;
                }
            }
            // The writing side ends first: the computer reads the last frame, and
            // then the end of the link.
            let _ = writer.finish();
        })
        .map_err(|error| FrameError::Io(std::io::Error::other(format!("the link thread: {error}"))))?;
    let result = exchange(&mut reader, &frames, store, &closed);
    closed.store(true, Ordering::SeqCst);
    drop(frames);
    let _ = writing.join();
    result
}

/// The frames of one link, until the computer ends it or breaks the protocol.
fn exchange<R: Read>(
    reader: &mut R,
    frames: &mpsc::Sender<Frame>,
    store: &Store,
    closed: &Arc<AtomicBool>,
) -> Result<(), FrameError> {
    let device = greet(reader, frames, store)?;
    let Some(device) = device else {
        return Ok(());
    };
    let waits = Arc::new(AtomicUsize::new(0));
    loop {
        let Some(frame) = next_frame(reader, frames)? else {
            return Ok(());
        };
        match frame {
            Frame::Push(push) => {
                if push.intents.len() > MAX_INTENTS_PER_PUSH {
                    let _ = frames.send(error_frame(
                        LinkErrorCode::TooManyIntents,
                        &format!("one push holds at most {MAX_INTENTS_PER_PUSH} intents"),
                    ));
                    return Ok(());
                }
                match store.push(&device, &push.intents) {
                    Ok(reply) => {
                        let _ = frames.send(Frame::Pushed(reply));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "the server could not apply a push");
                        let _ = frames.send(error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
                    }
                }
            }
            Frame::Pull(pull) => match store.pull(pull.since, &pull.tip) {
                Ok(changes) => {
                    let _ = frames.send(Frame::Changes(changes));
                }
                Err(error) => {
                    tracing::warn!(%error, "the server could not read a pull");
                    let _ = frames.send(error_frame(LinkErrorCode::StoreFailed, &error.to_string()));
                }
            },
            Frame::Wait(wait) => open_wait(store, frames, closed, &waits, wait.since),
            Frame::Hello(_) | Frame::HelloOk(_) | Frame::Pushed(_) | Frame::Changes(_) | Frame::Error(_) => {
                let _ = frames.send(error_frame(
                    LinkErrorCode::BadFrame,
                    "a link carries one hello, then pushes, pulls and waits",
                ));
                return Ok(());
            }
        }
    }
}

/// Reads the hello of a link and answers it. Returns the device name, or `None`
/// when the link ended before its hello.
fn greet<R: Read>(reader: &mut R, frames: &mpsc::Sender<Frame>, store: &Store) -> Result<Option<String>, FrameError> {
    let Some(first) = next_frame(reader, frames)? else {
        return Ok(None);
    };
    let Frame::Hello(Hello { protocol, device, .. }) = first else {
        let _ = frames.send(error_frame(
            LinkErrorCode::NotHello,
            "the first frame of a link is a hello",
        ));
        return Ok(None);
    };
    if protocol > PROTOCOL_VERSION {
        let _ = frames.send(error_frame(
            LinkErrorCode::ProtocolVersion,
            &format!("the computer speaks protocol {protocol}, and this server speaks protocol {PROTOCOL_VERSION}"),
        ));
        return Ok(None);
    }
    if !check_device_name(&device) {
        let _ = frames.send(error_frame(
            LinkErrorCode::BadDevice,
            "the device name is 1 to 40 characters of a-z, 0-9 and -",
        ));
        return Ok(None);
    }
    let answer = HelloOk {
        protocol: PROTOCOL_VERSION,
        store: store.store_id().to_owned(),
        now_millis: u64::try_from(crate::store::now_millis()).unwrap_or(0),
    };
    if frames.send(Frame::HelloOk(answer)).is_err() {
        return Ok(None);
    }
    Ok(Some(device))
}

/// Reads one frame, and answers a frame that the server cannot read with the typed
/// error of the protocol before the link ends.
fn next_frame<R: Read>(reader: &mut R, frames: &mpsc::Sender<Frame>) -> Result<Option<Frame>, FrameError> {
    match read_frame(reader) {
        Ok(Some(frame)) => Ok(Some(frame)),
        Ok(None) => Ok(None),
        Err(error) => {
            let _ = frames.send(error_frame(error.code(), &error.to_string()));
            Err(error)
        }
    }
}

/// Opens one wait on its own thread, unless the link holds the maximum.
fn open_wait(
    store: &Store,
    frames: &mpsc::Sender<Frame>,
    closed: &Arc<AtomicBool>,
    waits: &Arc<AtomicUsize>,
    since: u64,
) {
    if waits.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN_WAITS {
        waits.fetch_sub(1, Ordering::SeqCst);
        let _ = frames.send(error_frame(
            LinkErrorCode::TooManyWaits,
            &format!("one link holds at most {MAX_OPEN_WAITS} open waits"),
        ));
        return;
    }
    let store = store.clone();
    let replying = frames.clone();
    let stopping = Arc::clone(closed);
    let counting = Arc::clone(waits);
    let spawned = thread::Builder::new()
        .name("horizon-sync-wait".to_owned())
        .spawn(move || {
            let answer = wait_for_change(&store, since, &stopping);
            counting.fetch_sub(1, Ordering::SeqCst);
            let frame = match answer {
                Ok(Some(changes)) => Frame::Changes(changes),
                Ok(None) => return,
                Err(error) => error_frame(LinkErrorCode::StoreFailed, &error.to_string()),
            };
            let _ = replying.send(frame);
        });
    if spawned.is_err() {
        waits.fetch_sub(1, Ordering::SeqCst);
        let _ = frames.send(error_frame(
            LinkErrorCode::StoreFailed,
            "the server could not open a wait",
        ));
    }
}

/// Waits for a change after `since` within the bound of the link, and answers
/// `None` when the link ended first.
fn wait_for_change(
    store: &Store,
    since: u64,
    closed: &AtomicBool,
) -> Result<Option<crate::store::Changes>, StoreError> {
    let deadline = Instant::now() + WAIT_BOUND;
    loop {
        if closed.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let changes = if remaining.is_zero() {
            store.changes(since)?
        } else {
            store.wait(since, remaining.min(WAIT_SLICE))?
        };
        if changes.index > since || remaining.is_zero() {
            return Ok(Some(changes));
        }
    }
}

fn error_frame(code: LinkErrorCode, message: &str) -> Frame {
    Frame::Error(LinkError::new(code, message))
}
