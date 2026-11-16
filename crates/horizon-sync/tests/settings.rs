//! Two computers and one server, and the settings that are the same on both.
//!
//! The stand-in `ssh` is the relay of `tests/supervisor.rs`: one `sh` program that
//! runs a small Python relay against the socket of a temporary server. Each
//! computer has its own Horizon home with its own settings store, so a test reads
//! what the person would read in the Settings window.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use horizon_core::settings_sync;
use horizon_core::{AppearanceTheme, Config};
use horizon_sync::{
    Intent, KIND_SETTING, LinkState, Op, Store, Supervisor, SupervisorConfig, SyncHandle, SyncedSettings,
    run_server_on_unix_socket,
};
use serde_json::{Value, json};

/// The time a test gives a change to travel through a link and a store.
const PATIENCE: Duration = Duration::from_secs(30);

/// The time a test gives a change to travel through a link that may have ended: the
/// attempts of a lost link wait 3, 6, 12 and 24 seconds, so thirty seconds cover no
/// loss and sixty cover one of the first four attempts.
const PATIENCE_LINK: Duration = Duration::from_secs(60);

/// The relay of one stand-in `ssh`: the bytes of its standard input and output go
/// to the socket of the server in both directions.
const RELAY: &str = "\
import os
import socket
import sys
import threading

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])


def outward():
    try:
        while True:
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                break
            sock.sendall(data)
    except Exception:
        pass
    try:
        sock.shutdown(socket.SHUT_WR)
    except Exception:
        pass


threading.Thread(target=outward, daemon=True).start()
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
except Exception:
    pass
";

/// A relay that holds the first push of the settings until the test writes
/// [`RELEASE`] in the directory of the stand-in.
///
/// Two computers whose relays hold their push link at the same moment: each one
/// composes its first push on a server that holds no section, and the server
/// decides both pushes when the test releases them. The push that it decides
/// second meets the row that the first one wrote.
const GATED_RELAY: &str = "\
import os
import socket
import sys
import threading
import time

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])
directory = sys.argv[2]
held = os.path.join(directory, 'held.log')
release = os.path.join(directory, 'release')


def outward():
    buffer = b''
    try:
        while True:
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                break
            buffer += data
            while len(buffer) >= 4:
                length = int.from_bytes(buffer[:4], 'big')
                if len(buffer) < 4 + length:
                    break
                frame = buffer[:4 + length]
                buffer = buffer[4 + length:]
                if b'setting_put' in frame:
                    with open(held, 'a') as log:
                        log.write('held\\n')
                    while not os.path.exists(release):
                        time.sleep(0.01)
                sock.sendall(frame)
    except Exception:
        pass
    try:
        sock.shutdown(socket.SHUT_WR)
    except Exception:
        pass


threading.Thread(target=outward, daemon=True).start()
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
except Exception:
    pass
";

/// A relay that holds the first push of the settings until the test writes
/// [`RELEASE`], and then the request for changes that follows that push until the
/// test writes [`RELEASE_PULL`].
///
/// Between the answer of the push and the answer of that request, the version that
/// the push wrote is not in the mirror of the computer yet. The relay keeps that
/// window open, so a test can save a setting in it.
const HELD_PULL_RELAY: &str = "\
import os
import socket
import sys
import threading
import time

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(sys.argv[1])
directory = sys.argv[2]
held = os.path.join(directory, 'held.log')
release = os.path.join(directory, 'release')
pull_release = os.path.join(directory, 'release-pull')
state = {'pushed': False}


def wait_until(path):
    while not os.path.exists(path):
        time.sleep(0.01)


def outward():
    buffer = b''
    try:
        while True:
            data = os.read(sys.stdin.fileno(), 65536)
            if not data:
                break
            buffer += data
            while len(buffer) >= 4:
                length = int.from_bytes(buffer[:4], 'big')
                if len(buffer) < 4 + length:
                    break
                frame = buffer[:4 + length]
                buffer = buffer[4 + length:]
                if b'setting_put' in frame:
                    with open(held, 'a') as log:
                        log.write('push\\n')
                    wait_until(release)
                    state['pushed'] = True
                elif state['pushed'] and (b'\"pull\":' in frame or b'\"wait\":' in frame):
                    with open(held, 'a') as log:
                        log.write('request\\n')
                    wait_until(pull_release)
                sock.sendall(frame)
    except Exception:
        pass
    try:
        sock.shutdown(socket.SHUT_WR)
    except Exception:
        pass


threading.Thread(target=outward, daemon=True).start()
try:
    while True:
        data = sock.recv(65536)
        if not data:
            break
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
except Exception:
    pass
";

/// The file that holds one line for each start of the stand-in of a computer.
const STARTS: &str = "starts.log";
/// The file whose existence releases the push that a gated stand-in holds, and the
/// log of the pushes that it held.
const RELEASE: &str = "release";
/// The file whose existence releases the request for changes that a stand-in of
/// the held push holds.
const RELEASE_PULL: &str = "release-pull";
const HELD: &str = "held.log";

/// A stand-in `ssh` that runs the link relay against the socket of the server.
///
/// The stand-in writes one line to [`STARTS`] before it relays, so a test reads the
/// instant of an attempt of the link itself and not the handshake that follows
/// the start of its transport.
fn stand_in(directory: &Path, socket: &Path) -> String {
    stand_in_with(directory, socket, Relay::Plain)
}

/// A stand-in whose relay holds the first push of the settings until the test
/// writes [`RELEASE`] beside [`HELD`].
fn gated_stand_in(directory: &Path, socket: &Path) -> String {
    stand_in_with(directory, socket, Relay::Gated)
}

/// A stand-in whose relay holds the first push of the settings and then the pull
/// that follows it, each until the test writes its own file.
fn held_pull_stand_in(directory: &Path, socket: &Path) -> String {
    stand_in_with(directory, socket, Relay::HeldPull)
}

