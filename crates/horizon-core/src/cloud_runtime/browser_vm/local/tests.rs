//! The local browser VM against a fake program: Docker is a script that records what Horizon
//! asks it and answers from a small state directory.

use std::io::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{DEFAULT_IMAGE, DEFAULT_NAME, KEY_ENV, LocalBrowserVm};

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
state="$(cd "$(dirname "$0")" && pwd)/fake-state"
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
                        *Config.Env*) cat "$state/container-env" ;;
                        *PortBindings*)
                            printf '{'
                            first=1
                            while read -r spec; do
                                host_ip="${spec%%:*}"
                                rest="${spec#*:}"
                                published="${rest%%:*}"
                                container="${rest##*:}"
                                if [[ "$first" == "1" ]]; then first=0; else printf ','; fi
                                printf '"%s/tcp":[{"HostIp":"%s","HostPort":"%s"}]' \
                                    "$container" "$host_ip" "$published"
                            done <"$state/publish"
                            printf '}\n'
                            ;;
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
    start)
        [[ -f "$state/container" ]] || { printf 'Error: No such container: %s\n' "${2:-}" >&2; exit 1; }
        printf 'true\n' >"$state/running"
        ;;
    run)
        : >"$state/container"
        printf 'true\n' >"$state/running"
        printf '%s\n' "${@: -1}" >"$state/image-of-container"
        : >"$state/publish"
        previous=""
        for argument in "$@"; do
            if [[ "$previous" == "--publish" ]]; then printf '%s\n' "$argument" >>"$state/publish"; fi
            previous="$argument"
        done
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
        // One script for the whole test process, reached through a link in each fixture. A
        // script written beside the test can still be open for writing when another thread
        // starts it, and the kernel then refuses to run it with "Text file busy"; the shared
        // script is written once, before any test starts one.
        let docker = root.path().join("docker");
        std::os::unix::fs::symlink(shared_docker(), &docker).expect("a link to the fake docker");

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

    /// The container that the fixture runs carries the public key of this run, as the SSH
    /// server of the image reads it.
    fn carries_the_key_of(&self, vm: &LocalBrowserVm) {
        let key = vm.accepted_public_key().expect("the public key of this run");
        std::fs::write(self.state.join("container-env"), format!("{KEY_ENV}={key}\n")).expect("the key");
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

/// The fake `docker` of this test process, written once.
fn shared_docker() -> &'static Path {
    static DOCKER: OnceLock<PathBuf> = OnceLock::new();
    DOCKER.get_or_init(|| {
        let folder = tempfile::tempdir().expect("a folder for the fake docker").keep();
        write_program(&folder, "docker", FAKE_DOCKER)
    })
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
fn a_start_adopts_the_container_of_its_own_image_and_refuses_another() {
    let fixture = Fixture::new();
    fixture.mark_image();
    fixture.start_ssh_server();
    let vm = fixture.greeting_vm(2222);
    vm.up(None).expect("a first start");
    fixture.carries_the_key_of(&vm);

    // A run that broke after its container started is retried by adopting that container.
    let calls = fixture.state.join("calls");
    std::fs::remove_file(&calls).expect("a fresh log");
    let route = vm.up(None).expect("the container of the same image is adopted");
    assert_eq!(route.ssh_target, "horizon-browser-vm");
    let log = fixture.calls();
    assert!(
        !log.lines().any(|line| line.starts_with("run ")),
        "an adopted container is not started again: {log}"
    );

    // A stopped container of the same image is started again.
    std::fs::write(fixture.state.join("running"), "false\n").expect("stopped");
    vm.up(None).expect("a stopped container of the same image is started");
    assert!(
        fixture
            .calls()
            .lines()
            .any(|line| line == format!("start {DEFAULT_NAME}")),
        "{}",
        fixture.calls()
    );

    // A container that accepts the key of another Horizon is never adopted: the entry that
    // this run would write names a key that its SSH server refuses.
    std::fs::write(
        fixture.state.join("container-env"),
        format!("{KEY_ENV}=ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAnotherKeyOfAnotherHorizon\n"),
    )
    .expect("another key");
    let error = vm.up(None).expect_err("another key holds the container");
    assert!(
        error.to_string().contains("accepts a key of another Horizon"),
        "{error}"
    );
    fixture.carries_the_key_of(&vm);

    // A container that publishes other ports answers on another display, so it is refused.
    std::fs::write(
        fixture.state.join("publish"),
        "127.0.0.1:5901:5900\n127.0.0.1:2223:2222\n",
    )
    .expect("other ports");
    let error = vm.up(None).expect_err("other ports hold the container");
    assert!(error.to_string().contains("publishes the display on 5901"), "{error}");
    std::fs::write(
        fixture.state.join("publish"),
        "127.0.0.1:5900:5900\n127.0.0.1:2222:2222\n",
    )
    .expect("the ports of the run");

    // A container of another image holds the name and is never touched.
    std::fs::write(fixture.state.join("image-of-container"), "another-image:1\n").expect("image");
    let error = vm.up(None).expect_err("another image holds the name");
    assert!(error.to_string().contains("holds the image another-image:1"), "{error}");
}

#[test]
fn a_start_refuses_an_image_that_is_absent_with_no_recipe() {
    let fixture = Fixture::new();
    let error = fixture.vm().up(None).expect_err("the image is absent");
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
