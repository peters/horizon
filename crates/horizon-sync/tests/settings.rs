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

/// A stand-in `ssh` that runs the link relay against the socket of the server.
fn stand_in(directory: &Path, socket: &Path) -> String {
    fs::create_dir_all(directory).expect("the directory of the stand-in");
    let relay = directory.join("relay.py");
    fs::write(&relay, RELAY).expect("the relay of the stand-in");
    let program = directory.join("ssh");
    let script = format!(
        "#!/bin/sh\nexec /usr/bin/env python3 '{}' '{}'\n",
        relay.display(),
        socket.display()
    );
    fs::write(&program, script).expect("the stand-in ssh");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).expect("an executable stand-in");
    program.to_string_lossy().into_owned()
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
        link.program = stand_in(&stand_in_dir, socket);
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

fn eventually<T>(what: &str, mut answer: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
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
        (matches!(computer.handle().state(), LinkState::InStep { .. })
            && computer.handle().snapshot().settings().in_step()
            && computer.handle().snapshot().settings().pending().is_empty())
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

#[test]
fn a_theme_change_of_one_computer_reaches_the_other_and_is_applied() {
    let home = tempfile::tempdir().expect("a directory");
    let mut server = Server::start(&home.path().join("server"));
    let mut a = Computer::start(&home.path().join("a"), &Config::default(), &server.socket);
    let mut b = Computer::start(&home.path().join("b"), &Config::default(), &server.socket);
    in_step(&a);
    in_step(&b);

    a.save(&config_with_theme(AppearanceTheme::Light));
    eventually("the theme of the other computer", || {
        (b.theme() == AppearanceTheme::Light).then_some(())
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
fn two_changes_of_one_section_decide_by_the_first_and_the_other_reads_a_notice() {
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
    assert!(
        loser_notices
            .iter()
            .any(|line| line.contains("kept the value of another computer")),
        "the computer that lost reads one line about it: {loser_notices:?}"
    );
    a.stop();
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
