//! A browser process shared by independently controlled page drivers.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::cdp::CdpLink;
use crate::process::{ChromeProcess, ChromeProcessControl, ProcessLifecycle};

/// Host-owned session identity. Clone this handle to open another page in the
/// same Chromium profile. Each driver still owns its own protocol connection.
#[derive(Clone)]
pub struct SharedBrowserSession {
    profile_id: String,
    state: Arc<Mutex<SharedState>>,
    drivers: Arc<AtomicUsize>,
    stops: Arc<Mutex<Vec<Weak<AtomicBool>>>>,
}

#[derive(Default)]
struct SharedState {
    process: Option<ChromeProcess>,
    control: ChromeProcessControl,
    endpoint: String,
    pages: usize,
    retiring: bool,
    profile_retired: bool,
    launch_identity: Option<SharedLaunchIdentity>,
}

#[derive(Clone, PartialEq, Eq)]
struct SharedLaunchIdentity {
    profile: std::path::PathBuf,
    command: String,
    extra_args: Vec<String>,
    headless: bool,
    disclosure: crate::AutomationDisclosurePolicy,
}

impl SharedState {
    fn close_browser(&mut self, connection: Option<&mut CdpLink>) -> bool {
        self.retiring = true;
        let mut reconnect = if connection.is_none() && self.process.is_some() {
            CdpLink::connect(&self.endpoint).ok()
        } else {
            None
        };
        if let Some(link) = connection.or(reconnect.as_mut()) {
            let _ = link.call_and_drain(Duration::from_secs(1), "Browser.close", &serde_json::json!({}), None);
        }
        // Let Chromium flush the profile before using the bounded kill fallback.
        let deadline = Instant::now() + Duration::from_secs(1);
        while self
            .process
            .as_mut()
            .is_some_and(|process| process.child_status().is_none())
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let reaped = match self.process.as_mut() {
            Some(process) => process.kill(),
            None => self.control.is_reaped() || self.control.terminate(Duration::from_secs(1)),
        };
        if reaped {
            self.process = None;
        }
        reaped
    }
}

impl std::fmt::Debug for SharedBrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBrowserSession")
            .field("profile_id", &self.profile_id)
            .finish_non_exhaustive()
    }
}

