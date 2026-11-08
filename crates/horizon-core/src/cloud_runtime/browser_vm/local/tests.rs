//! The local browser VM against a fake program: Docker is a script that records what Horizon
//! asks it and answers from a small state directory.

use std::io::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use super::{DEFAULT_IMAGE, DEFAULT_NAME, LocalBrowserVm};

/// A listener on the loopback that answers like a VNC server, or that answers nothing.
fn vnc_listener(greeting: Option<&'static [u8]>) -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("an address").port();
    let handle = std::thread::spawn(move || {
        if let (Ok((mut stream, _)), Some(greeting)) = (listener.accept(), greeting) {
            let _ = stream.write_all(greeting);
        }
    });
    (port, handle)
}

const FAKE_DOCKER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
state="__STATE__"
printf '%s\n' "$*" >>"$state/calls"
case "${1:-}" in
    container)
        case "${2:-}" in
            inspect)
                if [[ ! -f "$state/container" ]]; then
                    printf 'Error: No such object: %s\n' "${3:-}" >&2
                    exit 1
                fi
                if [[ "${3:-}" == "--format" ]]; then
                    case "${4:-}" in
                        *Config.Image*) cat "$state/image-of-container" ;;
                        *) cat "$state/running" ;;
                    esac
                fi
                ;;
            *) exit 1 ;;
        esac
        ;;
    image)
        [[ "${2:-}" == "inspect" ]] || exit 1
        if [[ ! -f "$state/image" ]]; then
            printf 'Error: No such image: %s\n' "${3:-}" >&2
            exit 1
        fi
        ;;
    exec)
        [[ -f "$state/host-key" ]] || exit 1
        cat "$state/host-key"
        ;;
    build) ;;
    run)
        : >"$state/container"
        printf 'true\n' >"$state/running"
        ;;
    stop|rm)
        rm -f "$state/container"
        ;;
    *) ;;
esac
"#;

struct Fixture {
    root: tempfile::TempDir,
    home: tempfile::TempDir,
    state: PathBuf,
    docker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("root");
        let home = tempfile::tempdir().expect("home");
        let state = root.path().join("fake-state");
        std::fs::create_dir_all(&state).expect("state");
        let docker = write_program(
            root.path(),
            "docker",
            &FAKE_DOCKER.replace("__STATE__", &state.to_string_lossy()),
        );

        Self {
            root,
            home,
            state,
            docker,
        }
    }

    fn vm(&self) -> LocalBrowserVm {
        LocalBrowserVm::new(&self.root.path().join("browser-vm"), Some(self.home.path())).with_programs(&self.docker)
    }

    /// The same container, addressed on a VNC port that greets.
    fn greeting_vm(&self, ssh_port: u16) -> LocalBrowserVm {
        self.vm().with_ports(Self::greeting(), ssh_port)
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.state.join("calls")).unwrap_or_default()
    }

    /// A VNC listener that answers every connection of the run, and its loopback port.
    fn greeting() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.write_all(b"RFB 003.008\n");
            }
        });
        port
    }

    fn start_ssh_server(&self) {
        std::fs::write(
            self.state.join("host-key"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeHostKeyOfTheTest\n",
        )
        .expect("host key");
    }

    fn mark_image(&self) {
        std::fs::write(self.state.join("image"), "").expect("image");
    }

    fn mark_container(&self, running: bool) {
        std::fs::write(self.state.join("container"), "").expect("container");
        std::fs::write(self.state.join("image-of-container"), format!("{DEFAULT_IMAGE}\n")).expect("image");
        std::fs::write(self.state.join("running"), if running { "true\n" } else { "false\n" }).expect("running");
    }
}

fn write_program(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, body).expect("program");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
    }
    path
}