/// What a stand-in does with the frames of one link.
#[derive(Clone, Copy)]
enum Relay {
    /// Relays every frame.
    Plain,
    /// Holds the first push of the settings until [`RELEASE`].
    Gated,
    /// Holds the first push of the settings until [`RELEASE`], and the pull that
    /// follows it until [`RELEASE_PULL`].
    HeldPull,
}

fn stand_in_with(directory: &Path, socket: &Path, how: Relay) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let relay = directory.join("relay.py");
    let text = match how {
        Relay::Plain => RELAY,
        Relay::Gated => GATED_RELAY,
        Relay::HeldPull => HELD_PULL_RELAY,
    };
    fs::write(&relay, text).expect("the relay of the stand-in");
    let script = format!(
        "printf '.\\n' >> '{}'\nexec /usr/bin/env python3 '{}' '{}' '{}'",
        directory.join(STARTS).display(),
        relay.display(),
        socket.display(),
        directory.display()
    );
    let program = horizon_core::stand_in_program_in(directory, "ssh", &script).expect("the stand-in ssh");
    program.to_string_lossy().into_owned()
}

/// Waits until the stand-in of `computer` holds its push of the settings.
fn held_push(computer: &Computer) {
    eventually("the push of the settings in the stand-in", || {
        fs::read_to_string(computer.home.join("stand-in").join(HELD))
            .ok()
            .filter(|text| text.contains("push"))
            .map(|_| ())
    });
}

/// Waits until the stand-in of `computer` holds the request for changes that
/// follows that push, which is the pull or the wait that the link opens.
fn held_request(computer: &Computer) {
    eventually("the request after the push in the stand-in", || {
        fs::read_to_string(computer.home.join("stand-in").join(HELD))
            .ok()
            .filter(|text| text.contains("request"))
            .map(|_| ())
    });
}

/// Releases the request for changes that the stand-in of `computer` holds.
fn release_pull(computer: &Computer) {
    fs::write(computer.home.join("stand-in").join(RELEASE_PULL), b"release").expect("the release of the pull");
}

/// How many times the stand-in of the computer at `home` started, which is one
/// attempt of its link for each start.
fn attempts_of(home: &Path) -> usize {
    fs::read_to_string(home.join("stand-in").join(STARTS)).map_or(0, |text| text.lines().count())
}

/// One server of a temporary home, which a test can stop and start again over the
/// same store.
struct Server {
    socket: PathBuf,
    store: Store,
    running: Option<horizon_sync::SocketServer>,
}

impl Server {
    fn start(home: &Path) -> Self {
        fs::create_dir_all(home).expect("the server home");
        let socket = home.join("server.sock");
        let store = Store::open(&home.join("server.sqlite")).expect("the store");
        let running = Some(
            run_server_on_unix_socket(&socket, store.clone(), &horizon_sync::ServerInfo::default())
                .expect("the socket"),
        );
        Self { socket, store, running }
    }

    fn stop(&mut self) {
        if let Some(server) = self.running.take() {
            server.stop();
        }
    }

    fn restart(&mut self) {
        self.running = Some(
            run_server_on_unix_socket(&self.socket, self.store.clone(), &horizon_sync::ServerInfo::default())
                .expect("the socket"),
        );
    }

    /// The section of one setting on the server: its version and its body.
    fn setting(&self, section: &str) -> Option<(u64, Value)> {
        self.store
            .all()
            .expect("the rows of the server")
            .rows
            .into_iter()
            .find(|row| row.kind == KIND_SETTING && row.id == section && !row.deleted)
            .map(|row| (row.version, row.body))
    }

    /// The names of the settings that the server holds.
    fn sections(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .store
            .all()
            .expect("the rows of the server")
            .rows
            .into_iter()
            .filter(|row| row.kind == KIND_SETTING && !row.deleted)
            .map(|row| row.id)
            .collect();
        names.sort();
        names
    }

    /// Writes one setting as a computer that is not part of the test, on the
    /// version that it saw.
    fn push_setting(
        &self,
        device: &str,
        id: u64,
        section: &str,
        seen: Option<u64>,
        body: Value,
    ) -> horizon_sync::Outcome {
        let intent = Intent {
            device: device.to_owned(),
            id,
            seen,
            op: Op::SettingPut {
                id: section.to_owned(),
                body,
            },
        };
        self.store
            .push(device, &[intent])
            .expect("the push of the setting")
            .results[0]
            .outcome
            .clone()
    }
}

/// One computer of the test: its own Horizon home, its supervisor and its
/// settings store.
struct Computer {
    home: PathBuf,
    supervisor: Option<Supervisor>,
    settings: SyncedSettings,
}

impl Computer {
    /// A computer at `home` with `config` in its settings store, linked to
    /// `socket`.
    fn start(home: &Path, config: &Config, socket: &Path) -> Self {
        Self::start_with(home, config, socket, Relay::Plain)
    }

    /// A computer whose stand-in holds its first push of the settings until the
    /// test writes [`RELEASE`], so two computers can link at the same moment.
    fn start_gated(home: &Path, config: &Config, socket: &Path) -> Self {
        Self::start_with(home, config, socket, Relay::Gated)
    }

    /// A computer whose stand-in holds its first push of the settings, and then the
    /// pull that would bring the row of that push back.
    fn start_held_pull(home: &Path, config: &Config, socket: &Path) -> Self {
        Self::start_with(home, config, socket, Relay::HeldPull)
    }