impl SharedBrowserSession {
    #[must_use]
    pub fn new(profile_id: String) -> Self {
        Self {
            profile_id,
            state: Arc::default(),
            drivers: Arc::default(),
            stops: Arc::default(),
        }
    }

    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        let Ok(state) = self.state.try_lock() else {
            return false;
        };
        self.drivers.load(Ordering::Acquire) == 0 && state.pages == 0 && state.control.is_reaped()
    }

    /// Permanently prevent new page acquisition before profile removal begins.
    #[must_use]
    pub fn retire_profile_for_cleanup(&self) -> bool {
        let Ok(mut state) = self.state.try_lock() else {
            return false;
        };
        if state.profile_retired {
            return true;
        }
        if self.drivers.load(Ordering::Acquire) != 0 || state.pages != 0 || !state.control.is_reaped() {
            return false;
        }
        state.profile_retired = true;
        true
    }

    pub(super) fn reserve(self, stop: Arc<AtomicBool>) -> SharedDriverReservation {
        {
            let mut stops = self.stops.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            stops.retain(|stop| stop.strong_count() > 0);
            stops.push(Arc::downgrade(&stop));
        }
        self.drivers.fetch_add(1, Ordering::AcqRel);
        SharedDriverReservation { session: self, stop }
    }

    pub(super) fn acquire(
        &self,
        launch: &crate::process::ChromeLaunch,
        stop: &AtomicBool,
        panel_control: &ChromeProcessControl,
    ) -> Result<Option<(DriverProcess, String)>, String> {
        self.pin_launch(launch)?;
        self.acquire_with(stop, panel_control, |control| {
            super::startup::start_chrome(launch, stop, control)
        })
    }

    fn pin_launch(&self, launch: &crate::process::ChromeLaunch) -> Result<(), String> {
        let identity = SharedLaunchIdentity {
            profile: std::path::absolute(&launch.profile_dir).map_err(|error| error.to_string())?,
            command: launch.command.clone(),
            extra_args: launch.extra_args.clone(),
            headless: launch.headless,
            disclosure: launch.automation_disclosure,
        };
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.launch_identity.as_ref().is_some_and(|pinned| pinned != &identity) {
            return Err(
                "shared browser process configuration does not match its pinned profile and launch settings".into(),
            );
        }
        state.launch_identity.get_or_insert(identity);
        Ok(())
    }

    fn acquire_with(
        &self,
        stop: &AtomicBool,
        panel_control: &ChromeProcessControl,
        start: impl FnOnce(&ChromeProcessControl) -> Result<Option<(ChromeProcess, String)>, String>,
    ) -> Result<Option<(DriverProcess, String)>, String> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.profile_retired {
            return Err("shared browser profile has been retired for deletion".into());
        }
        if stop.load(Ordering::Acquire) {
            return Ok(None);
        }
        let alive = state
            .process
            .as_mut()
            .is_some_and(|process| process.child_status().is_none());
        if state.retiring && alive {
            return Err("previous shared browser has not finished stopping".into());
        }
        if state.pages == 0 && !alive {
            if state.process.as_mut().is_some_and(|process| !process.kill()) || !state.control.is_reaped() {
                return Err("previous shared browser has not released its profile".into());
            }
            state.process = None;
            state.control = ChromeProcessControl::default();
            if panel_control.delegate(Arc::new(StartupLifecycle(state.control.clone()))) {
                let _ = state.control.terminate(Duration::ZERO);
            }
            state.retiring = true;
            let started = start(&state.control);
            state.control.mark_registration_settled();
            let Some((process, endpoint)) = started? else {
                return Ok(None);
            };
            state.retiring = false;
            state.process = Some(process);
            state.endpoint = endpoint;
        } else if state
            .process
            .as_mut()
            .is_none_or(|process| process.child_status().is_some())
        {
            return Err("shared browser stopped; wait for its panels to stop before retrying".into());
        }
        state.pages += 1;
        let lifecycle = Arc::new(PageLifecycle {
            state: Arc::clone(&self.state),
            target: Mutex::new(None),
            closing: AtomicBool::new(false),
            released: AtomicBool::new(false),
            last_page: AtomicBool::new(false),
            process: state.control.clone(),
            drivers: Arc::clone(&self.drivers),
            stops: Arc::clone(&self.stops),
        });
        let force_requested = panel_control.delegate(lifecycle.clone());
        let endpoint = state.endpoint.clone();
        drop(state);
        if force_requested {
            let _ = panel_control.terminate(Duration::from_secs(1));
        }
        Ok(Some((DriverProcess::Shared(lifecycle), endpoint)))
    }
}

pub(super) struct SharedDriverReservation {
    pub(super) session: SharedBrowserSession,
    stop: Arc<AtomicBool>,
}

impl Drop for SharedDriverReservation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if self.session.drivers.fetch_sub(1, Ordering::AcqRel) == 1 {
            let mut state = self
                .session
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A new driver may have acquired a page while this drop waited
            // for the state lock. Reservations arriving after this check cannot
            // acquire a page until this process has finished retiring.
            if self.session.drivers.load(Ordering::Acquire) == 0 {
                state.close_browser(None);
            }
        }
    }
}

struct StartupLifecycle(ChromeProcessControl);

impl ProcessLifecycle for StartupLifecycle {
    fn is_reaped(&self) -> bool {
        self.0.is_reaped()
    }

    fn terminate(&self, timeout: Duration) -> bool {
        self.0.terminate(timeout)
    }
}

pub(crate) struct PageLifecycle {
    state: Arc<Mutex<SharedState>>,
    target: Mutex<Option<String>>,
    closing: AtomicBool,
    released: AtomicBool,
    last_page: AtomicBool,
    process: ChromeProcessControl,
    drivers: Arc<AtomicUsize>,
    stops: Arc<Mutex<Vec<Weak<AtomicBool>>>>,
}

