use std::{
    sync::{Mutex, OnceLock, TryLockError, mpsc},
    thread,
    time::{Duration, Instant},
};

use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, NONE,
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        Event,
        res::{ClientIdMask, ClientIdSpec, ConnectionExt as _},
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property, SELECTION_NOTIFY_EVENT,
            SelectionNotifyEvent, SelectionRequestEvent, Time, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::{InjectError, send_paste_chord};

mod data;
mod read;

use data::{ClientIdentity, ClipboardSnapshot, StagedText, StoredTarget, StoredValue};

const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(750);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(1);
const MAX_TARGETS: usize = 96;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

static PASTE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

x11rb::atom_manager! {
    Atoms: AtomCookies {
        CLIPBOARD,
        CLIPBOARD_MANAGER,
        TARGETS,
        MULTIPLE,
        TIMESTAMP,
        SAVE_TARGETS,
        INCR,
        UTF8_STRING,
        UTF8_MIME_0: b"text/plain;charset=utf-8",
        UTF8_MIME_1: b"text/plain;charset=UTF-8",
        TEXT_MIME: b"text/plain",
        STRING,
        TEXT,
        X_KDE_PASSWORDMANAGERHINT: b"x-kde-passwordManagerHint",
        HORIZON_CLIPBOARD_DATA,
        HORIZON_CLIPBOARD_TIME,
        _NET_WM_PID,
    }
}

pub(super) fn paste_text_preserving_clipboard(text: &str) -> Result<(), InjectError> {
    let transcript = text.to_owned();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("horizon-clipboard-paste".to_owned())
        .spawn(move || paste_worker(&transcript, &result_tx))
        .map_err(|_| InjectError::Failed("failed to start clipboard paste worker"))?;
    result_rx
        .recv()
        .unwrap_or(Err(InjectError::Failed("clipboard paste worker stopped")))
}

fn paste_worker(transcript: &str, result_tx: &mpsc::SyncSender<Result<(), InjectError>>) {
    let paste_lock = match PASTE_LOCK.get_or_init(|| Mutex::new(())).try_lock() {
        Ok(paste_lock) => paste_lock,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => {
            let _ = result_tx.send(Err(InjectError::Failed("another speech paste is still pending")));
            return;
        }
    };
    let mut session = match ClipboardSession::new() {
        Ok(session) => session,
        Err(error) => {
            let _ = result_tx.send(Err(error));
            return;
        }
    };
    let result = session.stage_and_send(transcript);
    let staged = result.is_ok();
    let _ = result_tx.send(result);
    if staged {
        session.wait_for_target_request();
    }
    session.staged = None;
    if session.owns_clipboard && session.snapshot.is_empty() {
        return;
    }
    if session.owns_clipboard {
        session.request_manager_handover();
    }
    drop(paste_lock);
    session.serve_restored_clipboard();
}

struct ClipboardSession {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
    snapshot: ClipboardSnapshot,
    staged: Option<StagedText>,
    owns_clipboard: bool,
    target_identity: Option<ClientIdentity>,
    max_property_bytes: usize,
}

