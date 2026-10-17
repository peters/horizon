#![cfg(all(feature = "ssh", unix))]

use std::{
    fmt::Write,
    fs,
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command as Process, Stdio},
    time::Duration,
};

use remote_session::{Credentials, Error, Event, ExecRequest, Exit, HostKeyPin};
use tempfile::TempDir;
use tokio::time::{Instant, timeout};

// Opt-in: the caller supplies an actual sshd, never a user's server or saved keys.
struct Server {
    child: Child,
    root: TempDir,
    endpoint: SocketAddr,
    user: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            eprintln!(
                "fixture sshd: {}",
                fs::read_to_string(self.root.path().join("sshd.log")).unwrap_or_default()
            );
        }
    }
}

impl Server {
    fn start() -> Self {
        let sshd = PathBuf::from(
            std::env::var_os("REMOTE_SESSION_TEST_SSHD")
                .expect("set REMOTE_SESSION_TEST_SSHD to an actual OpenSSH server"),
        );
        assert!(sshd.is_absolute() && sshd.is_file());
        // OpenSSH StrictModes checks ancestors too; the shared /tmp is rejected.
        let root = tempfile::Builder::new()
            .prefix(".remote-session-test-")
            .tempdir_in(std::env::var_os("HOME").expect("fixture account HOME"))
            .expect("private fixture directory below the account home");
        let path = root.path();
        fs::create_dir(path.join("home")).unwrap();
        for name in ["host", "client", "foreign"] {
            assert!(
                Process::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                    .arg(path.join(name))
                    .stdin(Stdio::null())
                    .status()
                    .expect("generate synthetic key")
                    .success()
            );
        }
        fs::copy(path.join("client.pub"), path.join("authorized_keys")).unwrap();
        let user = std::env::var("USER").expect("fixture account USER");
        assert!(!user.is_empty());
        assert!(user.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)));
        let endpoint = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let mut config = format!(
            "Port {}\nListenAddress 127.0.0.1\nHostKey {}/host\n\
             AuthorizedKeysFile {}/authorized_keys\nPidFile {}/sshd.pid\n\
             UsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
             AuthenticationMethods publickey\nPermitRootLogin prohibit-password\n\
             AllowUsers {user}\nStrictModes yes\nDisableForwarding yes\nPermitTTY no\n\
             PermitUserRC no\nPermitUserEnvironment no\nUseDNS no\nLoginGraceTime 5\nSetEnv HOME={}/home\n",
            endpoint.port(),
            path.display(),
            path.display(),
            path.display(),
            path.display(),
        );
        for (variable, directive) in [
            ("REMOTE_SESSION_TEST_SSHD_SESSION", "SshdSessionPath"),
            ("REMOTE_SESSION_TEST_SSHD_AUTH", "SshdAuthPath"),
        ] {
            if let Some(value) = std::env::var_os(variable) {
                let helper = PathBuf::from(value);
                assert!(helper.is_absolute() && helper.is_file());
                let value = helper.to_str().expect("UTF-8 sshd helper path");
                assert!(!value.chars().any(|c| matches!(c, '\0' | '\r' | '\n')));
                let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
                writeln!(config, "{directive} \"{escaped}\"").unwrap();
            }
        }
        fs::write(path.join("sshd_config"), config).unwrap();
        let child = Process::new(sshd)
            .args(["-D", "-e", "-f"])
            .arg(path.join("sshd_config"))
            .arg("-E")
            .arg(path.join("sshd.log"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start original foreground sshd");
        let mut server = Self {
            child,
            root,
            endpoint,
            user,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while TcpStream::connect(endpoint).is_err() {
            assert!(
                server.child.try_wait().unwrap().is_none(),
                "sshd exited; inspect fixture log"
            );
            assert!(std::time::Instant::now() < deadline, "sshd readiness deadline");
            std::thread::sleep(Duration::from_millis(25));
        }
        server
    }

    fn key_path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn credentials(&self, name: &str) -> Credentials {
        Credentials::from_openssh(&fs::read_to_string(self.key_path(name)).unwrap(), None).unwrap()
    }

    fn request(&self, command: &str, host: &str, duration: Duration) -> ExecRequest {
        let pin =
            HostKeyPin::from_openssh(&fs::read_to_string(self.key_path(host).with_extension("pub")).unwrap()).unwrap();
        ExecRequest::new(self.endpoint, &self.user, command, pin, Instant::now() + duration).unwrap()
    }
}

async fn refused(server: &Server, pin: &str, identity: &str, expected: Error) {
    let mut command = remote_session::execute(
        server.request("printf MUST_NOT_EXECUTE", pin, Duration::from_secs(10)),
        server.credentials(identity),
    )
    .unwrap();
    timeout(Duration::from_secs(12), async {
        while let Some(event) = command.next_event().await {
            assert!(!matches!(
                event,
                Event::ExecAccepted | Event::Stdout(_) | Event::Stderr(_)
            ));
        }
        assert_eq!(command.completion().await, Err(expected));
    })
    .await
    .expect("refusal must retire");
}

#[tokio::test]
#[ignore = "requires a genuine local sshd via REMOTE_SESSION_TEST_SSHD"]
async fn wrong_host_key_never_executes() {
    let server = Server::start();
    refused(&server, "foreign", "client", Error::HostKeyMismatch).await;
}

#[tokio::test]
#[ignore = "requires a genuine sshd and REMOTE_SESSION_TEST_EXAMPLE executable"]
async fn example_connects_without_ssh_programs_on_path() {
    let server = Server::start();
    let example = PathBuf::from(std::env::var_os("REMOTE_SESSION_TEST_EXAMPLE").expect("built ssh_exec example"));
    assert!(example.is_absolute() && example.is_file());
    let empty_bin = server.key_path("empty-bin");
    fs::create_dir(&empty_bin).unwrap();
    let output = timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(example)
            .arg(server.endpoint.to_string())
            .arg(&server.user)
            .arg(server.key_path("client"))
            .arg(server.key_path("host.pub"))
            .arg("printf 'rust-client-only\\n'")
            .env_clear()
            .env("HOME", server.key_path("home"))
            .env("PATH", empty_bin)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("example deadline")
    .expect("example process");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"rust-client-only\n");
    assert!(output.stderr.is_empty());
}

