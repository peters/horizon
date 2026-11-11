//! The press path of the Server tab, from the press to the state line, against a
//! real server of this process.
//!
//! The join of a computer that is not logged in is the one action of the tab that
//! needs no link: it names this computer, takes the token of one invitation and
//! starts the link of the address that the invitation names. These tests drive the
//! action that the tab draws, the coordination worker that joins, and the
//! supervisor of the new link, and then read the state that the tab shows.
//!
//! The server runs in this process on a TCP listener of `127.0.0.1` at port 0 and
//! on the Unix socket of a temporary home, with real store files. The whole suite
//! runs under the guard of the UI thread in strict mode, so a file or a socket on a
//! frame panics instead of freezing the window.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{RuntimeState, StartupDecision};
use horizon_sync::{
    Clock, DeviceRow, Invited, LinkErrorCode, LinkRegistry, ListenerInfo, SERVER_DEVICE, ServerHome, ServerInfo,
    SocketServer, Store, SystemClock, TcpServer, first_invitation, run_server_on_tcp, run_server_on_unix_socket,
};

use super::HorizonApp;
use super::settings::server::{ServerAction, ServerView};
use super::test_support::{run_app_frame, test_app_with_startup};

/// How long one test waits for the join that it pressed and for the link of the
/// new server.
const PATIENCE: Duration = Duration::from_secs(30);

/// One server of a test: its home, its store, its TCP listener and its socket.
struct TestServer {
    directory: tempfile::TempDir,
    store: Store,
    registry: Arc<LinkRegistry>,
    tcp: Option<TcpServer>,
    socket: Option<SocketServer>,
}

impl TestServer {
    fn start() -> Self {
        let directory = tempfile::tempdir().expect("a directory of the server");
        let store = Store::open(&directory.path().join("server.sqlite")).expect("the store");
        let listener = ListenerInfo::free(None);
        let registry = Arc::new(LinkRegistry::default());
        let info = ServerInfo {
            listener: Some(listener.clone()),
            registry: Arc::clone(&registry),
            clock: Arc::new(SystemClock) as Arc<dyn Clock>,
        };
        let tcp = run_server_on_tcp(&listener, store.clone(), &info).expect("the listener of the server");
        let socket = run_server_on_unix_socket(&directory.path().join("server.sock"), store.clone(), &info)
            .expect("the socket of the server");
        Self {
            directory,
            store,
            registry,
            tcp: Some(tcp),
            socket: Some(socket),
        }
    }

    /// One invitation, asked through the socket of the server computer.
    fn invite(&self) -> Invited {
        first_invitation(&ServerHome::at(self.home()), SERVER_DEVICE).expect("the first invitation")
    }

    fn home(&self) -> std::path::PathBuf {
        self.directory.path().to_path_buf()
    }

    /// The computers of this server, in name order.
    fn devices(&self) -> Vec<DeviceRow> {
        self.store.devices(SERVER_DEVICE).expect("the computers of the server")
    }

    fn names(&self) -> Vec<String> {
        self.devices().into_iter().map(|row| row.name).collect()
    }

    /// Removes one computer, as the tab of another computer removes it: the row
    /// goes and the links of that computer end with the typed error.
    fn remove(&self, name: &str) {
        assert!(
            self.store.remove_device(name).expect("the removal of a computer"),
            "the server held {name}"
        );
        self.registry.end_device(
            name,
            None,
            LinkErrorCode::Removed,
            "This computer was removed from the server.",
        );
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tcp) = self.tcp.take() {
            tcp.stop();
        }
        if let Some(socket) = self.socket.take() {
            socket.stop();
        }
        self.store.clone().close();
    }
}

/// A window with the Server tab open and no server in its settings.
fn window_with_the_server_tab() -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.toggle_settings();
    app.settings
        .as_mut()
        .expect("the settings window of a test")
        .show_server_tab();
    run_app_frame(&ctx, &mut app);
    (temp, ctx, app)
}

/// The state of the Server tab of `app`.
fn tab(app: &HorizonApp) -> &super::settings::server::ServerTabState {
    &app.settings.as_ref().expect("the settings window of a test").server
}

/// The device name that this computer wrote, when it wrote one.
fn written_name(app: &HorizonApp) -> Option<String> {
    let path = app.machine_registry().root().join("server-device");
    std::fs::read_to_string(path).ok().map(|text| text.trim().to_owned())
}

/// Writes the text of one invitation into the field of the tab, and presses Join
/// from the fields that the tab draws. Answers the action of that press.
fn press_join(app: &mut HorizonApp, ctx: &Context, link: &str) -> ServerAction {
    {
        let state = &mut app.settings.as_mut().expect("the settings window of a test").server;
        state.link = link.to_owned();
        state.join_error = None;
    }
    // The frame is the one that fills the name that this computer proposes.
    run_app_frame(ctx, app);
    let state = tab(app);
    let action = ServerAction::Join {
        link: state.link.trim().to_owned(),
        device: state.device.trim().to_owned(),
    };
    app.apply_server_action(action.clone(), ctx.clone());
    action
}

/// Runs the window until `done` holds for it.
fn run_until(app: &mut HorizonApp, ctx: &Context, what: &str, mut done: impl FnMut(&HorizonApp) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        app.settle_browser_host_io();
        run_app_frame(ctx, app);
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the window did not reach {what}");
}

fn view(app: &HorizonApp) -> ServerView {
    app.server_view()
}

fn is_in_step(app: &HorizonApp) -> bool {
    view(app).line == "Server: in step"
}