    fn start_with(home: &Path, config: &Config, socket: &Path, how: Relay) -> Self {
        fs::create_dir_all(home).expect("the home of the computer");
        let config_path = home.join("config.json");
        Config::save(&config_path, config).expect("the settings store of the computer");
        let stand_in_dir = home.join("stand-in");
        let keys = horizon_core::MachineKeys::new(home.join("machine-keys"));
        let mut link = SupervisorConfig::for_server(
            "server-vm",
            "/opt/horizon --server-link",
            &stand_in_dir,
            keys_only(keys),
        )
        .expect("the settings of the link");
        link.program = match how {
            Relay::Plain => stand_in(&stand_in_dir, socket),
            Relay::Gated => gated_stand_in(&stand_in_dir, socket),
            Relay::HeldPull => held_pull_stand_in(&stand_in_dir, socket),
        };
        link.settings_path = Some(config_path.clone());
        let supervisor = Supervisor::start(link, || {}).expect("the supervisor");
        let settings = SyncedSettings::new(supervisor.handle().clone());
        Self {
            home: home.to_path_buf(),
            supervisor: Some(supervisor),
            settings,
        }
    }

    fn handle(&self) -> &SyncHandle {
        self.supervisor.as_ref().expect("a running supervisor").handle()
    }

    fn settings(&self) -> &SyncedSettings {
        &self.settings
    }

    fn stop(&mut self) {
        if let Some(supervisor) = self.supervisor.take() {
            supervisor.stop();
        }
    }

    fn config_path(&self) -> PathBuf {
        self.home.join("config.json")
    }

    /// The settings that this computer's store holds now.
    fn stored(&self) -> Config {
        Config::read(&self.config_path())
            .expect("the settings of this computer")
            .expect("a store")
    }

    fn theme(&self) -> AppearanceTheme {
        self.stored().appearance.theme
    }

    /// A change in the Settings window: the store is written and one intent for
    /// each changed shared section follows it, in the order of one worker.
    fn save(&self, config: &Config) {
        let before = self.stored();
        Config::save(&self.config_path(), config).expect("the save of this computer");
        let shared = settings_sync::changed_sections(&before, config);
        self.settings()
            .enqueue(config, &shared)
            .expect("the intents of the settings");
    }

    /// How many attempts of the link this computer started.
    fn attempts(&self) -> usize {
        attempts_of(&self.home)
    }

    /// The version that the intent of `section` names, when one of its intents
    /// waits in the queue of this computer.
    fn queued_seen(&self, section: &str) -> Option<u64> {
        let path = self.home.join("stand-in").join("server-queue.json");
        let text = fs::read_to_string(&path).expect("the queue of this computer");
        let queue: Value = serde_json::from_str(&text).expect("the queue of this computer");
        queue["pending"]
            .as_array()
            .expect("the intents of the queue")
            .iter()
            .find(|intent| intent["op"]["setting_put"]["id"] == json!(section))
            .and_then(|intent| intent["seen"].as_u64())
    }

    /// The ids of the setting intents that wait in the queue of this computer.
    fn queued_settings(&self) -> Vec<String> {
        let path = self.home.join("stand-in").join("server-queue.json");
        let Ok(text) = fs::read_to_string(&path) else {
            return Vec::new();
        };
        let queue: Value = serde_json::from_str(&text).expect("the queue of this computer");
        queue["pending"]
            .as_array()
            .expect("the intents of the queue")
            .iter()
            .filter(|intent| intent["op"].get("setting_put").is_some())
            .map(|intent| intent["op"]["setting_put"]["id"].as_str().expect("an id").to_owned())
            .collect()
    }
}