#[test]
fn a_start_publishes_both_ports_on_the_loopback_only() {
    let fixture = Fixture::new();
    fixture.mark_image();
    fixture.start_ssh_server();
    let route = fixture.greeting_vm(2222).up(None).expect("a started container");
    assert_eq!(route.ssh_target, "horizon-browser-vm");
    let calls = fixture.calls();
    let run = calls
        .lines()
        .find(|line| line.starts_with("run "))
        .unwrap_or_else(|| panic!("no run call in {calls}"));
    assert!(
        run.contains("--publish 127.0.0.1:5900:5900") || run.contains("--publish 127.0.0.1:"),
        "{run}"
    );
    assert!(run.contains(":5900"), "the VNC port maps to the container port: {run}");
    assert!(run.contains("--publish 127.0.0.1:2222:2222"), "{run}");
    assert!(run.contains(&format!("--volume {DEFAULT_NAME}-state:/state")), "{run}");
    assert!(run.contains("--shm-size 1g"), "{run}");
    assert!(run.contains("--env HORIZON_BROWSER_VM_SSH=1"), "{run}");
    assert!(
        run.contains("--env HORIZON_BROWSER_VM_SSH_PUBKEY=ssh-ed25519 "),
        "{run}"
    );
    assert!(run.ends_with(DEFAULT_IMAGE), "{run}");
    assert!(!run.contains("0.0.0.0"), "{run}");
}

#[test]
fn a_start_writes_the_ssh_entry_with_the_port_and_the_key_of_the_container() {
    let fixture = Fixture::new();
    fixture.mark_image();
    fixture.start_ssh_server();
    fixture.greeting_vm(2222).up(None).expect("a started container");
    let config = std::fs::read_to_string(fixture.home.path().join(".ssh").join("config")).expect("config");
    assert!(config.contains("Host horizon-browser-vm\n"), "{config}");
    assert!(config.contains("  HostName 127.0.0.1\n"), "{config}");
    assert!(config.contains("  Port 2222\n"), "{config}");
    assert!(config.contains("  IdentitiesOnly yes\n"), "{config}");
    assert!(config.contains("  HostKeyAlias horizon-browser-vm\n"), "{config}");
    let known = std::fs::read_to_string(fixture.root.path().join("browser-vm").join("browser-vm-known-hosts"))
        .expect("host keys");
    // The key comes from the container itself, and it is keyed by the alias that the block
    // names in `HostKeyAlias`: no scan of the network and no lookup by address stand between
    // that key and the entry that trusts it.
    assert!(
        known.contains("horizon-browser-vm ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeHostKeyOfTheTest"),
        "{known}"
    );
    assert!(
        !fixture.calls().lines().any(|line| line.contains("keyscan")),
        "the start scans nothing: {}",
        fixture.calls()
    );
}

#[test]
fn the_key_of_the_container_is_private_and_made_once() {
    let fixture = Fixture::new();
    fixture.mark_image();
    fixture.start_ssh_server();
    let vm = fixture.greeting_vm(2222);
    vm.up(None).expect("a started container");
    let key = fixture.root.path().join("browser-vm/browser-vm-local/id_ed25519");
    assert!(key.exists(), "the key exists");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&key).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
    let before = std::fs::read(&key).expect("key");
    std::fs::remove_file(fixture.state.join("container")).expect("no container");
    vm.up(None).expect("a second start");
    assert_eq!(
        std::fs::read(&key).expect("key"),
        before,
        "a second start keeps the key"
    );
}

#[test]
fn a_start_refuses_a_container_that_exists_and_an_image_that_is_absent() {
    let fixture = Fixture::new();
    let vm = fixture.vm();
    fixture.mark_container(true);
    let error = vm.up(None).expect_err("the container exists");
    assert!(error.to_string().contains("stop it"), "{error}");
    std::fs::remove_file(fixture.state.join("container")).expect("no container");
    let error = vm.up(None).expect_err("the image is absent");
    assert!(error.to_string().contains("--build"), "{error}");
}

