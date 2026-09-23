//! One `ssh -W` process whose stdio carries a single VNC connection.
use std::{
    io,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, BufReader, Join},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

use super::ViewError;

const STDERR_LINE_CHARS: usize = 200;
/// ssh reports a failed forward as a cause line followed by a summary line.
const STDERR_TAIL_LINES: usize = 2;
/// How long to wait for ssh's diagnostic after its pipe closed.
const STDERR_FLUSH_GRACE: Duration = Duration::from_millis(300);

pub(super) struct SshTunnel {
    // Killed on drop, so the tunnel never outlives the viewer connection.
    child: Child,
    stream: Option<Join<ChildStdout, ChildStdin>>,
    stderr_tail: Arc<Mutex<Vec<String>>>,
    stderr_reader: JoinHandle<()>,
}

impl SshTunnel {
    pub(super) fn spawn(program: &str, args: &[String]) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("tunnel input unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("tunnel output unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("tunnel diagnostics unavailable"))?;
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = tokio::spawn(collect_last_lines(stderr, Arc::clone(&stderr_tail)));
        Ok(Self {
            child,
            stream: Some(tokio::io::join(stdout, stdin)),
            stderr_tail,
            stderr_reader,
        })
    }

    pub(super) fn take_stream(&mut self) -> io::Result<Join<ChildStdout, ChildStdin>> {
        self.stream
            .take()
            .ok_or_else(|| io::Error::other("tunnel stream already taken"))
    }

    /// Attach ssh's last diagnostic lines, which name the real failure when
    /// the forwarded stream simply ended.
    pub(super) async fn explain(&mut self, error: ViewError) -> ViewError {
        let _ = tokio::time::timeout(STDERR_FLUSH_GRACE, &mut self.stderr_reader).await;
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("; ");
        tracing::debug!(pid = ?self.child.id(), %error, ssh = %tail, "device tunnel ended");
        if tail.is_empty() {
            error
        } else if tail.contains("Host key verification failed") {
            ViewError::Tunnel(format!(
                "{error}; ssh: {tail} (the tunnel never trusts a first-contact key; open the host over SSH once to trust it)"
            ))
        } else if tail.starts_with("ssh: ") {
            ViewError::Tunnel(format!("{error}; {tail}"))
        } else {
            ViewError::Tunnel(format!("{error}; ssh: {tail}"))
        }
    }
}

async fn collect_last_lines(stderr: ChildStderr, tail: Arc<Mutex<Vec<String>>>) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut tail = tail.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if tail.len() == STDERR_TAIL_LINES {
            tail.remove(0);
        }
        tail.push(line.chars().take(STDERR_LINE_CHARS).collect());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn tunnel_relays_bytes_through_the_program_stdio_and_reaps_it_on_drop() {
        runtime().block_on(async {
            let mut tunnel = SshTunnel::spawn("cat", &[]).unwrap();
            let mut stream = tunnel.take_stream().unwrap();
            assert!(tunnel.take_stream().is_err(), "one connection per tunnel");
            stream.write_all(b"RFB 003.008\n").await.unwrap();
            let mut echo = [0; 12];
            stream.read_exact(&mut echo).await.unwrap();
            assert_eq!(&echo, b"RFB 003.008\n");
            let pid = tunnel.child.id().expect("running child");
            drop(tunnel);
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::path::Path::new(&format!("/proc/{pid}")).exists()
                && std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| !stat.contains(" Z "))
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "tunnel process outlived the session"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    }

    #[test]
    fn a_failed_tunnel_reports_the_cause_and_summary_lines() {
        runtime().block_on(async {
            let script = "echo ignored >&2; echo >&2; \
                echo 'channel 0: open failed: connect failed: Connection refused' >&2; \
                echo 'stdio forwarding failed' >&2; exit 255";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), script.to_string()]).unwrap();
            let mut stream = tunnel.take_stream().unwrap();
            assert_eq!(stream.read(&mut [0; 1]).await.unwrap(), 0, "the forward ended");
            let error = tunnel.explain(ViewError::Frame("early end of stream")).await;
            assert_eq!(
                error.to_string(),
                "early end of stream; ssh: channel 0: open failed: connect failed: Connection refused; \
                 stdio forwarding failed"
            );
        });
    }

    #[test]
    fn ssh_prefixed_diagnostics_are_not_prefixed_twice() {
        runtime().block_on(async {
            let script = "echo 'ssh: Could not resolve hostname nowhere.invalid: Name or service not known' >&2";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), script.to_string()]).unwrap();
            let error = tunnel.explain(ViewError::Timeout).await;
            assert_eq!(
                error.to_string(),
                "connection timed out; ssh: Could not resolve hostname nowhere.invalid: Name or service not known"
            );
        });
    }

    #[test]
    fn an_unknown_host_key_explains_how_to_trust_it() {
        runtime().block_on(async {
            let script = "echo 'Host key verification failed.' >&2; exit 255";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), script.to_string()]).unwrap();
            let error = tunnel.explain(ViewError::Frame("early end of stream")).await;
            let text = error.to_string();
            assert!(
                text.starts_with("early end of stream; ssh: Host key verification failed."),
                "{text}"
            );
            assert!(text.contains("open the host over SSH once"), "{text}");
        });
    }

    #[test]
    fn a_silent_failure_keeps_the_original_error() {
        runtime().block_on(async {
            let mut tunnel = SshTunnel::spawn("true", &[]).unwrap();
            let error = tunnel.explain(ViewError::Timeout).await;
            assert!(matches!(error, ViewError::Timeout));
        });
    }
}