/// The name of the field of the join, which a test may set before a press.
fn device_field(app: &mut HorizonApp) -> &mut String {
    &mut app
        .settings
        .as_mut()
        .expect("the settings window of a test")
        .server
        .device
}

#[test]
fn a_computer_with_no_server_and_no_name_joins_with_the_name_that_the_tab_proposes() {
    let server = TestServer::start();
    let invited = server.invite();
    let (_temp, ctx, mut app) = window_with_the_server_tab();

    assert_eq!(
        view(&app).line,
        "This computer has no server.",
        "a fresh computer has no link and no name"
    );
    let proposed = tab(&app).device.clone();
    assert!(
        proposed.starts_with("horizon-") && proposed.len() > "horizon-".len(),
        "the tab proposes the name that the first link of this computer would take: {proposed:?}"
    );
    assert_eq!(written_name(&app), None, "the proposal is no file yet");

    let ServerAction::Join { link, device } = press_join(&mut app, &ctx, &invited.link) else {
        panic!("the press of Join is a join");
    };
    assert_eq!(link, invited.link);
    assert_eq!(device, proposed, "a person presses Join without typing a name");

    run_until(&mut app, &ctx, "the link of the server in step", is_in_step);
    assert_eq!(written_name(&app).as_deref(), Some(proposed.as_str()));
    let joined = server
        .devices()
        .into_iter()
        .find(|row| row.name == proposed)
        .expect("the server holds the computer under the name of the join");
    assert_eq!(joined.login, horizon_sync::DeviceLogin::Token);
    assert_eq!(view(&app).device, proposed, "the tab shows the name of the link");
    assert!(
        view(&app).linked,
        "the tab offers the invitation and the list of a link"
    );
    assert!(view(&app).name_locked, "the name belongs to the login");
}

#[test]
fn a_computer_that_types_a_name_joins_as_that_name_and_keeps_it() {
    let server = TestServer::start();
    let invited = server.invite();
    let (_temp, ctx, mut app) = window_with_the_server_tab();
    device_field(&mut app).clone_from(&"window-b".to_owned());

    let ServerAction::Join { device, .. } = press_join(&mut app, &ctx, &invited.link) else {
        panic!("the press of Join is a join");
    };
    assert_eq!(device, "window-b", "the join takes the name that the person typed");

    run_until(&mut app, &ctx, "the link of the server in step", is_in_step);
    assert_eq!(written_name(&app).as_deref(), Some("window-b"));
    assert!(server.names().iter().any(|name| name == "window-b"));
    assert_eq!(
        view(&app).device,
        "window-b",
        "the name of the join is the name of the link, so the server knows this computer"
    );
}

#[test]
fn a_wrong_code_shows_its_sentence_and_leaves_no_computer_on_the_server() {
    let server = TestServer::start();
    let invited = server.invite();
    let (_temp, ctx, mut app) = window_with_the_server_tab();
    device_field(&mut app).clone_from(&"window-c".to_owned());
    let code = horizon_core::JoinLink::parse(&invited.link)
        .expect("the link of the invitation")
        .code()
        .to_owned();
    let wrong_code = if code.starts_with('A') {
        "BBBB-BBBB-BBBB"
    } else {
        "AAAA-AAAA-AAAA"
    };
    let wrong = invited.link.replace(&code, wrong_code);

    press_join(&mut app, &ctx, &wrong);
    run_until(&mut app, &ctx, "the sentence of a wrong code", |app| {
        tab(app).join_error.is_some()
    });
    let sentence = tab(&app).join_error.clone().expect("the sentence of the refusal");
    assert!(
        sentence.starts_with("The code is not one open invitation of this server."),
        "{sentence}"
    );
    assert!(!tab(&app).joining, "the press ended, so a person reads the sentence");
    assert!(
        !server.names().iter().any(|name| name == "window-c"),
        "a refused join leaves no computer on the server: {:?}",
        server.names()
    );
    assert!(app.server_sync.is_none(), "a refused join starts no link");
    assert_eq!(view(&app).line, "This computer has no server.");
}

#[test]
fn a_removed_computer_reads_that_it_was_removed_and_joins_again_with_a_new_link() {
    let server = TestServer::start();
    let invited = server.invite();
    let (_temp, ctx, mut app) = window_with_the_server_tab();
    device_field(&mut app).clone_from(&"window-d".to_owned());
    press_join(&mut app, &ctx, &invited.link);
    run_until(&mut app, &ctx, "the link of the server in step", is_in_step);
    // The server registers the link of a computer just after it greets it, so the
    // removal waits for the link that it ends.
    run_until(&mut app, &ctx, "the link of this computer on the server", |_| {
        server.registry.devices().contains("window-d")
    });

    // Another computer removes this one: its row goes and its link ends with the
    // typed error.
    server.remove("window-d");
    run_until(&mut app, &ctx, "the line of a removed computer", |app| {
        view(app).logged_out
    });
    let removed = view(&app);
    assert_eq!(
        removed.line, "This computer was removed from the server.",
        "the tab says what happened instead of that it is connecting"
    );
    assert!(
        !removed.linked,
        "the invitation and the list of a link that ended are gone"
    );
    assert!(
        !removed.name_locked,
        "the token is gone, so a person may name this computer again"
    );

    // A new invitation joins this computer again, under the name that it already
    // has.
    let fresh = server.invite();
    press_join(&mut app, &ctx, &fresh.link);
    run_until(&mut app, &ctx, "the link of the server in step again", is_in_step);
    assert!(server.names().iter().any(|name| name == "window-d"));
    assert_eq!(view(&app).device, "window-d");
    assert!(view(&app).linked && view(&app).name_locked);
}
