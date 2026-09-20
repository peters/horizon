//! An owned OpenSSH tunnel for the native read-only desktop viewer.
use super::{Cancellation, Error, Result, ssh::Connection};
use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Stdio},
    sync::Mutex,
    time::Duration,
};
#[derive(Debug)]
pub struct DesktopTunnel {
    pub endpoint: SocketAddr,
    child: Mutex<Child>,
}
impl DesktopTunnel {
    /// # Errors
    /// Fails if the SSH listener cannot bind or the worker desktop is unavailable.
    pub fn open(connection: &Connection, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = listener.local_addr()?;
        let mut args = connection.args();
        args.splice(
            0..0,
            [
                "-N".into(),
                "-o".into(),
                "ExitOnForwardFailure=yes".into(),
                "-L".into(),
                format!("{endpoint}:127.0.0.1:5900"),
            ],
        );
        drop(listener);
        let child = std::process::Command::new("ssh")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let tunnel = Self {
            endpoint,
            child: Mutex::new(child),
        };
        for _ in 0..100 {
            cancel.check()?;
            if tunnel
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .try_wait()?
                .is_some()
            {
                return Err(Error::Invalid("Desktop SSH tunnel failed"));
            }
            if let Ok(mut socket) = TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)) {
                use std::io::Read;
                // Some VNC servers wait while detecting WebSocket clients before sending RFB.
                socket.set_read_timeout(Some(Duration::from_secs(3)))?;
                let mut banner = [0; 12];
                if socket.read_exact(&mut banner).is_ok() && banner.starts_with(b"RFB ") {
                    return Ok(tunnel);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(Error::Invalid("Worker desktop did not become ready"))
    }
}
impl Drop for DesktopTunnel {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}
