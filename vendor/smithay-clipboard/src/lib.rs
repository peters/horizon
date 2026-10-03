//! Smithay Clipboard
//!
//! Provides access to the Wayland clipboard for gui applications. The user
//! should have surface around.

#![cfg(target_os = "linux")]
#![deny(unsafe_code)]
// Preserve upstream assertion policy; the native extension has stricter lints.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![deny(clippy::all, clippy::if_not_else, clippy::enum_glob_use)]
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Result;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use sctk::reexports::calloop::channel::{self, Sender};
use sctk::reexports::client::Connection;
use sctk::reexports::client::backend::Backend;

mod mime;
pub mod native;
mod state;
mod worker;

/// Access to a Wayland clipboard.
pub struct Clipboard {
    worker: Arc<ClipboardWorker>,
}

// Every viewport uses the same Wayland connection. Multiple devices on it can
// compete for compositor offers, so their clipboard handles share one worker.
struct ClipboardWorker {
    request_sender: Sender<worker::Command>,
    request_receiver: Mutex<Receiver<Result<String>>>,
    native: Arc<native::Hub>,
    clipboard_thread: Option<std::thread::JoinHandle<()>>,
}

type Workers = HashMap<usize, Weak<ClipboardWorker>>;
static WORKERS: OnceLock<Mutex<Workers>> = OnceLock::new();

impl Clipboard {
    /// Creates new clipboard which will be running on its own thread with its
    /// own event queue to handle clipboard requests.
    ///
    /// # Safety
    ///
    /// `display` must be a valid `*mut wl_display` pointer, and it must remain
    /// valid for as long as `Clipboard` object is alive.
    #[allow(unsafe_code)] // Upstream foreign-display adoption boundary.
    pub unsafe fn new(display: *mut c_void) -> Self {
        let mut workers = WORKERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        workers.retain(|_, worker| worker.strong_count() > 0);
        if let Some(worker) = workers.get(&(display as usize)).and_then(Weak::upgrade)
            && worker.native.is_alive()
        {
            return Self { worker };
        }
        // SAFETY: the caller guarantees the foreign display lifetime above.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let connection = Connection::from_backend(backend);

        // Create channel to send data to clipboard thread.
        let (request_sender, rx_chan) = channel::channel();
        // Create channel to get data from the clipboard thread.
        let (clipboard_reply_sender, request_receiver) = mpsc::channel();

        let name = String::from("smithay-clipboard");
        let native = native::Hub::register(display as usize, request_sender.clone());
        let clipboard_thread = worker::spawn(name, connection, rx_chan, clipboard_reply_sender, native.clone());

        let worker = Arc::new(ClipboardWorker {
            request_receiver: Mutex::new(request_receiver),
            request_sender,
            clipboard_thread,
            native,
        });
        if worker.clipboard_thread.is_none() {
            worker.native.stop();
        }
        workers.insert(display as usize, Arc::downgrade(&worker));
        Self { worker }
    }

    /// Load clipboard data.
    ///
    /// Loads content from a clipboard on a last observed seat.
    ///
    /// # Errors
    /// Returns an error when the worker or selection transfer fails.
    pub fn load(&self) -> Result<String> {
        self.worker.load(worker::Command::Load)
    }

    /// Store to a clipboard.
    ///
    /// Stores to a clipboard on a last observed seat.
    pub fn store<T: Into<String>>(&self, text: T) {
        let request = worker::Command::Store(text.into());
        let _ = self.worker.request_sender.send(request);
    }

    /// Load primary clipboard data.
    ///
    /// Loads content from a  primary clipboard on a last observed seat.
    ///
    /// # Errors
    /// Returns an error when the worker or primary selection transfer fails.
    pub fn load_primary(&self) -> Result<String> {
        self.worker.load(worker::Command::LoadPrimary)
    }

    /// Store to a primary clipboard.
    ///
    /// Stores to a primary clipboard on a last observed seat.
    pub fn store_primary<T: Into<String>>(&self, text: T) {
        let request = worker::Command::StorePrimary(text.into());
        let _ = self.worker.request_sender.send(request);
    }
}

impl ClipboardWorker {
    fn load(&self, command: worker::Command) -> Result<String> {
        // Serialize text loads so concurrent viewport requests cannot exchange
        // their replies. Native image requests have a separate event queue.
        let receiver = self
            .request_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.request_sender
            .send(command)
            .map_err(|_| std::io::Error::other("clipboard is dead."))?;
        receiver
            .recv()
            .unwrap_or_else(|_| Err(std::io::Error::other("clipboard is dead.")))
    }
}

impl Drop for ClipboardWorker {
    fn drop(&mut self) {
        // Shutdown smithay-clipboard.
        let _ = self.request_sender.send(worker::Command::Exit);
        if let Some(clipboard_thread) = self.clipboard_thread.take() {
            let _ = clipboard_thread.join();
        }
    }
}
