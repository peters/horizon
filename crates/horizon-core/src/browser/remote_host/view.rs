//! The connection of one panel to its browser host: the frames that cross it,
//! the input that goes back, and the state that the panel reads.
//!
//! The pump thread owns the MCP client of the host. It creates the page once,
//! then carries each queued panel command to that page and reads one frame per
//! tick, so the panel's slot always holds the latest page of the host. The panel
//! thread never waits on the channel: it reads the latest state from a slot and
//! sends a command through the queue, which is how the cloud view of a worker
//! browser works as well.

use std::io::{BufRead, Write};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use horizon_browser::{BackendKind, BrowserCommand, BrowserEventWaker, FrameSlot};

use super::client::{ClientError, McpClient};
use super::input::translate;
use super::{RemoteHostFailure, RemoteHostRefusal};

/// How often the pump reads the page of the host. One tick reads one frame, and
/// every [`STATE_EVERY`] ticks it reads the page state as well.
const POLL: Duration = Duration::from_millis(150);
const STATE_EVERY: u32 = 5;
/// Most commands that one tick carries to the page.
const MAX_COMMANDS: usize = 64;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// A create waits for a page and its engine, so it keeps a longer bound.
const CREATE_TIMEOUT: Duration = Duration::from_secs(75);
/// Commands this panel can queue before one is refused.
const QUEUE: usize = 128;
/// How long the channel stays open after a panel stops, so the pump can close the
/// page of the host through it. A channel that is unresponsive is stopped when
/// this bound ends, so no `ssh` process outlives a panel.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

/// The transport of one panel to its browser host: the standard streams of the
/// MCP server, and the way to stop it.
pub(super) trait HostChannel: Send {
    fn take_streams(&mut self) -> Option<(Box<dyn Write + Send>, Box<dyn BufRead + Send>)>;
    fn stop(&mut self);
}

/// The page that a panel asks its host for.
pub(super) struct HostPage {
    pub(super) url: Option<String>,
    pub(super) profile: Option<String>,
}

/// What the client of a browser host last read from the page of this panel.
#[derive(Clone, Debug)]
pub(super) struct HostViewState {
    pub(super) ready: bool,
    pub(super) url: String,
    pub(super) title: String,
    pub(super) backend: BackendKind,
    /// Why the channel ended, when it did.
    pub(super) failure: Option<RemoteHostFailure>,
    /// The reason of the host for the last command it refused.
    pub(super) note: Option<String>,
}

impl HostViewState {
    const fn starting(backend: BackendKind) -> Self {
        Self {
            ready: false,
            url: String::new(),
            title: String::new(),
            backend,
            failure: None,
            note: None,
        }
    }
}

/// Opens the channel of one panel to its host. It runs on the worker thread of
/// the view, so a host that does not answer never holds the thread that asked for
/// the panel, and a person keeps seeing the window paint.
pub(super) type HostOpener = Box<dyn FnOnce() -> Result<Box<dyn HostChannel>, RemoteHostFailure> + Send>;

/// The channel of a view once it exists. The pump stores it as soon as it opens,
/// so a stop that arrives during the open still stops it.
type ChannelSlot = Arc<Mutex<Option<Box<dyn HostChannel>>>>;

/// One live connection of a panel to its browser host.
pub(super) struct HostView {
    tx: mpsc::SyncSender<BrowserCommand>,
    alive: Weak<()>,
    latest: Arc<Mutex<Option<HostViewState>>>,
    waker: Arc<Mutex<Option<BrowserEventWaker>>>,
    stop: Arc<AtomicBool>,
    channel: ChannelSlot,
}

