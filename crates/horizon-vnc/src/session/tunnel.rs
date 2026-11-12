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

use super::host_key::{HostKeyFailure, TunnelHost};
use crate::Error;

const STDERR_LINE_CHARS: usize = 200;
/// ssh reports a failed forward as a cause line followed by a summary line.
const STDERR_TAIL_LINES: usize = 2;
/// How long to wait for ssh's diagnostic after its pipe closed.
const STDERR_FLUSH_GRACE: Duration = Duration::from_millis(300);
/// A killed ssh exits at once; this only bounds a wedged one.
const REAP_TIMEOUT: Duration = Duration::from_secs(2);
const REAP_POLL: Duration = Duration::from_millis(10);

pub(super) struct SshTunnel {
    // Killed and reaped on drop, so the tunnel never outlives the viewer
    // connection and never lingers as a zombie.
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

    /// Kill the process and wait for it, so no zombie survives the session's
    /// runtime. Runs synchronously because drop also happens when a cancelled
    /// connection future is torn down.
    fn kill_and_reap(&mut self) {
        let _ = self.child.start_kill();
        let deadline = std::time::Instant::now() + REAP_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(REAP_POLL),
                Ok(None) => {
                    tracing::warn!(pid = ?self.child.id(), "device tunnel did not exit after kill");
                    return;
                }
            }
        }
    }

    pub(super) fn take_stream(&mut self) -> io::Result<Join<ChildStdout, ChildStdin>> {
        self.stream
            .take()
            .ok_or_else(|| io::Error::other("tunnel stream already taken"))
    }

    /// Attach ssh's last diagnostic lines, which name the real failure when
    /// the forwarded stream simply ended, and type a refused host key so that
    /// "a person must trust this host" is not read as "the host is down".
    pub(super) async fn explain(&mut self, error: Error, host: Option<&TunnelHost>) -> Error {
        let _ = tokio::time::timeout(STDERR_FLUSH_GRACE, &mut self.stderr_reader).await;
        let tail = self
            .stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("; ");
        tracing::debug!(pid = ?self.child.id(), %error, ssh = %tail, "device tunnel ended");
        let host_key = match host {
            Some(host) => HostKeyFailure::detect(&tail, host).await,
            None => None,
        };
        if tail.is_empty() {
            return error;
        }
        let message = failure_message(&error, &tail, host_key.as_ref());
        Error::Tunnel {
            message,
            host_key: host_key.map(Box::new),
        }
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

/// Why a viewer lost its desktop, in the terms a person reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Cause {
    /// The link or the server did not answer in time.
    TimedOut,
    /// The computer that runs the desktop refused the connection.
    Refused,
    /// The desktop or the stream that carried it ended the connection.
    Closed,
    /// ssh does not trust the identity of the computer that runs the desktop.
    Untrusted,
}

impl Cause {
    /// One plain sentence for the cause. The words of ssh follow it, and the
    /// words of the protocol library stay in the log.
    const fn sentence(self) -> &'static str {
        match self {
            Self::TimedOut => "The desktop link timed out",
            Self::Refused => "The computer that runs the desktop refused the connection",
            Self::Closed => "The desktop closed the connection",
            Self::Untrusted => "The identity of the computer that runs the desktop is not trusted yet",
        }
    }
}

/// Classify the failure from the typed error and from ssh's own lines, which
/// name a refusal or a timeout when the forwarded stream simply ended.
fn cause(error: &Error, tail: &str, host_key: Option<&HostKeyFailure>) -> Cause {
    if host_key.is_some() || tail.contains("Host key verification failed") || tail.contains("host key is known") {
        return Cause::Untrusted;
    }
    let ssh_says_refused = tail.contains("Connection refused") || tail.contains("Permission denied");
    let ssh_says_timeout = tail.contains("timed out") || tail.contains("Timeout") || tail.contains("not responding");
    match error {
        Error::Timeout => Cause::TimedOut,
        Error::Closed => {
            if ssh_says_refused {
                Cause::Refused
            } else if ssh_says_timeout {
                Cause::TimedOut
            } else {
                Cause::Closed
            }
        }
        Error::Tunnel { .. } => {
            if ssh_says_refused {
                Cause::Refused
            } else if ssh_says_timeout {
                Cause::TimedOut
            } else {
                Cause::Closed
            }
        }
        _ => {
            if ssh_says_refused {
                Cause::Refused
            } else if ssh_says_timeout {
                Cause::TimedOut
            } else {
                Cause::Closed
            }
        }
    }
}