/// The machine keys of a computer whose secrets store holds nothing else.
fn keys_only(keys: horizon_core::MachineKeys) -> std::sync::Arc<dyn horizon_core::SecretTarget> {
    use horizon_core::named_secrets::NamedSecretVault;
    use horizon_core::secret::{CredentialVault, SecretVault};
    use horizon_core::secret_store::SecretStore;

    let credentials = std::sync::Arc::new(CredentialVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
    let named = std::sync::Arc::new(NamedSecretVault::new(Box::new(
        SecretStore::in_memory().expect("a store in memory"),
    )));
    std::sync::Arc::new(SecretVault::new(keys, credentials, named))
}

fn eventually<T>(what: &str, answer: impl FnMut() -> Option<T>) -> T {
    eventually_within(PATIENCE, what, answer)
}

/// Waits for `answer` within `limit`, and names what the test waited for.
fn eventually_within<T>(limit: Duration, what: &str, mut answer: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = answer() {
            return value;
        }
        assert!(Instant::now() < deadline, "the test waited for {what} in vain");
        thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until the link of a computer is in step and its settings are too, so a
/// test writes a section on a value that the computer has read.
fn in_step(computer: &Computer) {
    eventually("the link of a computer", || {
        let settings = computer.handle().snapshot().settings().clone();
        (matches!(computer.handle().state(), LinkState::InStep { .. })
            && settings.is_seeded()
            && settings.in_step()
            && settings.pending().is_empty())
        .then_some(())
    });
}

/// Waits for one line for the person, and keeps every line that it took.
fn notice(computer: &Computer, needle: &str) -> Vec<String> {
    let deadline = Instant::now() + PATIENCE;
    let mut kept = Vec::new();
    loop {
        kept.extend(computer.handle().take_notices());
        if kept.iter().any(|line| line.contains(needle)) {
            return kept;
        }
        assert!(
            Instant::now() < deadline,
            "the test waited for the line {needle} in vain: {kept:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// The row of one section of the store of a computer.
fn row_of(computer: &Computer, section: &str) -> Option<String> {
    horizon_core::config_store::ConfigStore::resolve(&computer.config_path())
        .section_rows()
        .expect("the rows of this computer")
        .remove(section)
}

/// The row that a computer with no server writes for one section of `config`.
fn plain_row(config: &Config, section: &str) -> String {
    let directory = tempfile::tempdir().expect("a directory");
    let path = directory.path().join("config.json");
    Config::save(&path, config).expect("the store of a computer with no server");
    row_of_path(&path, section).expect("the row of the section")
}

/// The row of one section of the store at `path`.
fn row_of_path(path: &Path, section: &str) -> Option<String> {
    horizon_core::config_store::ConfigStore::resolve(path)
        .section_rows()
        .expect("the rows of that store")
        .remove(section)
}

fn config_with_theme(theme: AppearanceTheme) -> Config {
    Config {
        appearance: horizon_core::AppearanceConfig { theme },
        ..Config::default()
    }
}

/// The body of one section of `config`, as the other computer reads it.
fn section_body(config: &Config, section: &str) -> Value {
    let text = settings_sync::encode_body(config, section)
        .expect("the section of this build")
        .expect("a shared section");
    serde_json::from_str(&text).expect("one body")
}

/// Waits until the link of `computer` is away, so a change waits in its queue.
fn away(computer: &Computer) {
    eventually("the link away", || {
        matches!(computer.handle().state(), LinkState::Away { .. }).then_some(())
    });
}

/// Waits until the stand-in of `computer` holds its first push of the settings.
fn held(computer: &Computer) {
    eventually("the first push of the settings in the stand-in", || {
        fs::read_to_string(computer.home.join("stand-in").join(HELD))
            .ok()
            .filter(|text| !text.is_empty())
            .map(|_| ())
    });
}

/// Releases the push that the stand-in of `computer` holds.
fn release(computer: &Computer) {
    fs::write(computer.home.join("stand-in").join(RELEASE), b"release").expect("the release of the push");
}

/// The backup files that the first link of `computer` wrote beside its store.
fn backups(computer: &Computer) -> Vec<PathBuf> {
    fs::read_dir(computer.home.as_path())
        .expect("the home of the computer")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with("settings.before-server."))
        })
        .collect()
}

/// Writes one section of `config` as the computer `device` writes it, and answers
/// the outcome of the push. `id` is the counter of that computer: the server answers
/// a counter that it decided once with the first result.
fn push_of_another_computer(
    server: &Server,
    device: &str,
    id: u64,
    section: &str,
    config: &Config,
) -> horizon_sync::Outcome {
    let seen = server.setting(section).map(|(version, _)| version);
    server.push_setting(device, id, section, seen, section_body(config, section))
}

/// Waits until the link of `computer` is in step again after a loss, with no intent
/// that waits for the server.
fn settled(computer: &Computer) {
    eventually("the link in step with the settings of the server", || {
        let settings = computer.handle().snapshot().settings().clone();
        (matches!(computer.handle().state(), LinkState::InStep { .. })
            && settings.in_step()
            && settings.pending().is_empty())
        .then_some(())
    });
}

#[test]
fn a_theme_change_of_one_computer_reaches_the_other_and_is_applied() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &Config::default(), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&a);
    in_step(&b);

    a.save(&config_with_theme(AppearanceTheme::Light));
    // The section of this computer reaches the server before the other computer reads
    // it, so a lane that fails names the side that did not do its work.
    eventually("the light theme of the section of the server", || {
        let light = server
            .setting("appearance")
            .is_some_and(|(_, body)| body["section"]["theme"] == json!("light"));
        light.then_some(())
    });
    eventually_within(PATIENCE_LINK, "the theme of the other computer", || {
        (b.theme() == AppearanceTheme::Light).then_some(())
    });
    // The other computer read the value, and the computer that changed it ends its
    // own attempt on the answer of the server. That answer arrives after the read
    // of the other computer, so this state is waited for as well.
    eventually("the settings of the first computer in step", || {
        let settings = a.handle().snapshot().settings().clone();
        (settings.is_seeded() && settings.in_step() && settings.pending().is_empty()).then_some(())
    });
    assert_eq!(
        a.theme(),
        AppearanceTheme::Light,
        "the computer that changed it keeps it"
    );
    assert_eq!(a.stored().shortcuts.search, Config::default().shortcuts.search);
    assert!(server.setting("appearance").is_some(), "the server holds the section");
    assert!(a.queued_settings().is_empty(), "a committed intent is not queued");
    assert!(
        a.settings().view().in_step(),
        "the settings of the first computer are in step"
    );
    assert_eq!(
        a.settings().view().pending().len(),
        0,
        "a committed setting is not pending"
    );
    b.stop();
    a.stop();
    server.stop();
}

