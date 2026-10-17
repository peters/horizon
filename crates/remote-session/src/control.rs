use std::{
    net::{Shutdown, TcpStream},
    sync::{Arc, Mutex},
};

use tokio::sync::watch;

use crate::Error;

#[derive(Default)]
struct State {
    cancelled: bool,
    socket: Option<TcpStream>,
}

pub(crate) struct Control {
    state: Mutex<State>,
    changed: watch::Sender<bool>,
}

impl Drop for Control {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(socket) = state.socket.take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

/// Clones retain cancellation authority, not an additional transport descriptor.
#[derive(Clone)]
pub struct Cancellation(pub(crate) Arc<Control>);

impl Default for Cancellation {
    fn default() -> Self {
        let (changed, _) = watch::channel(false);
        Self(Arc::new(Control {
            state: Mutex::new(State::default()),
            changed,
        }))
    }
}

impl Cancellation {
    /// Physically shut down the original TCP connection and wake blocked queue operations.
    /// Does not promise that the remote process was killed; await `Command::shutdown` for retirement.
    pub fn cancel(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.cancelled = true;
        if let Some(socket) = state.socket.take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        self.0.changed.send_replace(true);
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.0.changed.subscribe()
    }

    pub(crate) fn install(&self, socket: TcpStream) -> Result<(), Error> {
        let mut state = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.cancelled {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(Error::Cancelled);
        }
        state.socket = Some(socket);
        Ok(())
    }
}