impl HostView {
    /// Start the worker thread that opens the channel, then creates the page of
    /// `page` on the host and pumps its frames. This call returns at once: the
    /// outcome, including a host that never answers, is a state of the panel.
    pub(super) fn start(open: HostOpener, page: HostPage, backend: BackendKind, frames: Arc<FrameSlot>) -> Self {
        let (tx, commands) = mpsc::sync_channel(QUEUE);
        let shared = Shared {
            latest: Arc::default(),
            waker: Arc::default(),
            stop: Arc::new(AtomicBool::new(false)),
            frames,
        };
        let channel: ChannelSlot = Arc::default();
        let alive = Arc::new(());
        let view = Self {
            tx,
            alive: Arc::downgrade(&alive),
            latest: Arc::clone(&shared.latest),
            waker: Arc::clone(&shared.waker),
            stop: Arc::clone(&shared.stop),
            channel: Arc::clone(&channel),
        };
        std::thread::spawn(move || {
            let mut opened = match open() {
                Ok(channel) => channel,
                Err(failure) => {
                    // The host never answered. The panel draws this reason, and
                    // the thread that asked for the panel is already free.
                    publish(
                        &shared,
                        Some(HostViewState {
                            failure: Some(failure),
                            ..HostViewState::starting(backend)
                        }),
                    );
                    drop(commands);
                    drop(alive);
                    shared.stop.store(true, Ordering::Release);
                    return;
                }
            };
            let streams = opened.take_streams();
            *lock(&channel) = Some(opened);
            let ended = match streams {
                Some((input, output)) => drive(McpClient::start(input, output), &commands, &shared, &page, backend),
                None => Some((RemoteHostFailure::STOPPED, None)),
            };
            // Drop the receiver before the last state, so a send cannot succeed
            // after the pump stopped reading it.
            drop(commands);
            drop(alive);
            if let Some((failure, note)) = ended {
                publish(
                    &shared,
                    Some(HostViewState {
                        failure: Some(failure),
                        note,
                        ..HostViewState::starting(backend)
                    }),
                );
            }
            stop_channel(&channel);
            shared.stop.store(true, Ordering::Release);
        });
        view
    }

    /// Queue one command for the page of the host.
    pub(super) fn send(&self, command: BrowserCommand) -> bool {
        self.can_send() && self.tx.try_send(command).is_ok()
    }

    /// Whether the pump still reads the queue. A full queue can still refuse one
    /// command.
    pub(super) fn can_send(&self) -> bool {
        self.alive.upgrade().is_some()
    }

    /// The latest state of the page, taken once.
    pub(super) fn take_state(&self) -> Option<HostViewState> {
        lock(&self.latest).take()
    }

    pub(super) fn needs_waker(&self) -> bool {
        lock(&self.waker).is_none()
    }

    pub(super) fn set_waker(&self, waker: BrowserEventWaker) {
        *lock(&self.waker) = Some(waker);
    }

    /// Stop this panel: the pump closes the page of the host through the live
    /// channel, and the channel is stopped when it did or when the grace ends.
    /// A channel that is still opening has no process yet, so the stop also
    /// covers a host that never answers.
    pub(super) fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        let channel = Arc::clone(&self.channel);
        std::thread::spawn(move || {
            std::thread::sleep(SHUTDOWN_GRACE);
            stop_channel(&channel);
        });
    }
}

struct Shared {
    latest: Arc<Mutex<Option<HostViewState>>>,
    waker: Arc<Mutex<Option<BrowserEventWaker>>>,
    stop: Arc<AtomicBool>,
    frames: Arc<FrameSlot>,
}