#[test]
fn two_changes_of_one_section_decide_by_the_first_and_the_other_reads_a_sticky_line() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &Config::default(), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&a);
    in_step(&b);
    // Both computers read the same version of the section.
    eventually("the second computer in step", || {
        b.settings().view().in_step().then_some(())
    });

    // The server is away, so both changes wait in the queues of their computers.
    server.stop();
    eventually("the first link away", || {
        matches!(a.handle().state(), LinkState::Away { .. }).then_some(())
    });
    eventually("the second link away", || {
        matches!(b.handle().state(), LinkState::Away { .. }).then_some(())
    });
    a.save(&config_with_theme(AppearanceTheme::Light));
    b.save(&config_with_theme(AppearanceTheme::Dark));
    eventually("both changes in their queues", || {
        (!a.queued_settings().is_empty() && !b.queued_settings().is_empty()).then_some(())
    });

    server.restart();
    // Both links return, push the change that waits and read the value that the
    // server decided.
    eventually("both computers back in step", || {
        let settled = |computer: &Computer| {
            matches!(computer.handle().state(), LinkState::InStep { .. })
                && computer.handle().snapshot().settings().pending().is_empty()
        };
        (settled(&a) && settled(&b)).then_some(())
    });
    let decided = server.setting("appearance").expect("the section of the server").1["section"]["theme"]
        .as_str()
        .expect("a theme")
        .to_owned();
    let theme = |theme: AppearanceTheme| match theme {
        AppearanceTheme::Auto => "auto",
        AppearanceTheme::Dark => "dark",
        AppearanceTheme::Light => "light",
    };
    eventually("both computers on the value of the server", || {
        (theme(a.theme()) == decided && theme(b.theme()) == decided).then_some(())
    });
    let loser_notices: Vec<String> = {
        let mut lines = a.handle().take_notices();
        lines.extend(b.handle().take_notices());
        lines
    };
    // The computer whose change lost reads one line about the setting, and the
    // line is not one of the notices that a frame takes and ends: it stays until
    // the person closes it.
    let lost: Vec<(&str, String)> = eventually("the line about the setting that lost", || {
        let lost: Vec<(&str, String)> = [("a", &a), ("b", &b)]
            .into_iter()
            .filter_map(|(name, computer)| computer.handle().settings_lost().map(|line| (name, line)))
            .collect();
        (!lost.is_empty()).then_some(lost)
    });
    assert_eq!(
        lost.len(),
        1,
        "exactly one computer reads the line about the setting that it did not keep: {lost:?}"
    );
    let (loser_name, line) = &lost[0];
    assert!(
        line.contains("appearance"),
        "the line names the setting that another computer changed first: {line}"
    );
    assert!(
        !loser_notices.iter().any(|notice| notice.contains("kept the value")),
        "the line is no short notice that a frame ends: {loser_notices:?}"
    );
    let loser = if *loser_name == "a" { &a } else { &b };
    loser.handle().dismiss_settings_lost();
    assert!(
        loser.handle().settings_lost().is_none(),
        "the line is gone when the person closes it"
    );
    let keeper = if *loser_name == "a" { &b } else { &a };
    assert!(
        keeper.handle().settings_lost().is_none(),
        "the computer whose value stayed reads no such line"
    );
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_save_before_the_first_pull_back_names_the_version_that_its_push_applied() {
    // The first link of this computer pushes every section that the server does not
    // have. The row of that push is in the mirror of this computer only when the
    // request behind it comes back, so a save of the person in that window names no
    // version, and the server answers `already_answered`: the save is lost with the
    // value of the push.
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start_held_pull(&home.path().join("b"), &Config::default(), &server.socket);
    // The stand-in holds the push of the eleven sections, so the first answer of the
    // link is behind it.
    held_push(&b);
    assert!(
        matches!(b.handle().state(), LinkState::InStep { .. }),
        "the first answer of the link is applied: {:?}",
        b.handle().state()
    );
    release(&b);
    // The push reached the server and its answer is applied; the request that would
    // bring the row of it back waits in the stand-in.
    held_request(&b);
    eventually("the answer of the push of this computer", || {
        matches!(b.handle().state(), LinkState::Behind).then_some(())
    });
    let applied = server
        .setting("appearance")
        .expect("the push of the appearance reached the server")
        .0;

    // The person changes the theme in that window, before the row of the push comes
    // back.
    b.save(&config_with_theme(AppearanceTheme::Light));
    assert_eq!(
        b.queued_seen("appearance"),
        Some(applied),
        "the intent of the save names the version that the push of this computer applied"
    );

    // The request comes back, and the save reaches the server on the version that
    // the push wrote.
    release_pull(&b);
    eventually("the section of the save on the server", || {
        server
            .setting("appearance")
            .is_some_and(|(_, body)| body["section"]["theme"] == json!("light"))
            .then_some(())
    });
    settled(&b);
    assert_eq!(
        b.theme(),
        AppearanceTheme::Light,
        "the row of the server is the value that the person saved"
    );
    assert!(
        b.handle().settings_lost().is_none(),
        "the save of the person lost to no value of another computer: {:?}",
        b.handle().settings_lost()
    );
    b.stop();
    server.stop();
}

#[test]
fn two_computers_with_equal_settings_that_join_together_read_no_line() {
    // Both computers keep the default settings and join an empty server at the same
    // moment: each one pushes every shared section, and the push that the server
    // decides second meets the row of the first one. The content of that row is the
    // content of the second computer, so nothing of it was replaced.
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start_gated(&home.path().join("a"), &Config::default(), &server.socket);
    let mut b = Computer::start_gated(&home.path().join("b"), &Config::default(), &server.socket);
    // The relay of each computer holds its push, so both pushes are composed on a
    // server that holds no section yet.
    held(&a);
    held(&b);
    assert!(
        server.sections().is_empty(),
        "no section reached the server while both pushes wait: {:?}",
        server.sections()
    );
    release(&a);
    release(&b);
    in_step(&a);
    in_step(&b);

    assert_eq!(
        server.sections().len(),
        settings_sync::shared_sections().count(),
        "the server holds every shared section once"
    );
    for (name, computer) in [("a", &a), ("b", &b)] {
        assert!(
            computer.handle().settings_lost().is_none(),
            "the computer {name} reads no line about a setting that it kept: {:?}",
            computer.handle().settings_lost()
        );
        assert_eq!(
            computer.theme(),
            Config::default().appearance.theme,
            "the computer {name} keeps the value of its own settings"
        );
        assert!(
            backups(computer).is_empty(),
            "nothing of the computer {name} was replaced, so no backup was written: {:?}",
            backups(computer)
        );
    }
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn a_push_that_loses_against_the_same_value_of_another_computer_reads_no_line() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    // The server is away, so the change of this computer waits in its queue.
    server.stop();
    away(&b);
    let light = config_with_theme(AppearanceTheme::Light);
    b.save(&light);
    eventually("the change in the queue of this computer", || {
        b.queued_settings().contains(&"appearance".to_owned()).then_some(())
    });

    // Another computer sends the same content of the section before the queue of
    // this computer reaches the server, so the push of this computer loses against
    // a value that is its own value.
    let outcome = push_of_another_computer(&server, "other-2c", 1, "appearance", &light);
    assert!(
        matches!(outcome, horizon_sync::Outcome::Applied { .. }),
        "the other computer writes the section first: {outcome:?}"
    );
    server.restart();
    settled(&b);

    assert_eq!(
        b.theme(),
        AppearanceTheme::Light,
        "the value of this computer is the value of the server"
    );
    assert!(
        b.handle().settings_lost().is_none(),
        "the value that this computer kept is the value of the server, so it reads no line: {:?}",
        b.handle().settings_lost()
    );
    assert!(
        backups(&b).is_empty(),
        "nothing of this computer was replaced, so no backup was written: {:?}",
        backups(&b)
    );
    b.stop();
    server.stop();
}

