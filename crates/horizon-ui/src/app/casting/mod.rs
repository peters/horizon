mod capture;
mod controls;
mod notifications;
mod requests;
mod scaling;

use horizon_cast::{CastSession, PairedDevice, PairingStore, Receiver};
use horizon_core::WorkspaceId;
use horizon_core::browser::manifest::cast::{CastOrientation, CastResolution, CastSource};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct CastState {
    pub(super) pairing_directory: std::path::PathBuf,
    paired_receivers: Vec<PairedDevice>,
    paired_refresh: Option<mpsc::Receiver<Result<Vec<PairedDevice>, String>>>,
    paired_refresh_pending: bool,
    pairing_error: Option<String>,
    receivers: Vec<Receiver>,
    discovery: Option<mpsc::Receiver<Result<Vec<Receiver>, String>>>,
    discovery_error: Option<String>,
    sessions: Vec<Session>,
    retiring: Vec<Session>,
    picker: Option<Picker>,
    control_menus: Option<[egui::LayerId; 2]>,
    notice: Option<String>,
    notification: Option<String>,
    last_capture: Option<Instant>,
}
struct Session {
    generation: Instant,
    receiver_id: String,
    workspace: WorkspaceId,
    source: CastSource,
    orientation: CastOrientation,
    resolution: CastResolution,
    worker: CastSession,
    scaling: Option<scaling::Scaler>,
    failure_notified: bool,
}
struct Picker {
    workspace: WorkspaceId,
    source: CastSource,
    receiver: Option<String>,
    orientation: CastOrientation,
    resolution: CastResolution,
    pin: zeroize::Zeroizing<String>,
}
impl CastState {
    pub(super) fn new(pairing_directory: std::path::PathBuf) -> Self {
        let mut state = Self {
            pairing_directory,
            ..Self::default()
        };
        state.refresh_pairings();
        state
    }
    pub(super) fn refresh_pairings(&mut self) {
        if self.paired_refresh.is_some() {
            self.paired_refresh_pending = true;
            return;
        }
        let directory = self.pairing_directory.clone();
        let (send, receive) = mpsc::channel();
        self.paired_refresh = Some(receive);
        std::thread::spawn(move || {
            let _ = send.send(PairingStore::list(&directory).map_err(|error| error.to_string()));
        });
    }
    pub(super) fn pairing_loading(&self) -> bool {
        self.paired_refresh.is_some()
    }
    pub(super) fn pairing_error(&self) -> Option<&str> {
        self.pairing_error.as_deref()
    }
    pub(super) fn paired_devices(&self) -> &[PairedDevice] {
        &self.paired_receivers
    }
    pub(super) fn receiver_available(&self, id: &str) -> bool {
        self.receivers.iter().any(|receiver| receiver.id == id)
    }
    pub(super) fn receiver_busy(&self, id: &str) -> bool {
        self.sessions
            .iter()
            .chain(&self.retiring)
            .any(|session| session.receiver_id == id && !session.worker.finished())
    }
    fn forget_pairing(&mut self, receiver_id: &str) -> Result<(), String> {
        if self.receiver_busy(receiver_id) {
            return Err("Stop this TV's session before forgetting its pairing".into());
        }
        PairingStore::new(self.pairing_directory.clone(), receiver_id.to_owned(), String::new())
            .forget()
            .map_err(|error| error.to_string())?;
        self.paired_receivers.retain(|receiver| receiver.id != receiver_id);
        // Discard a pre-delete snapshot so it cannot restore a forgotten row.
        self.paired_refresh = None;
        self.paired_refresh_pending = false;
        self.refresh_pairings();
        self.notice = None;
        Ok(())
    }
    pub(super) fn stop_all(&mut self) {
        self.picker = None;
        for session in self.sessions.iter_mut().chain(&mut self.retiring) {
            session.scaling = None;
            session.worker.stop();
        }
    }
    pub(super) fn reset_for_session_switch(&mut self) {
        self.stop_all();
        self.retiring.append(&mut self.sessions);
        self.discovery = None;
        self.discovery_error = None;
        self.control_menus = None;
        self.receivers.clear();
        self.notice = None;
        self.notification = None;
        self.last_capture = None;
        self.refresh_pairings();
    }
    pub(super) fn finished(&self) -> bool {
        self.sessions
            .iter()
            .chain(&self.retiring)
            .all(|session| session.worker.finished())
    }
    pub(super) fn stop_and_wait(&mut self, timeout: Duration) -> bool {
        self.stop_all();
        let started = Instant::now();
        while !self.finished() {
            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                return false;
            };
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        }
        true
    }
    pub(super) fn picker_open(&self) -> bool {
        self.picker.is_some()
    }
    fn notify(&mut self, message: String) {
        self.notice = Some(message.clone());
        self.notification = Some(message);
    }
    fn discover(&mut self) {
        if self.discovery.is_some() {
            return;
        }
        let (send, receive) = mpsc::channel();
        self.discovery = Some(receive);
        self.discovery_error = None;
        std::thread::spawn(move || {
            let _ = send.send(horizon_cast::discover().map_err(|error| error.to_string()));
        });
    }
    fn poll(&mut self) {
        for session in &mut self.sessions {
            if session.worker.finished() {
                session.scaling = None;
                session.worker.reap();
            }
            if !session.failure_notified
                && let horizon_cast::CastStatus::Failed(error) = session.worker.status()
            {
                session.failure_notified = true;
                self.notice = Some(error.clone());
                self.notification = Some(error);
            }
        }
        let mut saved = false;
        for session in self.sessions.iter().chain(&self.retiring) {
            saved |= session.worker.take_pairing_saved();
        }
        self.retiring.retain(|session| !session.worker.finished());
        if saved {
            self.refresh_pairings();
        }
        if let Some(result) = self.paired_refresh.as_ref().and_then(|receive| receive.try_recv().ok()) {
            self.paired_refresh = None;
            match result {
                Ok(receivers) => {
                    self.paired_receivers = receivers;
                    self.pairing_error = None;
                }
                Err(error) => {
                    self.pairing_error = Some(error.clone());
                    self.notify(error);
                }
            }
            if std::mem::take(&mut self.paired_refresh_pending) {
                self.refresh_pairings();
            }
        }
        if let Some(result) = self.discovery.as_ref().and_then(|receive| receive.try_recv().ok()) {
            self.discovery = None;
            match result {
                Ok(receivers) => {
                    self.receivers = receivers;
                    self.discovery_error = None;
                }
                Err(error) => {
                    self.discovery_error = Some(error.clone());
                    self.notify(error);
                }
            }
        }
    }
}