impl ClipboardSession {
    fn new() -> Result<Self, InjectError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|_| InjectError::Unsupported)?;
        let root = conn.setup().roots.get(screen_num).ok_or(InjectError::Unsupported)?.root;
        let window = conn
            .generate_id()
            .map_err(|_| InjectError::Clipboard("failed to allocate clipboard window"))?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::COPY_FROM_PARENT,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|_| InjectError::Clipboard("failed to create clipboard window"))?
        .check()
        .map_err(|_| InjectError::Clipboard("failed to create clipboard window"))?;
        let atoms = Atoms::new(&conn)
            .map_err(|_| InjectError::Clipboard("failed to initialize clipboard atoms"))?
            .reply()
            .map_err(|_| InjectError::Clipboard("failed to initialize clipboard atoms"))?;
        conn.flush()
            .map_err(|_| InjectError::Clipboard("failed to initialize clipboard connection"))?;
        let max_property_bytes = conn.maximum_request_bytes().saturating_sub(64);
        Ok(Self {
            conn,
            window,
            atoms,
            snapshot: ClipboardSnapshot::default(),
            staged: None,
            owns_clipboard: false,
            target_identity: None,
            max_property_bytes,
        })
    }

    fn stage_and_send(&mut self, transcript: &str) -> Result<(), InjectError> {
        if transcript.len() > self.max_property_bytes {
            return Err(InjectError::Clipboard("speech transcript is too large to paste safely"));
        }
        let snapshot_time = self.server_time()?;
        let original_owner = self.selection_owner(self.atoms.CLIPBOARD)?;
        self.snapshot = self.read_snapshot(original_owner, snapshot_time)?;
        self.claim_clipboard(snapshot_time)?;
        self.staged = Some(StagedText::new(transcript));
        self.send_paste()
    }

    fn send_paste(&mut self) -> Result<(), InjectError> {
        let focus_before = self.focus_identity()?;
        self.target_identity = Some(focus_before);
        send_paste_chord()?;
        if self.focus_identity()? != focus_before {
            return Err(InjectError::Failed("focused application changed during speech paste"));
        }
        Ok(())
    }

    fn wait_for_target_request(&mut self) {
        loop {
            let Ok(event) = self.conn.wait_for_event() else {
                return;
            };
            match event {
                Event::SelectionRequest(request) => match self.respond_to_request(request) {
                    Ok(false) => {}
                    Ok(true) | Err(_) => return,
                },
                Event::SelectionClear(event) if event.selection == self.atoms.CLIPBOARD => {
                    self.owns_clipboard = false;
                    return;
                }
                _ => {}
            }
        }
    }

    fn serve_restored_clipboard(&mut self) {
        if !self.owns_clipboard {
            return;
        }
        loop {
            let Ok(event) = self.conn.wait_for_event() else {
                return;
            };
            match event {
                Event::SelectionRequest(request) => {
                    if self.respond_to_request(request).is_err() {
                        return;
                    }
                }
                Event::SelectionClear(event) if event.selection == self.atoms.CLIPBOARD => return,
                _ => {}
            }
        }
    }

    fn claim_clipboard(&mut self, timestamp: u32) -> Result<(), InjectError> {
        self.conn
            .set_selection_owner(self.window, self.atoms.CLIPBOARD, timestamp)
            .map_err(|_| InjectError::Clipboard("failed to stage speech transcript"))?
            .check()
            .map_err(|_| InjectError::Clipboard("failed to stage speech transcript"))?;
        self.conn
            .flush()
            .map_err(|_| InjectError::Clipboard("failed to stage speech transcript"))?;
        if self.selection_owner(self.atoms.CLIPBOARD)? != self.window {
            return Err(InjectError::Clipboard("clipboard changed while preparing speech paste"));
        }
        self.owns_clipboard = true;
        Ok(())
    }

    fn respond_to_request(&self, request: SelectionRequestEvent) -> Result<bool, InjectError> {
        if request.selection != self.atoms.CLIPBOARD {
            self.send_selection_notify(request, NONE)?;
            return Ok(false);
        }
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let serve_staged = self.staged.is_some() && self.request_matches_target(request.requestor);
        let wrote_transcript = if request.target == self.atoms.TARGETS {
            let targets = self.advertised_targets(serve_staged);
            self.conn
                .change_property32(PropMode::REPLACE, request.requestor, property, AtomEnum::ATOM, &targets)
                .map_err(|_| InjectError::Clipboard("failed to serve clipboard target list"))?
                .check()
                .map_err(|_| InjectError::Clipboard("failed to serve clipboard target list"))?;
            false
        } else if serve_staged {
            let staged = self.staged.as_ref().ok_or(InjectError::Clipboard(
                "speech transcript was not available for the target request",
            ))?;
            let Some(wrote_transcript) = self.write_staged_text(request.requestor, property, request.target, staged)?
            else {
                self.send_selection_notify(request, NONE)?;
                return Ok(false);
            };
            wrote_transcript
        } else if let Some(stored) = self.snapshot.find(request.target) {
            self.write_stored_target(request.requestor, property, stored)?;
            false
        } else {
            self.send_selection_notify(request, NONE)?;
            return Ok(false);
        };
        self.send_selection_notify(request, property)?;
        Ok(wrote_transcript)
    }

    fn advertised_targets(&self, serve_staged: bool) -> Vec<Atom> {
        let mut targets = if serve_staged {
            let mut targets = vec![
                self.atoms.UTF8_STRING,
                self.atoms.UTF8_MIME_0,
                self.atoms.UTF8_MIME_1,
                self.atoms.TEXT_MIME,
                self.atoms.TEXT,
            ];
            if self.staged.as_ref().is_some_and(|staged| staged.latin1.is_some()) {
                targets.push(self.atoms.STRING);
            }
            targets
        } else {
            self.snapshot.targets.iter().map(|target| target.target).collect()
        };
        targets.push(self.atoms.TARGETS);
        if self.staged.is_none() && !self.snapshot.is_empty() {
            targets.push(self.atoms.SAVE_TARGETS);
        }
        targets
    }

    fn write_staged_text(
        &self,
        requestor: Window,
        property: Atom,
        target: Atom,
        staged: &StagedText,
    ) -> Result<Option<bool>, InjectError> {
        let (property_type, bytes) = if target == self.atoms.STRING {
            let Some(latin1) = staged.latin1.as_deref() else {
                return Ok(None);
            };
            (self.atoms.STRING, latin1)
        } else if target == self.atoms.TEXT {
            (self.atoms.UTF8_STRING, staged.utf8.as_slice())
        } else if self.is_utf8_target(target) {
            (target, staged.utf8.as_slice())
        } else {
            return Ok(None);
        };
        self.conn
            .change_property8(PropMode::REPLACE, requestor, property, property_type, bytes)
            .map_err(|_| InjectError::Clipboard("failed to serve speech transcript"))?
            .check()
            .map_err(|_| InjectError::Clipboard("failed to serve speech transcript"))?;
        Ok(Some(true))
    }

    fn write_stored_target(&self, requestor: Window, property: Atom, stored: &StoredTarget) -> Result<(), InjectError> {
        let result = match &stored.value {
            StoredValue::Bytes8(value) => {
                self.conn
                    .change_property8(PropMode::REPLACE, requestor, property, stored.property_type, value)
            }
            StoredValue::Bytes16(value) => {
                self.conn
                    .change_property16(PropMode::REPLACE, requestor, property, stored.property_type, value)
            }
            StoredValue::Bytes32(value) => {
                self.conn
                    .change_property32(PropMode::REPLACE, requestor, property, stored.property_type, value)
            }
        };
        result
            .map_err(|_| InjectError::Clipboard("failed to restore clipboard data"))?
            .check()
            .map_err(|_| InjectError::Clipboard("failed to restore clipboard data"))
    }

    fn send_selection_notify(&self, request: SelectionRequestEvent, property: Atom) -> Result<(), InjectError> {
        self.conn
            .send_event(
                false,
                request.requestor,
                EventMask::NO_EVENT,
                SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: request.sequence,
                    time: request.time,
                    requestor: request.requestor,
                    selection: request.selection,
                    target: request.target,
                    property,
                },
            )
            .map_err(|_| InjectError::Clipboard("failed to finish clipboard transfer"))?
            .check()
            .map_err(|_| InjectError::Clipboard("failed to finish clipboard transfer"))?;
        self.conn
            .flush()
            .map_err(|_| InjectError::Clipboard("failed to finish clipboard transfer"))
    }

    fn request_manager_handover(&self) {
        let _ = self.conn.convert_selection(
            self.window,
            self.atoms.CLIPBOARD_MANAGER,
            self.atoms.SAVE_TARGETS,
            NONE,
            Time::CURRENT_TIME,
        );
        let _ = self.conn.flush();
    }

    fn server_time(&self) -> Result<u32, InjectError> {
        self.conn
            .change_property8(
                PropMode::APPEND,
                self.window,
                self.atoms.HORIZON_CLIPBOARD_TIME,
                AtomEnum::INTEGER,
                &[0],
            )
            .map_err(|_| InjectError::Clipboard("failed to timestamp clipboard snapshot"))?
            .check()
            .map_err(|_| InjectError::Clipboard("failed to timestamp clipboard snapshot"))?;
        self.conn
            .flush()
            .map_err(|_| InjectError::Clipboard("failed to timestamp clipboard snapshot"))?;
        let deadline = Instant::now() + SNAPSHOT_TIMEOUT;
        loop {
            let event = self.wait_for_event(deadline)?;
            let Event::PropertyNotify(notify) = event else {
                continue;
            };
            if notify.window == self.window
                && notify.atom == self.atoms.HORIZON_CLIPBOARD_TIME
                && notify.state == Property::NEW_VALUE
            {
                return Ok(notify.time);
            }
        }
    }

    fn focus_identity(&self) -> Result<ClientIdentity, InjectError> {
        let focus = self
            .conn
            .get_input_focus()
            .map_err(|_| InjectError::Failed("failed to identify focused application"))?
            .reply()
            .map_err(|_| InjectError::Failed("failed to identify focused application"))?
            .focus;
        self.client_identity(focus).ok_or(InjectError::Failed(
            "no focused application is available for speech paste",
        ))
    }

    fn client_identity(&self, window: Window) -> Option<ClientIdentity> {
        if window <= 1 || window == self.window {
            return None;
        }
        let resource_mask = self.conn.setup().resource_id_mask;
        let resource_client = window & !resource_mask;
        let pid = self.xres_pid(window).or_else(|| self.window_pid_property(window));
        Some(ClientIdentity { resource_client, pid })
    }

    fn xres_pid(&self, window: Window) -> Option<u32> {
        let spec = ClientIdSpec {
            client: window,
            mask: ClientIdMask::LOCAL_CLIENT_PID,
        };
        self.conn
            .res_query_client_ids(&[spec])
            .ok()?
            .reply()
            .ok()?
            .ids
            .into_iter()
            .find_map(|id| id.value.first().copied())
    }

    fn window_pid_property(&self, mut window: Window) -> Option<u32> {
        for _ in 0..32 {
            if let Ok(cookie) = self
                .conn
                .get_property(false, window, self.atoms._NET_WM_PID, AtomEnum::CARDINAL, 0, 1)
                && let Ok(reply) = cookie.reply()
                && let Some(pid) = reply.value32().and_then(|mut values| values.next())
            {
                return Some(pid);
            }
            let tree = self.conn.query_tree(window).ok()?.reply().ok()?;
            if tree.parent == window || tree.parent == NONE {
                break;
            }
            window = tree.parent;
        }
        None
    }

    fn request_matches_target(&self, requestor: Window) -> bool {
        let Some(requestor) = self.client_identity(requestor) else {
            return false;
        };
        self.target_identity.is_some_and(|target| target.matches(requestor))
    }

    fn is_protocol_target(&self, target: Atom) -> bool {
        matches!(
            target,
            target if target == self.atoms.TARGETS
                || target == self.atoms.MULTIPLE
                || target == self.atoms.TIMESTAMP
                || target == self.atoms.SAVE_TARGETS
                || target == self.atoms.INCR
        )
    }

    fn is_utf8_target(&self, target: Atom) -> bool {
        target == self.atoms.UTF8_STRING
            || target == self.atoms.UTF8_MIME_0
            || target == self.atoms.UTF8_MIME_1
            || target == self.atoms.TEXT_MIME
    }

    fn selection_owner(&self, selection: Atom) -> Result<Window, InjectError> {
        self.conn
            .get_selection_owner(selection)
            .map_err(|_| InjectError::Clipboard("failed to inspect clipboard owner"))?
            .reply()
            .map(|reply| reply.owner)
            .map_err(|_| InjectError::Clipboard("failed to inspect clipboard owner"))
    }

    fn poll_event(&self) -> Result<Option<Event>, InjectError> {
        let event = self
            .conn
            .poll_for_event()
            .map_err(|_| InjectError::Clipboard("clipboard connection closed during speech paste"))?;
        if event.is_none() {
            thread::sleep(EVENT_POLL_INTERVAL);
        }
        Ok(event)
    }

    fn wait_for_event(&self, deadline: Instant) -> Result<Event, InjectError> {
        while Instant::now() < deadline {
            if let Some(event) = self.poll_event()? {
                return Ok(event);
            }
        }
        Err(InjectError::Clipboard(
            "timed out while preserving the existing clipboard",
        ))
    }
}