#[test]
fn a_lost_push_names_only_the_setting_whose_value_differs() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    server.stop();
    away(&b);
    let light = config_with_theme(AppearanceTheme::Light);
    b.save(&light);
    eventually("the change in the queue of this computer", || {
        b.queued_settings().contains(&"appearance".to_owned()).then_some(())
    });
    let dark = config_with_theme(AppearanceTheme::Dark);
    push_of_another_computer(&server, "other-2c", 1, "appearance", &dark);
    server.restart();
    settled(&b);

    let line = b.handle().settings_lost().expect("the line of the setting that lost");
    assert!(
        line.contains("for the setting appearance, so this computer takes that value."),
        "the line names one setting, in the singular: {line}"
    );
    assert_eq!(
        b.theme(),
        AppearanceTheme::Dark,
        "this computer takes the value of the server"
    );
    b.stop();
    server.stop();
}

#[test]
fn a_lost_push_that_names_two_settings_says_the_settings() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    server.stop();
    away(&b);
    let mut light = config_with_theme(AppearanceTheme::Light);
    light.overlays.minimap_width = 500.0;
    b.save(&light);
    eventually("the two changes in the queue of this computer", || {
        (b.queued_settings().contains(&"appearance".to_owned()) && b.queued_settings().contains(&"overlays".to_owned()))
            .then_some(())
    });
    let mut dark = config_with_theme(AppearanceTheme::Dark);
    dark.overlays.minimap_width = 620.0;
    push_of_another_computer(&server, "other-2c", 1, "appearance", &dark);
    push_of_another_computer(&server, "other-2c", 2, "overlays", &dark);
    server.restart();
    settled(&b);

    let line = b.handle().settings_lost().expect("the line of the settings that lost");
    assert!(
        line.contains("for the settings appearance, overlays, so this computer takes that value."),
        "the line names both settings, in the plural: {line}"
    );
    assert_eq!(
        b.theme(),
        AppearanceTheme::Dark,
        "this computer takes the value of the server"
    );
    assert_eq!(
        b.stored().overlays.minimap_width,
        620.0,
        "the section that lost holds the value of the server"
    );
    b.stop();
    server.stop();
}

#[test]
fn the_first_link_takes_the_value_of_the_server_and_keeps_the_own_one_in_a_backup() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    // The server already holds one section of another computer, and no other one.
    let light = config_with_theme(AppearanceTheme::Light);
    server.push_setting("other-2a", 1, "appearance", None, section_body(&light, "appearance"));
    assert_eq!(server.sections(), vec!["appearance".to_owned()]);

    // This computer has a value of its own for that section, and its own overlays.
    let mut own = config_with_theme(AppearanceTheme::Dark);
    own.overlays.minimap_width = 500.0;
    own.window.width = 1000.0;
    let mut b = Computer::start(&home.path().join("b"), &own, &server.socket);
    in_step(&b);

    eventually("the value of the server at the first link", || {
        (b.theme() == AppearanceTheme::Light).then_some(())
    });
    // The section that the server did not have is pushed as the value of this
    // computer.
    eventually("the section that the server did not have", || {
        (server.setting("overlays").is_some() && server.setting("shortcuts").is_some()).then_some(())
    });
    assert_eq!(
        server.setting("overlays").expect("the section").1["section"]["minimap_width"],
        json!(500.0),
        "the server holds the value of the computer that joined"
    );

    // The local value of the section that the server won is kept in one private
    // file beside the store.
    let mut backups: Vec<PathBuf> = fs::read_dir(b.home.as_path())
        .expect("the home of the computer")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with("settings.before-server."))
        })
        .collect();
    backups.sort();
    assert_eq!(backups.len(), 1, "one backup file: {backups:?}");
    let kept: Value = serde_json::from_str(&fs::read_to_string(&backups[0]).expect("the backup")).expect("json");
    assert_eq!(
        kept["appearance"],
        json!("{\"theme\":\"dark\"}"),
        "the value of this computer before the first link: {kept}"
    );
    assert_eq!(
        fs::metadata(&backups[0]).expect("the file").permissions().mode() & 0o777,
        0o600,
        "the settings of a person are private"
    );
    let notices = b.handle().take_notices();
    assert!(
        notices
            .iter()
            .any(|line| line.contains("settings of this computer are kept in")),
        "the person reads where the local values went: {notices:?}"
    );
    // A local section never reaches the server.
    assert!(!server.sections().contains(&"window".to_owned()));
    assert!(!server.sections().contains(&"server".to_owned()));
    b.stop();
    server.stop();
}