impl PageLifecycle {
    fn release(&self, mut connection: Option<(&mut CdpLink, &str)>) -> bool {
        self.closing.store(true, Ordering::Release);
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.released.load(Ordering::Acquire) {
            return !self.last_page.load(Ordering::Acquire) || self.process.is_reaped();
        }
        let mut target = self.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, id)) = connection.as_ref() {
            *target = Some((*id).to_string());
        }
        if let Some(id) = target.as_deref() {
            let closed = connection.as_mut().is_some_and(|(link, _)| close_target(link, id));
            let closed = closed || CdpLink::connect(&state.endpoint).is_ok_and(|mut link| close_target(&mut link, id));
            if !closed && !self.process.is_reaped() {
                return false;
            }
        }
        state.pages -= 1;
        let last = state.pages == 0 && self.drivers.load(Ordering::Acquire) == 1;
        self.last_page.store(last, Ordering::Release);
        let reaped = !last || state.close_browser(connection.map(|(link, _)| link));
        self.released.store(true, Ordering::Release);
        reaped
    }
}

fn close_target(link: &mut CdpLink, target: &str) -> bool {
    if link
        .call_and_drain(
            Duration::from_secs(1),
            "Target.closeTarget",
            &serde_json::json!({"targetId": target}),
            None,
        )
        .result
        .is_ok_and(|result| result["success"].as_bool() == Some(true))
    {
        return true;
    }
    // A disconnected response can still have closed the target. Verify absence
    // before declaring success; errors and malformed replies remain pending.
    link.call_and_drain(
        Duration::from_secs(1),
        "Target.getTargets",
        &serde_json::json!({}),
        None,
    )
    .result
    .ok()
    .and_then(|result| result["targetInfos"].as_array().cloned())
    .is_some_and(|targets| {
        targets
            .iter()
            .all(|info| info["targetId"].as_str().is_some_and(|id| id != target))
    })
}

impl ProcessLifecycle for PageLifecycle {
    fn is_reaped(&self) -> bool {
        if self.closing.load(Ordering::Acquire) && !self.released.load(Ordering::Acquire) && self.process.is_reaped() {
            let Ok(mut state) = self.state.try_lock() else {
                return false;
            };
            if !self.released.swap(true, Ordering::AcqRel) {
                state.pages -= 1;
            }
        }
        self.released.load(Ordering::Acquire)
            && (!(self.last_page.load(Ordering::Acquire) || self.drivers.load(Ordering::Acquire) == 0)
                || self.process.is_reaped())
    }

    fn terminate(&self, timeout: Duration) -> bool {
        let Ok(state) = self.state.try_lock() else {
            return false;
        };
        if self.released.load(Ordering::Acquire) {
            return !(self.last_page.load(Ordering::Acquire) || self.drivers.load(Ordering::Acquire) == 0)
                || self.process.terminate(timeout);
        }
        // A panel's emergency teardown must never kill a sibling's browser.
        let Ok(stops) = self.stops.try_lock() else {
            return false;
        };
        let all_stopping = stops
            .iter()
            .filter_map(Weak::upgrade)
            .all(|stop| stop.load(Ordering::Acquire));
        (all_stopping || (state.pages == 1 && self.drivers.load(Ordering::Acquire) == 1))
            && self.process.terminate(timeout)
    }
}

pub(super) enum DriverProcess {
    Exclusive(ChromeProcess),
    Shared(Arc<PageLifecycle>),
}

impl DriverProcess {
    pub(super) fn is_shared(&self) -> bool {
        matches!(self, Self::Shared(_))
    }

    pub(super) fn child_status(&mut self) -> Option<std::process::ExitStatus> {
        match self {
            Self::Exclusive(process) => process.child_status(),
            Self::Shared(page) => page
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .process
                .as_mut()
                .and_then(ChromeProcess::child_status),
        }
    }

    pub(super) fn register_target(&self, target: &str) {
        if let Self::Shared(page) = self {
            *page.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(target.to_string());
        }
    }

    pub(super) fn close_page(&mut self, link: &mut CdpLink, target: &str) {
        match self {
            Self::Exclusive(process) => {
                let _ = link.call_and_drain(Duration::from_secs(1), "Browser.close", &serde_json::json!({}), None);
                let _ = process.kill();
            }
            Self::Shared(page) => {
                page.release(Some((link, target)));
            }
        }
    }

    pub(super) fn kill(&mut self) -> bool {
        match self {
            Self::Exclusive(process) => process.kill(),
            Self::Shared(page) => page.release(None),
        }
    }
}

impl Drop for DriverProcess {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;