#[tokio::test]
#[ignore = "requires a genuine local sshd via REMOTE_SESSION_TEST_SSHD"]
async fn unauthorized_client_key_never_executes() {
    let server = Server::start();
    refused(&server, "host", "foreign", Error::Authentication).await;
}

#[tokio::test]
#[ignore = "requires a genuine local sshd via REMOTE_SESSION_TEST_SSHD"]
async fn binary_stdin_eof_keeps_both_outputs_and_nonzero_exit() {
    let server = Server::start();
    let mut command = remote_session::execute(
        server.request(
            "cat; printf 'after-stdin-eof\\n' >&2; exit 23",
            "host",
            Duration::from_secs(10),
        ),
        server.credentials("client"),
    )
    .unwrap();
    let payload: Vec<u8> = (0_u8..=255).cycle().take(4096).collect();
    let mut input = command.take_input().unwrap();
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut accepted, mut eof, mut closed, mut exits) = (0, 0, 0, Vec::new());
    timeout(Duration::from_secs(12), async {
        input.write(&payload).await.unwrap();
        input.eof().await.unwrap();
        while let Some(event) = command.next_event().await {
            match event {
                Event::ExecAccepted => accepted += 1,
                Event::Stdout(bytes) => stdout.extend(bytes),
                Event::Stderr(bytes) => stderr.extend(bytes),
                Event::RemoteEof => eof += 1,
                Event::Exit(exit) => exits.push(exit),
                Event::RemoteClose => closed += 1,
            }
        }
        let completion = command.completion().await.unwrap();
        assert_eq!(completion.exit, Exit::Status(23));
        assert!(completion.remote_eof);
    })
    .await
    .expect("command must retire");
    assert_eq!(stdout, payload);
    assert_eq!(stderr, b"after-stdin-eof\n");
    assert_eq!((accepted, eof, closed), (1, 1, 1));
    assert_eq!(exits, [Exit::Status(23)]);
}

#[tokio::test]
#[ignore = "requires a genuine local sshd via REMOTE_SESSION_TEST_SSHD"]
async fn cancellation_retires_a_command_waiting_for_input() {
    let server = Server::start();
    let mut command = remote_session::execute(
        server.request("cat >/dev/null", "host", Duration::from_secs(10)),
        server.credentials("client"),
    )
    .unwrap();
    let mut input = command.take_input().unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            match command.next_event().await {
                Some(Event::ExecAccepted) => break,
                Some(_) => {}
                None => panic!("command ended before exec acceptance"),
            }
        }
    })
    .await
    .expect("exec acceptance deadline");
    command.cancellation().cancel();
    assert_eq!(
        timeout(Duration::from_secs(3), command.shutdown()).await.unwrap(),
        Err(Error::Cancelled)
    );
    assert_eq!(input.write(b"late").await, Err(Error::Cancelled));
}

#[tokio::test]
#[ignore = "requires a genuine local sshd via REMOTE_SESSION_TEST_SSHD"]
async fn deadline_retires_a_command_waiting_for_input() {
    let server = Server::start();
    let mut command = remote_session::execute(
        server.request("cat >/dev/null", "host", Duration::from_secs(5)),
        server.credentials("client"),
    )
    .unwrap();
    let input = command.take_input().unwrap();
    let mut accepted = false;
    timeout(Duration::from_secs(8), async {
        while let Some(event) = command.next_event().await {
            accepted |= matches!(event, Event::ExecAccepted);
        }
        assert_eq!(command.completion().await, Err(Error::Deadline));
    })
    .await
    .expect("deadline must retire");
    assert!(accepted, "exercise a running command, not a connect timeout");
    drop(input);
}
