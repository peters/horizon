use crate::{Error, Result, session::lock};
use std::{
    net::{Shutdown, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) struct Cancellation {
    stopped: Arc<AtomicBool>,
    socket: Mutex<Option<TcpStream>>,
}
impl Cancellation {
    pub(crate) fn new(stopped: Arc<AtomicBool>) -> Self {
        Self {
            stopped,
            socket: Mutex::new(None),
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if self.stopped.load(Ordering::Relaxed) {
            Err(Error::Protocol("casting cancelled"))
        } else {
            Ok(())
        }
    }
    pub(crate) fn register(&self, socket: &TcpStream) -> Result<()> {
        let mut registered = lock(&self.socket);
        self.check()?;
        *registered = Some(socket.try_clone()?);
        Ok(())
    }
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(socket) = lock(&self.socket).as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    pub(crate) fn release(&self) -> Result<()> {
        let mut registered = lock(&self.socket);
        self.check()?;
        *registered = None;
        Ok(())
    }
    pub(crate) fn clear(&self) {
        *lock(&self.socket) = None;
    }
}