fn video_format(orientation: CastOrientation, resolution: CastResolution) -> horizon_cast::VideoFormat {
    horizon_cast::VideoFormat {
        orientation: match orientation {
            CastOrientation::Landscape => horizon_cast::Orientation::Landscape,
            CastOrientation::Portrait => horizon_cast::Orientation::Portrait,
        },
        resolution: match resolution {
            CastResolution::Hd720 => horizon_cast::Resolution::Hd720,
            CastResolution::FullHd1080 => horizon_cast::Resolution::FullHd1080,
            CastResolution::Uhd4k => horizon_cast::Resolution::Uhd4k,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_cast::CastStatus;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[test]
    fn session_switch_retains_pairing_worker_until_synchronous_exit() {
        let listener = TcpListener::bind("127.0.0.99:0").expect("synthetic listener");
        let address = listener.local_addr().expect("address");
        let receiver = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            socket.set_read_timeout(Some(Duration::from_secs(3))).expect("timeout");
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).expect("pair request");
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"POST /pair-pin-start "));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                .expect("reply");
            assert_eq!(socket.read(&mut byte).expect("connection closed"), 0);
        });
        let (_temp, mut app) = crate::app::test_support::test_app();
        let worker = CastSession::start(address, horizon_cast::VideoFormat::default()).expect("cast");
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.status() != CastStatus::PinRequired {
            assert!(Instant::now() < deadline, "pairing prompt: {:?}", worker.status());
            std::thread::sleep(Duration::from_millis(10));
        }
        app.casting.sessions.push(Session {
            generation: Instant::now(),
            receiver_id: "synthetic".into(),
            workspace: WorkspaceId(1),
            source: CastSource::Panel { id: "synthetic".into() },
            orientation: CastOrientation::Landscape,
            resolution: CastResolution::default(),
            worker,
            scaling: Some(scaling::Scaler::new((1280, 720)).expect("scaler")),
            failure_notified: false,
        });
        app.casting.reset_for_session_switch();
        assert!(app.casting.sessions.is_empty());
        assert_eq!(app.casting.retiring.len(), 1);
        app.run_exit_cleanup();
        assert!(app.casting.finished());
        assert_eq!(app.casting.retiring[0].worker.status(), CastStatus::Stopped);
        receiver.join().expect("receiver");
    }

    #[test]
    fn completed_session_releases_scaler_and_preserves_failure_status() {
        let listener = TcpListener::bind("127.0.0.98:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let receiver = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            socket.set_read_timeout(Some(Duration::from_secs(2))).expect("timeout");
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).expect("request");
                request.push(byte[0]);
            }
            socket
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .expect("reject");
        });
        let worker = CastSession::start(address, horizon_cast::VideoFormat::default()).expect("worker");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.casting.sessions.push(Session {
            generation: Instant::now(),
            receiver_id: "synthetic".into(),
            workspace: WorkspaceId(1),
            source: CastSource::Panel { id: "synthetic".into() },
            orientation: CastOrientation::Landscape,
            resolution: CastResolution::default(),
            worker,
            scaling: Some(scaling::Scaler::new((8, 8)).expect("scaler")),
            failure_notified: false,
        });
        app.casting.poll();
        assert!(app.casting.sessions[0].scaling.is_none());
        assert!(!app.casting.receiver_busy("synthetic"));
        let status = app.cast_snapshot(WorkspaceId(1), &egui::Context::default());
        assert_eq!(status.sessions[0].state, "failed");
        assert!(
            status.sessions[0]
                .error
                .as_ref()
                .is_some_and(|error| error.contains("403"))
        );
        assert!(app.casting.notification.is_some());
        receiver.join().expect("receiver");
    }

    #[test]
    fn discovery_failure_is_shared_and_successful_refresh_clears_it() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        for result in [Err("synthetic discovery failure".to_string()), Ok(Vec::new())] {
            let expected = result.as_ref().err().cloned();
            let (send, receive) = mpsc::channel();
            app.casting.discovery = Some(receive);
            send.send(result).expect("discovery result");
            let outcome = app.cast_operation(
                WorkspaceId(1),
                &horizon_core::browser::manifest::cast::CastOperation::Status,
                &ctx,
            );
            assert_eq!(outcome.error, expected);
            let stop = app.cast_operation(
                WorkspaceId(1),
                &horizon_core::browser::manifest::cast::CastOperation::Stop {
                    receiver_id: "absent".into(),
                },
                &ctx,
            );
            assert!(stop.error.is_none(), "unrelated successful stop");
        }
    }
}
