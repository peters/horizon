mod capture;
mod controls;
mod requests;

use horizon_cast::{CastSession, PairedDevice, PairingStore, Receiver};
use horizon_core::WorkspaceId;
use horizon_core::browser::manifest::cast::{CastOrientation, CastResolution, CastSource};
use std::{sync::mpsc, time::Instant};

#[derive(Default)]
pub(super) struct CastState {
    pub(super) pairing_directory: std::path::PathBuf,
    paired_receivers: Vec<PairedDevice>,
    paired_refresh: Option<mpsc::Receiver<Result<Vec<PairedDevice>, String>>>,
    paired_refresh_pending: bool,
    pairing_error: Option<String>,
    receivers: Vec<Receiver>,
    discovery: Option<mpsc::Receiver<Result<Vec<Receiver>, String>>>,
    sessions: Vec<Session>,
    picker: Option<Picker>,
    notice: Option<String>,
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
        for session in &self.sessions {
            session.worker.stop();
        }
    }
    pub(super) fn finished(&self) -> bool {
        self.sessions.iter().all(|session| session.worker.finished())
    }
    pub(super) fn picker_open(&self) -> bool {
        self.picker.is_some()
    }
    fn discover(&mut self) {
        if self.discovery.is_some() {
            return;
        }
        let (send, receive) = mpsc::channel();
        self.discovery = Some(receive);
        std::thread::spawn(move || {
            let _ = send.send(horizon_cast::discover().map_err(|error| error.to_string()));
        });
    }
    fn poll(&mut self) {
        let mut saved = false;
        for session in &self.sessions {
            saved |= session.worker.take_pairing_saved();
        }
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
                    self.notice = Some(error);
                }
            }
            if std::mem::take(&mut self.paired_refresh_pending) {
                self.refresh_pairings();
            }
        }
        if let Some(result) = self.discovery.as_ref().and_then(|receive| receive.try_recv().ok()) {
            self.discovery = None;
            match result {
                Ok(receivers) => self.receivers = receivers,
                Err(error) => self.notice = Some(error),
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