fn lock<T>(slot: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stop the process of the channel, when it exists yet.
fn stop_channel(slot: &ChannelSlot) {
    if let Some(channel) = lock(slot).as_mut() {
        channel.stop();
    }
}

fn publish(shared: &Shared, state: Option<HostViewState>) {
    *lock(&shared.latest) = state;
    if let Some(waker) = lock(&shared.waker).as_ref() {
        waker();
    }
}

/// The reason a command did not reach the page, when the host refused it.
fn refused_note(refusal: RemoteHostRefusal) -> String {
    refusal.message().to_string()
}

/// Carry the panel's work to the page until the panel stops, then close it.
/// Returns the failure that ended the channel, or `None` for a normal stop.
fn drive(
    mut client: McpClient,
    commands: &mpsc::Receiver<BrowserCommand>,
    shared: &Shared,
    page: &HostPage,
    backend: BackendKind,
) -> Option<(RemoteHostFailure, Option<String>)> {
    run(&mut client, commands, shared, page, backend).err()
}

fn run(
    client: &mut McpClient,
    commands: &mpsc::Receiver<BrowserCommand>,
    shared: &Shared,
    page: &HostPage,
    backend: BackendKind,
) -> Result<(), (RemoteHostFailure, Option<String>)> {
    client.initialize(CALL_TIMEOUT).map_err(|error| failure_of(&error))?;
    let mut state = HostViewState::starting(backend);
    let created = client
        .call("browser_create", &create_arguments(page), CREATE_TIMEOUT)
        .map_err(|error| failure_of(&error))?;
    // A create answers the page under `panel`, and every other call names the
    // page at its top level.
    let Some(panel_id) = created
        .value
        .get("panel")
        .and_then(|panel| panel.get("panel_id"))
        .or_else(|| created.value.get("panel_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
    else {
        return Err((
            RemoteHostFailure::REFUSED,
            Some("the browser host created no page".to_string()),
        ));
    };
    apply_panel(&mut state, created.value.get("panel"));
    state.ready = true;
    publish(shared, Some(state.clone()));

    let mut ticks = 0u32;
    while !shared.stop.load(Ordering::Acquire) {
        for command in commands.try_iter().take(MAX_COMMANDS) {
            let actions = match translate(&command, &panel_id) {
                Ok(actions) => actions,
                Err(refusal) => {
                    state.note = Some(refused_note(refusal));
                    continue;
                }
            };
            for action in actions {
                match client.call(action.tool, &action.arguments, CALL_TIMEOUT) {
                    Ok(_) => {}
                    Err(ClientError::Refused(reason)) => state.note = Some(reason),
                    Err(error) => return Err(failure_of(&error)),
                }
            }
        }
        match client.call("browser_screenshot", &panel_arguments(&panel_id), CALL_TIMEOUT) {
            Ok(frame) => {
                if let Some(png) = frame.image.as_deref() {
                    let _ = shared.frames.store_base64_png(png);
                }
            }
            Err(ClientError::Refused(reason)) => {
                // A screen is not ready for every moment of a page: the host
                // refuses a capture before the first frame, while the page
                // loads, and while another claim holds it. The reason stays in
                // the panel note and the panel keeps reading the page.
                state.note = Some(reason);
            }
            Err(error) => return Err(failure_of(&error)),
        }
        ticks = ticks.wrapping_add(1);
        if ticks.is_multiple_of(STATE_EVERY)
            && let Ok(panel) = client.call("browser_panel", &panel_arguments(&panel_id), CALL_TIMEOUT)
        {
            apply_panel(&mut state, Some(&panel.value));
        }
        publish(shared, Some(state.clone()));
        std::thread::sleep(POLL);
    }
    // The page of the host belongs to this panel, so it closes with the panel.
    let _ = client.call("browser_close", &panel_arguments(&panel_id), CALL_TIMEOUT);
    Ok(())
}

fn failure_of(error: &ClientError) -> (RemoteHostFailure, Option<String>) {
    match error {
        ClientError::Refused(reason) => (RemoteHostFailure::REFUSED, Some(reason.clone())),
        ClientError::Stopped => (RemoteHostFailure::STOPPED, None),
        ClientError::NoAnswer(_) => (RemoteHostFailure::NO_ANSWER, Some(error.to_string())),
        ClientError::Write(reason) => (RemoteHostFailure::STOPPED, Some(reason.clone())),
    }
}

fn create_arguments(page: &HostPage) -> serde_json::Value {
    let mut arguments = serde_json::json!({});
    if let Some(profile) = page.profile.as_deref() {
        arguments["target"] = serde_json::Value::String(profile.to_string());
    }
    if let Some(url) = page.url.as_deref() {
        arguments["url"] = serde_json::Value::String(url.to_string());
    }
    arguments
}

/// The one argument of every call that names the page of this panel.
fn panel_arguments(panel_id: &str) -> serde_json::Value {
    serde_json::json!({ "panel_id": panel_id })
}

/// Read the page state that a create answer or a `browser_panel` result gives.
fn apply_panel(state: &mut HostViewState, panel: Option<&serde_json::Value>) {
    let Some(panel) = panel else { return };
    if let Some(url) = panel.get("url").and_then(serde_json::Value::as_str) {
        state.url = url.to_string();
    }
    if let Some(title) = panel.get("title").and_then(serde_json::Value::as_str) {
        state.title = title.to_string();
    }
    if let Some(backend) = panel
        .get("backend")
        .and_then(serde_json::Value::as_str)
        .and_then(backend_of)
    {
        state.backend = backend;
    }
}

/// The engine that the host reports for one page.
fn backend_of(name: &str) -> Option<BackendKind> {
    match name {
        "chromium" => Some(BackendKind::ChromiumCdp),
        "firefox" => Some(BackendKind::FirefoxBidi),
        "safari" => Some(BackendKind::SafariWebDriver),
        _ => None,
    }
}