/// What a viewer shows for a failed tunnel: one plain sentence for the cause,
/// ssh's own lines and, when ssh refused the host key, the person's next step.
fn failure_message(error: &Error, tail: &str, host_key: Option<&HostKeyFailure>) -> String {
    let ssh = if tail.starts_with("ssh: ") {
        tail.to_owned()
    } else {
        format!("ssh: {tail}")
    };
    let sentence = cause(error, tail, host_key).sentence();
    tracing::debug!(%error, "device tunnel failure, with the words of the protocol");
    match host_key {
        Some(host_key) => format!("{sentence}; {ssh} ({})", host_key.advice()),
        None => format!("{sentence}; {ssh}"),
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;

    #[test]
    fn a_lost_link_names_its_cause_and_keeps_the_words_of_ssh() {
        let closed = Error::Closed;
        assert_eq!(
            failure_message(&closed, "Timeout, server lab not responding", None),
            "The desktop link timed out; ssh: Timeout, server lab not responding",
            "a person reads a cause, and the words of ssh follow it"
        );
        assert_eq!(
            failure_message(&closed, "connect to host lab port 22: Connection refused", None),
            "The computer that runs the desktop refused the connection; \
             ssh: connect to host lab port 22: Connection refused"
        );
        assert_eq!(
            failure_message(&closed, "Connection reset by peer", None),
            "The desktop closed the connection; ssh: Connection reset by peer"
        );
        assert!(
            !failure_message(&closed, "Timeout, server lab not responding", None).contains("VNC client"),
            "the words of the protocol library stay in the log"
        );
    }

    #[test]
    fn a_library_stop_is_a_closed_connection_and_not_a_sentence_of_the_library() {
        let error: Error = vnc::VncError::ClientNotRunning.into();
        assert!(matches!(error, Error::Closed));
        assert_eq!(error.to_string(), "The desktop closed the connection");
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
    use crate::HostKeyRefusal;

    /// Whether the kernel still knows the pid, zombie or not. `kill -0` is
    /// portable across Unix targets, unlike `/proc`, and a zombie still
    /// answers it, so only a reaped child makes this false.
    fn process_exists(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

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
            assert!(process_exists(pid), "the child is running before the drop");
            drop(tunnel);
            // Killed and reaped synchronously on drop: not merely a zombie.
            assert!(!process_exists(pid), "tunnel process outlived the session");
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
            let error = tunnel.explain(Error::Frame("early end of stream"), None).await;
            assert_eq!(
                error.to_string(),
                "The computer that runs the desktop refused the connection; ssh: channel 0: open failed: \
                 connect failed: Connection refused; stdio forwarding failed"
            );
        });
    }

    #[test]
    fn ssh_prefixed_diagnostics_are_not_prefixed_twice() {
        runtime().block_on(async {
            let script = "echo 'ssh: Could not resolve hostname nowhere.invalid: Name or service not known' >&2";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), script.to_string()]).unwrap();
            let error = tunnel.explain(Error::Timeout, None).await;
            assert_eq!(
                error.to_string(),
                "The desktop link timed out; ssh: Could not resolve hostname nowhere.invalid: Name or service not known"
            );
        });
    }

    #[test]
    fn a_tunnel_dropped_inside_a_cancelled_future_is_still_reaped() {
        let runtime = runtime();
        let pid = runtime.block_on(async {
            let tunnel = SshTunnel::spawn("cat", &[]).unwrap();
            let pid = tunnel.child.id().expect("running child");
            let (cancel, cancelled) = tokio::sync::oneshot::channel::<()>();
            let hold = async move {
                let _tunnel = tunnel;
                std::future::pending::<()>().await;
            };
            cancel.send(()).unwrap();
            tokio::select! {
                _ = cancelled => {}
                () = hold => {}
            }
            pid
        });
        drop(runtime);
        assert!(!process_exists(pid), "cancelled tunnel left a process behind");
    }

    /// A host whose ssh program cannot reach a probe: its directory holds no
    /// `ssh-keyscan`, so no test contacts a network.
    fn unreachable_host() -> TunnelHost {
        TunnelHost {
            program: "/nonexistent/horizon-vnc-test/ssh".into(),
            host: "lab".into(),
            port: Some(22),
            args: vec!["-W".to_string(), "127.0.0.1:5900".to_string(), "lab".to_string()],
        }
    }

    #[test]
    fn an_unknown_host_key_is_typed_but_a_changed_key_is_a_verification_failure() {
        runtime().block_on(async {
            let unknown = "echo 'No ED25519 host key is known for [lab]:22 and you have requested strict checking.' >&2; \
                echo 'Host key verification failed.' >&2; exit 255";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), unknown.to_string()]).unwrap();
            let error = tunnel.explain(Error::Frame("early end of stream"), Some(&unreachable_host())).await;
            let text = error.to_string();
            assert!(
                text.starts_with(
                    "The identity of the computer that runs the desktop is not trusted yet; \
                     ssh: No ED25519 host key is known"
                ),
                "{text}"
            );
            assert!(text.contains("[lab]:22 is not in known_hosts"), "{text}");
            assert!(text.contains("standard ssh tooling"), "{text}");
            assert_eq!(
                error.host_key_failure().map(|failure| failure.refusal),
                Some(HostKeyRefusal::NotTrusted)
            );

            let changed = "echo 'WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!' >&2; \
                echo 'Host key verification failed.' >&2; exit 255";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), changed.to_string()]).unwrap();
            let error = tunnel.explain(Error::Frame("early end of stream"), Some(&unreachable_host())).await;
            let text = error.to_string();
            assert!(text.contains("Host key verification failed."), "{text}");
            assert!(text.contains("verification failure"), "{text}");
            assert!(!text.contains("adds the key"), "a changed key must not invite a bypass: {text}");
            assert_eq!(
                error.host_key_failure().map(|failure| failure.refusal),
                Some(HostKeyRefusal::Changed)
            );
        });
    }

    #[test]
    fn a_bare_verification_failure_stays_untyped() {
        runtime().block_on(async {
            let script = "echo 'Host key verification failed.' >&2; exit 255";
            let mut tunnel = SshTunnel::spawn("sh", &["-c".to_string(), script.to_string()]).unwrap();
            let error = tunnel
                .explain(Error::Frame("early end of stream"), Some(&unreachable_host()))
                .await;
            assert!(error.host_key_failure().is_none(), "{error}");
            assert_eq!(
                error.to_string(),
                "The identity of the computer that runs the desktop is not trusted yet; \
                 ssh: Host key verification failed."
            );
        });
    }

    #[test]
    fn a_silent_failure_keeps_the_original_error() {
        runtime().block_on(async {
            let mut tunnel = SshTunnel::spawn("true", &[]).unwrap();
            let error = tunnel.explain(Error::Timeout, Some(&unreachable_host())).await;
            assert!(matches!(error, Error::Timeout));
        });
    }
}