/// A store that fails once is no refusal of the content of a section: the next
/// answer of the server writes the section again, so it arrives on this computer.
///
/// The store of this computer is damaged when the section comes in, so the write of
/// that section fails once. The version of the section stays the same while the
/// store is broken, so only a second attempt can bring it.
#[test]
fn a_store_that_fails_once_is_written_again_on_the_next_answer_of_the_server() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    let store = horizon_core::config_store::ConfigStore::resolve(&b.config_path());
    let kept = home.path().join("kept.settings.sqlite");
    fs::copy(store.path(), &kept).expect("a copy of the store");
    fs::write(store.path(), b"this is not a settings store").expect("a store that cannot be read");

    // The section of another computer arrives, and the store of this computer fails
    // to write it.
    push_of_another_computer(
        &server,
        "other-2c",
        1,
        "appearance",
        &config_with_theme(AppearanceTheme::Light),
    );
    notice(&b, "the settings of this computer");

    // The store returns. The next answer of the server carries the same version of
    // the section, and this computer tries the write again.
    fs::copy(&kept, store.path()).expect("the store that works again");
    let mut dark = config_with_theme(AppearanceTheme::Dark);
    dark.overlays.minimap_width = 620.0;
    push_of_another_computer(&server, "other-2c", 2, "overlays", &dark);
    eventually("the theme of the server on this computer", || {
        (b.theme() == AppearanceTheme::Light).then_some(())
    });
    eventually("the section that failed once in step", || {
        b.settings().view().in_step().then_some(())
    });
    assert_eq!(
        b.stored().overlays.minimap_width,
        620.0,
        "the section of the same answer arrived too"
    );
    b.stop();
    server.stop();
}

#[test]
fn a_section_that_this_computer_refuses_is_not_applied_and_its_own_row_stays() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    // One computer of the person sends a section that breaks the rule of the
    // settings: two shortcuts of one key.
    let mut broken = Config::default();
    broken.shortcuts.new_terminal = broken.shortcuts.command_palette.clone();
    let body = json!({
        "format": settings_sync::BODY_FORMAT,
        "config_version": crate::current_config_version(),
        "section": {
            "command_palette": broken.shortcuts.command_palette,
            "new_terminal": broken.shortcuts.new_terminal
        }
    });
    let seen = server.setting("shortcuts").map(|(version, _)| version);
    server.push_setting("other-2a", 1, "shortcuts", seen, body);

    notice(&b, "not one that this Horizon accepts");
    assert_eq!(
        b.stored().shortcuts.command_palette,
        Config::default().shortcuts.command_palette,
        "the row of this computer is exactly what it was"
    );
    assert_eq!(
        b.stored().shortcuts.new_terminal,
        Config::default().shortcuts.new_terminal
    );
    assert!(
        server.setting("shortcuts").is_some(),
        "the section stays on the server, and this computer keeps its own"
    );
    assert!(
        !b.settings().view().in_step(),
        "a section that this computer refused is not in step"
    );
    assert!(
        b.settings()
            .view()
            .refused()
            .is_some_and(|line| line.contains("shortcuts")),
        "the state names the section that was refused"
    );
    b.stop();
    server.stop();
}

#[test]
fn a_section_of_a_newer_horizon_stays_on_the_server_and_says_why() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    let newer = json!({
        "format": settings_sync::BODY_FORMAT,
        "config_version": crate::current_config_version() + 1,
        "section": { "theme": "light" }
    });
    let seen = server.setting("appearance").map(|(version, _)| version);
    server.push_setting("other-2a", 1, "appearance", seen, newer);

    notice(&b, "newer Horizon");
    assert_eq!(b.theme(), AppearanceTheme::Auto, "this computer keeps its own value");
    assert!(
        server.setting("appearance").is_some(),
        "the section stays on the server"
    );
    b.stop();
    server.stop();
}

#[test]
fn six_saves_of_two_computers_keep_the_row_of_a_section_plain() {
    // A save that goes through the server once merged the envelope of the body of
    // the server as if it were the section, so every change wrapped the row of the
    // store one more time. The row of each store must be the bytes that a computer
    // with no server writes for the same value, after every change.
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &Config::default(), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&a);
    in_step(&b);

    let themes = [
        AppearanceTheme::Light,
        AppearanceTheme::Dark,
        AppearanceTheme::Auto,
        AppearanceTheme::Light,
        AppearanceTheme::Dark,
        AppearanceTheme::Auto,
    ];
    for (turn, theme) in themes.into_iter().enumerate() {
        let saver = if turn % 2 == 0 { &a } else { &b };
        let mut config = saver.stored();
        config.appearance.theme = theme;
        saver.save(&config);
        eventually("both computers on the value of this turn", || {
            let settled = |computer: &Computer| {
                computer.theme() == theme
                    && matches!(computer.handle().state(), LinkState::InStep { .. })
                    && computer.handle().snapshot().settings().pending().is_empty()
                    && computer.handle().snapshot().settings().in_step()
            };
            (settled(&a) && settled(&b)).then_some(())
        });

        let expected = plain_row(&config, "appearance");
        for (name, computer) in [("a", &a), ("b", &b)] {
            assert_eq!(
                row_of(computer, "appearance").as_deref(),
                Some(expected.as_str()),
                "the row of the store of {name} after change {turn}: the plain section of the theme {theme:?}"
            );
        }

        let (_, body) = server.setting("appearance").expect("the section of the server");
        let mut fields: Vec<&str> = body
            .as_object()
            .expect("the body of the server is one object")
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            vec!["config_version", "format", "section"],
            "the body of the server after change {turn} holds the envelope and nothing else: {body}"
        );
        let mut held: Vec<&str> = body["section"]
            .as_object()
            .expect("the section of the body is one object")
            .keys()
            .map(String::as_str)
            .collect();
        held.sort_unstable();
        let mut wanted: Vec<String> = settings_sync::body_of(&config, "appearance")
            .expect("the section of this build")
            .expect("a shared section")
            .as_object()
            .expect("one object")
            .keys()
            .cloned()
            .collect();
        wanted.sort_unstable();
        assert_eq!(
            held, wanted,
            "the section of change {turn} has the fields of the setting"
        );
    }
    a.stop();
    b.stop();
    server.stop();
}