#[test]
fn a_stop_keeps_the_state_volume_and_the_key() {
    let fixture = Fixture::new();
    fixture.mark_container(true);
    fixture.vm().down().expect("a stopped container");
    let calls = fixture.calls();
    assert!(
        calls.lines().any(|line| line == format!("stop {DEFAULT_NAME}")),
        "{calls}"
    );
    assert!(
        calls.lines().any(|line| line == format!("rm {DEFAULT_NAME}")),
        "{calls}"
    );
    assert!(
        !calls.lines().any(|line| line.starts_with("volume")),
        "a stop must keep the volume that holds the sign-ins: {calls}"
    );
}

#[test]
fn a_docker_that_cannot_answer_is_an_error_and_not_an_absent_container() {
    let fixture = Fixture::new();
    let down = write_program(
        fixture.root.path(),
        "docker-down",
        "#!/usr/bin/env bash\nprintf 'Cannot connect to the Docker daemon at unix:///var/run/docker.sock\n' >&2\nexit 1\n",
    );
    let vm =
        LocalBrowserVm::new(&fixture.root.path().join("browser-vm"), Some(fixture.home.path())).with_programs(&down);
    let error = vm.down().expect_err("a stop cannot report success without a daemon");
    assert!(
        error.to_string().contains("Cannot connect to the Docker daemon"),
        "{error}"
    );
    let error = vm
        .status()
        .expect_err("a status cannot report a stopped container without a daemon");
    assert!(
        error.to_string().contains("Cannot connect to the Docker daemon"),
        "{error}"
    );
}

#[test]
fn a_status_names_the_loopback_addresses_and_the_volume() {
    let fixture = Fixture::new();
    fixture.mark_container(true);
    let status = fixture.vm().status().expect("a status");
    assert!(status.running);
    assert_eq!(status.vnc_address, "127.0.0.1:5900");
    assert_eq!(status.ssh_address, "127.0.0.1:2222");
    assert_eq!(status.volume, format!("{DEFAULT_NAME}-state"));
    assert_eq!(
        status.image, DEFAULT_IMAGE,
        "the status names the image of the running container"
    );
    std::fs::remove_file(fixture.state.join("container")).expect("no container");
    assert!(!fixture.vm().status().expect("a status").running);
}

#[test]
fn a_start_reports_a_route_only_when_the_vnc_server_greets() {
    let fixture = Fixture::new();
    fixture.mark_image();
    fixture.start_ssh_server();
    let (port, server) = vnc_listener(Some(b"RFB 003.008\n"));
    fixture
        .vm()
        .with_ports(port, 2292)
        .up(None)
        .expect("a VNC server that greets");
    server.join().expect("the listener");

    // A second container of its own, because the first one still exists.
    let quiet_fixture = Fixture::new();
    quiet_fixture.mark_image();
    quiet_fixture.start_ssh_server();
    let (quiet, server) = vnc_listener(None);
    let error = quiet_fixture
        .vm()
        .with_ports(quiet, 2293)
        .up(None)
        .expect_err("a VNC server that greets nothing");
    server.join().expect("the listener");
    assert!(error.to_string().contains("no VNC greeting"), "{error}");
}

#[test]
fn a_second_name_and_other_ports_keep_their_own_volume_and_publish() {
    let fixture = Fixture::new();
    fixture.mark_image();
    fixture.start_ssh_server();
    let vnc = Fixture::greeting();
    let vm = fixture.vm().named("probe-local").with_ports(vnc, 2291);
    assert_eq!(vm.volume(), "probe-local-state");
    vm.up(None).expect("a started container");
    let calls = fixture.calls();
    assert!(calls.contains(&format!("--publish 127.0.0.1:{vnc}:5900")), "{calls}");
    assert!(calls.contains("--publish 127.0.0.1:2291:2222"), "{calls}");
    assert!(calls.contains("--volume probe-local-state:/state"), "{calls}");
}
