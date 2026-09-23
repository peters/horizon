//! Claim Device panel requests without waiting for a presented frame.
//!
//! On Wayland, `request_redraw` produces `RedrawRequested` only after the
//! compositor's frame callback. A surface that is not being presented never
//! receives that callback. `winit` also reports neither visibility nor
//! minimization there, so eframe's hidden-window path does not run. The
//! repaint schedule is consumed once and the event loop sleeps with the
//! queue still on disk. A directory watcher wakes the UI thread through the
//! event-loop proxy; that wake is not a redraw.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::UserEvent;
use egui::{Context, ViewportId};
use horizon_core::browser::manifest::{self, device};
use winit::event_loop::EventLoopProxy;

use super::HorizonApp;

/// Pass number that cannot match a real egui pass. eframe then ignores the
/// wake instead of scheduling a redraw Wayland would drop.
pub(crate) const DEVICE_QUEUE_WAKE_PASS: u64 = u64::MAX;

const IDLE_WAIT: Duration = Duration::from_millis(100);
const PENDING_WAIT: Duration = Duration::from_millis(50);

struct InstalledHost {
    app: HorizonApp,
    ctx: Context,
}

pub(crate) struct DeviceRequestBridge {
    installed: Mutex<Option<InstalledHost>>,
    /// `None` follows the process runtime root. Tests pin a directory.
    root_override: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    watcher: Mutex<Option<JoinHandle<()>>>,
}

pub(crate) struct BridgeApp {
    bridge: Arc<DeviceRequestBridge>,
}

impl DeviceRequestBridge {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::build(None))
    }

    #[cfg(test)]
    pub(crate) fn with_root(root: PathBuf) -> Arc<Self> {
        Arc::new(Self::build(Some(root)))
    }

    fn build(root_override: Option<PathBuf>) -> Self {
        Self {
            installed: Mutex::new(None),
            root_override,
            stop: Arc::new(AtomicBool::new(false)),
            watcher: Mutex::new(None),
        }
    }

    pub(crate) fn install(&self, app: HorizonApp, ctx: Context) {
        *lock(&self.installed) = Some(InstalledHost { app, ctx });
    }

    /// Claim waiting requests on the UI thread. Also used when no frame is running.
    pub(crate) fn poll_on_ui_thread(&self) -> bool {
        let mut installed = lock(&self.installed);
        let Some(installed) = installed.as_mut() else {
            return false;
        };
        let changed = installed
            .app
            .drain_device_panel_requests(&installed.ctx, self.root_override.as_deref());
        if changed {
            installed.app.save_runtime_after_device_request();
        }
        changed
    }

    pub(crate) fn start_watcher(&self, proxy: EventLoopProxy<UserEvent>) {
        let mut watcher = lock(&self.watcher);
        if watcher.is_some() || self.stop.load(Ordering::Relaxed) {
            return;
        }
        let stop = Arc::clone(&self.stop);
        let root_override = self.root_override.clone();
        match thread::Builder::new()
            .name("horizon-device-requests".to_owned())
            .spawn(move || watch_device_requests(&stop, root_override.as_deref(), &proxy))
        {
            Ok(handle) => *watcher = Some(handle),
            Err(error) => tracing::error!(%error, "could not start Device request watcher"),
        }
    }

    pub(crate) fn stop_watcher(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let handle = lock(&self.watcher).take();
        if let Some(handle) = handle {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

impl Drop for DeviceRequestBridge {
    fn drop(&mut self) {
        self.stop_watcher();
    }
}

impl BridgeApp {
    pub(crate) fn new(bridge: Arc<DeviceRequestBridge>) -> Self {
        Self { bridge }
    }
}

impl eframe::App for BridgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let mut installed = lock(&self.bridge.installed);
        if let Some(installed) = installed.as_mut() {
            eframe::App::ui(&mut installed.app, ui, frame);
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        let installed = lock(&self.bridge.installed);
        installed.as_ref().map_or_else(
            || egui::Color32::from_rgb(12, 12, 12).to_normalized_gamma_f32(),
            |installed| eframe::App::clear_color(&installed.app, visuals),
        )
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let mut installed = lock(&self.bridge.installed);
        if let Some(installed) = installed.as_mut() {
            eframe::App::raw_input_hook(&mut installed.app, ctx, raw_input);
        }
    }

    fn on_exit(&mut self) {
        // `HorizonApp::on_exit` ends the process, which skips this bridge's
        // destructor. Stop the watcher first so it is not killed mid-claim.
        self.bridge.stop_watcher();
        let mut installed = lock(&self.bridge.installed);
        if let Some(installed) = installed.as_mut() {
            eframe::App::on_exit(&mut installed.app);
        }
    }
}

pub(crate) fn is_device_queue_wake(event: &UserEvent) -> bool {
    matches!(
        event,
        UserEvent::RequestRepaint {
            cumulative_pass_nr: DEVICE_QUEUE_WAKE_PASS,
            viewport_id,
            ..
        } if *viewport_id == ViewportId::ROOT
    )
}

fn watch_device_requests(stop: &AtomicBool, root_override: Option<&Path>, proxy: &EventLoopProxy<UserEvent>) {
    while !stop.load(Ordering::Relaxed) {
        let pending = match device_requests_pending(root_override) {
            Ok(pending) => pending,
            Err(error) => {
                tracing::warn!(%error, "could not check Device panel requests");
                thread::park_timeout(Duration::from_secs(1));
                continue;
            }
        };
        if pending
            && proxy
                .send_event(UserEvent::RequestRepaint {
                    viewport_id: ViewportId::ROOT,
                    when: Instant::now(),
                    cumulative_pass_nr: DEVICE_QUEUE_WAKE_PASS,
                })
                .is_err()
        {
            return;
        }
        thread::park_timeout(if pending { PENDING_WAIT } else { IDLE_WAIT });
    }
}

fn device_requests_pending(root: Option<&Path>) -> std::io::Result<bool> {
    let host = manifest::host_instance();
    match root {
        Some(root) => device::has_pending_at(root, host),
        None => device::has_pending(host),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_sentinel_pass_is_a_device_queue_wake() {
        let wake = UserEvent::RequestRepaint {
            viewport_id: ViewportId::ROOT,
            when: Instant::now(),
            cumulative_pass_nr: DEVICE_QUEUE_WAKE_PASS,
        };
        assert!(is_device_queue_wake(&wake));
        let repaint = UserEvent::RequestRepaint {
            viewport_id: ViewportId::ROOT,
            when: Instant::now(),
            cumulative_pass_nr: 3,
        };
        assert!(!is_device_queue_wake(&repaint));
    }
}