#[test]
fn an_unknown_section_travels_and_survives_a_save_of_this_computer() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    let body = json!({
        "format": settings_sync::BODY_FORMAT,
        "config_version": crate::current_config_version(),
        "section": { "of_a_newer_horizon": 7 }
    });
    server.push_setting("other-2a", 1, "future_section", None, body);
    eventually("the unknown section in the store", || {
        let rows = horizon_core::config_store::ConfigStore::resolve(&b.config_path())
            .section_rows()
            .expect("the rows of this computer");
        rows.contains_key("future_section").then_some(())
    });

    // A save of this build keeps the section of the newer Horizon.
    b.save(&config_with_theme(AppearanceTheme::Light));
    eventually("the theme change", || {
        (server.setting("appearance").is_some()).then_some(())
    });
    let rows = horizon_core::config_store::ConfigStore::resolve(&b.config_path())
        .section_rows()
        .expect("the rows of this computer");
    assert_eq!(
        rows.get("future_section").map(String::as_str),
        Some("{\"of_a_newer_horizon\":7}"),
        "the section of a newer Horizon stays on this computer"
    );
    assert!(rows.contains_key("appearance"));
    assert_eq!(
        server
            .sections()
            .iter()
            .filter(|name| *name == "future_section")
            .count(),
        1,
        "the section of the newer Horizon stays on the server once"
    );
    b.stop();
    server.stop();
}

#[test]
fn a_local_section_never_leaves_the_computer_even_when_it_is_asked_for() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&b);

    let mut window = b.stored();
    window.window.width = 1111.0;
    window.server.ssh_target = Some("another-server".to_owned());
    Config::save(&b.config_path(), &window).expect("the save of this computer");
    let changed = settings_sync::changed_sections(&b.stored(), &window);
    assert!(
        changed.is_empty(),
        "a local section is no change that follows the server"
    );
    // Even a caller that names the section writes no intent.
    b.settings()
        .enqueue(
            &window,
            &["window".to_owned(), "server".to_owned(), "version".to_owned()],
        )
        .expect("no intent of a local section");
    assert!(b.queued_settings().is_empty(), "no local section enters the queue");
    let sections = server.sections();
    for local in ["window", "server", "version"] {
        assert!(!sections.contains(&local.to_owned()), "{local} stays on this computer");
    }
    b.stop();
    server.stop();
}

#[test]
fn a_computer_without_a_settings_path_applies_nothing_and_queues_nothing() {
    // A link that names no settings keeps every setting of this computer, and a
    // section that the server holds reaches no store.
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let stand_in_dir = home.path().join("stand-in");
    let keys = horizon_core::MachineKeys::new(home.path().join("machine-keys"));
    let mut link = SupervisorConfig::for_server(
        "server-vm",
        "/opt/horizon --server-link",
        &stand_in_dir,
        keys_only(keys),
    )
    .expect("the settings of the link");
    link.program = stand_in(&stand_in_dir, &server.socket);
    assert_eq!(link.settings_path, None, "a link without a settings path");
    let supervisor = Supervisor::start(link, || {}).expect("the supervisor");
    let handle = supervisor.handle().clone();
    eventually("the link of the computer", || {
        matches!(handle.state(), LinkState::InStep { .. }).then_some(())
    });
    let light = config_with_theme(AppearanceTheme::Light);
    server.push_setting("other-2a", 1, "appearance", None, section_body(&light, "appearance"));
    thread::sleep(Duration::from_millis(300));
    assert!(
        handle.take_notices().is_empty(),
        "a link of the machines alone reads no setting"
    );
    assert!(
        !home.path().join("settings.sqlite").exists(),
        "no settings store was written by the link"
    );
    supervisor.stop();
    server.stop();
}

/// The configuration version of this build, for a body that a test writes.
fn current_config_version() -> u32 {
    horizon_core::Config::default().version
}

/// A save while the server is away starts one attempt at once.
///
/// The wait of a lost link grows with every attempt, so a server that someone
/// started again is followed only at the end of that wait. A person who saves a
/// setting expects it to travel, so the save ends the wait of one attempt. The test
/// measures the start of the transport of the next attempt rather than a clock of
/// the link: the delay after a lost link is six seconds and this bound is well
/// under it.
#[test]
fn a_save_while_the_server_is_away_starts_an_attempt_at_once() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &Config::default(), &server.socket);
    in_step(&a);

    server.stop();
    eventually("the link away", || {
        matches!(a.handle().state(), LinkState::Away { .. }).then_some(())
    });
    // The server returns, and the link stays in its wait: it is not due yet.
    server.restart();
    thread::sleep(Duration::from_millis(600));
    assert!(
        matches!(a.handle().state(), LinkState::Away { .. }),
        "the link waits for the end of its delay: {:?}",
        a.handle().state()
    );

    let attempts = a.attempts();
    let saved_at = Instant::now();
    let mut config = a.stored();
    config.appearance.theme = AppearanceTheme::Light;
    a.save(&config);
    // A loaded computer starts a program and answers a hello at no bound, so the
    // test waits for the transport of the next attempt and not for the link that
    // follows it.
    eventually("the attempt that the save started", || {
        (a.attempts() > attempts).then_some(())
    });
    let waited = saved_at.elapsed();
    assert_eq!(
        a.attempts(),
        attempts + 1,
        "one save starts one attempt and no other one"
    );
    assert!(
        waited < Duration::from_secs(2),
        "the save started one attempt at once instead of waiting for the delay: {waited:?}"
    );
    eventually("the section of the save on the server", || {
        server.setting("appearance").is_some().then_some(())
    });
    a.stop();
    server.stop();
}
