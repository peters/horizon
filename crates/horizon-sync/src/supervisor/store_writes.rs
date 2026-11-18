//! The writes of the settings store of one computer, on one worker and in one order.
//!
//! An answer of the server gives the mirror of this computer a row before the store
//! holds it, and one write of that store is a file sync: 29 to 76 ms on a busy
//! computer, and 1.5 to 2.4 s for the twelve sections of one link under load. A link
//! that wrote the store itself answered no frame while it ran, so a save of the person
//! in that window named the version of the row before. The link hands the rows to this
//! worker and takes the next frame; the worker writes them in the order of the answers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::SyncHandle;
use super::settings;

/// How long the worker waits for the next row before it reads the stop flag of the
/// supervisor again.
const SLICE: Duration = Duration::from_millis(250);

/// Starts the worker that writes the settings store of `handle`, and answers its thread.
///
/// # Errors
/// Returns why the thread cannot start.
pub(super) fn start(handle: &SyncHandle, stop: &Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>> {
    let Some(requests) = handle.take_store_requests() else {
        return thread::Builder::new().spawn(|| {});
    };
    let handle = handle.clone();
    let stop = Arc::clone(stop);
    thread::Builder::new()
        .name("horizon-sync-settings".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match requests.recv_timeout(SLICE) {
                    Ok(()) => {}
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                // Every answer that arrived while the last write ran is one write: the
                // state that this call reads holds the rows of them all.
                while requests.try_recv().is_ok() {}
                settings::apply(&handle);
            }
        })
}
