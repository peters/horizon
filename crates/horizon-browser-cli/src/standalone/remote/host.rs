//! The host side of a standalone remote host: it claims the private requests
//! of the MCP server in this process, starts and stops the remote sessions,
//! and publishes their panels into the host's one workspace.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use horizon_browser::{
    BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, BrowserShutdownSignal, FrameSlot,
    RemoteReleaseOutcome, RemoteSessionEvent, VideoCaptureHandle, start_session,
};
use horizon_browser_control::manifest::{
    self, AgentIdentity, BrowserCloseRequest, BrowserCloseResult, BrowserCreateRequest, BrowserCreateResult,
    BrowserVisibilityResult, CreateNavigation, ManifestCoordination, ManifestWorkspace,
};
use horizon_browser_control::remote_slots::{self, SlotError, SlotLease};

use super::config::HostConfig;
use super::holds::{Holds, WORKSPACE};
use super::usage::Usage;

/// At most this many remote sessions run in one host.
const MAX_SESSIONS: usize = 16;
const EXIT_RELEASE_TIMEOUT: Duration = Duration::from_secs(30);
const FORCED_EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an exit waits for the provider checks of unconfirmed releases.
const EXIT_CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// Codes that a request failure keeps from its message.
const CODES: &[&str] = &[
    "target_unknown",
    "target_invalid",
    "provider_unknown",
    "provider_catalog_refresh_required",
    "credentials_not_ready",
    "credentials_invalid",
];

struct Hosted {
    session: BrowserSession,
    visible: bool,
    ready: bool,
    lost: bool,
    url: bool,
    warning: Option<String>,
    orientation: Option<(&'static str, RemoteReleaseOutcome)>,
    lease: Option<SlotLease>,
}

struct Stopping {
    signal: BrowserShutdownSignal,
    close: Option<BrowserCloseRequest>,
    lease: Option<SlotLease>,
}

pub struct RemoteHost {
    config: Arc<HostConfig>,
    actor: String,
    host: String,
    root: PathBuf,
    sessions: BTreeMap<String, Hosted>,
    pending: Vec<BrowserCreateRequest>,
    stopping: BTreeMap<String, Stopping>,
    /// Provider slots of stopped sessions whose release is not confirmed yet.
    retained: Vec<(horizon_browser::RemoteAllocation, SlotLease)>,
    holds: Holds,
    usage: Usage,
}

impl RemoteHost {
    /// # Errors
    /// The hold folder of this host cannot be created or locked.
    pub fn open(config: HostConfig, actor: String, root: PathBuf) -> std::io::Result<Self> {
        let host = manifest::host_instance().to_owned();
        let holds = Holds::open(&root.join("standalone-remote-holds"), &host, &actor, &config)?;
        // A taken-over session may still run at the provider, so it keeps a provider slot.
        let retained = holds
            .adopted()
            .into_iter()
            .filter_map(|(provider, allocation)| {
                let profile = config.provider(&provider).ok()?;
                let max_sessions = profile.local_session_limit()?;
                let key = horizon_browser_control::remote_slots::quota_key(profile);
                Some((allocation, remote_slots::acquire_slot(&root, &key, max_sessions).ok()?))
            })
            .collect();
        Ok(Self {
            config: Arc::new(config),
            actor,
            host,
            root,
            sessions: BTreeMap::new(),
            pending: Vec::new(),
            stopping: BTreeMap::new(),
            retained,
            holds,
            usage: Usage::default(),
        })
    }

    fn workspace(&self) -> ManifestWorkspace {
        ManifestWorkspace::new(&self.host, WORKSPACE, vec![self.actor.clone()])
    }

    /// One pass over every queue and every session.
    pub fn tick(&mut self) {
        self.drain();
        self.creates();
        self.pending();
        self.visibility();
        self.closes();
        self.stopped();
        let workspace = self.workspace();
        for (id, hosted) in &self.sessions {
            let _ = manifest::sync_host_state(id, hosted.visible, &workspace);
        }
        self.holds.answer();
        self.usage.answer(&self.config, &self.host, &self.actor);
        if let Ok(requests) = manifest::cast::claim(&self.host) {
            for request in requests {
                let outcome = manifest::cast::CastOutcome::failed("cast_unavailable: requires a Horizon host");
                let _ = manifest::cast::complete(&request, outcome);
            }
        }
        if let Ok(requests) = manifest::device::claim(&self.host) {
            for request in requests {
                let outcome = manifest::device::Outcome::failed(
                    "viewer_requires_horizon",
                    "A standalone host has no Horizon window to show a device viewer.",
                );
                let _ = manifest::device::complete(&request, outcome);
            }
        }
    }

    /// Stop every session and wait for each release.
    /// Returns whether every session stopped and released. Holds taken over
    /// from an earlier host stay on disk for the next host.
    pub fn shutdown(mut self) -> bool {
        let ids: Vec<_> = self.sessions.keys().cloned().collect();
        for id in ids {
            self.begin_stop(&id, None);
        }
        for pending in std::mem::take(&mut self.pending) {
            let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                &pending,
                "browser_start_failed",
                "the standalone host is stopping",
            ));
        }
        // Every driver got its stop above and releases in parallel, so one
        // deadline bounds them all.
        let deadline = std::time::Instant::now() + EXIT_RELEASE_TIMEOUT;
        let mut stopped = true;
        for stopping in self.stopping.values() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            stopped &= stopping.signal.wait(left) || stopping.signal.force_cleanup(FORCED_EXIT_TIMEOUT);
        }
        self.stopped();
        let checks = std::time::Instant::now() + EXIT_CHECK_TIMEOUT;
        while self.holds.checking() && std::time::Instant::now() < checks {
            std::thread::sleep(Duration::from_millis(100));
        }
        self.holds.prune();
        stopped && !self.holds.own_unreleased()
    }

    fn drain(&mut self) {
        let mut lost = Vec::new();
        for (id, hosted) in &mut self.sessions {
            for event in hosted.session.event_rx.try_iter() {
                match event {
                    BrowserEvent::Ready => hosted.ready = true,
                    BrowserEvent::UrlChanged(url) => hosted.url = !url.is_empty(),
                    BrowserEvent::Warning(message) => hosted.warning = Some(message),
                    BrowserEvent::RemoteSession(RemoteSessionEvent::OrientationRejected { code, released, .. }) => {
                        hosted.orientation = Some((code, released));
                    }
                    BrowserEvent::Stopped { .. } => hosted.lost = true,
                    BrowserEvent::Frame { .. } => hosted.session.frame_slot.release_notification(),
                    _ => {}
                }
            }
            if hosted.lost && !self.pending.iter().any(|pending| panel_id(pending) == *id) {
                lost.push(id.clone());
            }
        }
        for id in lost {
            self.begin_stop(&id, None);
        }
    }

    fn creates(&mut self) {
        let Ok(mut requests) = manifest::list_create_requests() else {
            return;
        };
        requests.retain(|request| request.actor == self.actor);
        for request in requests {
            let Ok(Some(request)) =
                manifest::claim_create_request(&request.request_id, &request.actor, &self.host, std::process::id())
            else {
                continue;
            };
            match self.start(&request) {
                Ok(()) => self.pending.push(request),
                Err((code, message)) => {
                    let _ = manifest::complete_create_request(&BrowserCreateResult::failed(&request, &code, &message));
                }
            }
        }
    }

    fn start(&mut self, request: &BrowserCreateRequest) -> Result<(), (String, String)> {
        let fail = |code: &str, message: &str| Err((code.to_owned(), message.to_owned()));
        if request.duplicate_from.is_some() {
            return fail(
                "unsupported_backend",
                "a standalone host does not duplicate remote sessions",
            );
        }
        let Some(target) = request.target.as_deref() else {
            return fail(
                "target_required",
                "this standalone host serves only remote targets; name a configured target or a provider device",
            );
        };
        if manifest::now_millis() >= request.deadline_at_millis {
            return fail("browser_start_failed", "Browser creation deadline expired");
        }
        if self.sessions.len() + self.stopping.len() >= MAX_SESSIONS {
            return fail(
                "remote_session_limit_reached",
                "this standalone host already runs its maximum of sessions",
            );
        }
        let mut remote = self
            .config
            .request(target, &self.usage.catalog)
            .map_err(|error| coded(&error.to_string()))?;
        horizon_browser::remote_config::override_orientation(&mut remote, request.orientation);
        let lease = self.lease(&remote)?;
        let id = panel_id(request);
        self.holds.insert(&id, &remote).map_err(|error| {
            (
                "browser_start_failed".to_owned(),
                format!("could not save the remote hold: {error}"),
            )
        })?;
        let coordination = ManifestCoordination::with_remote_allocation(Some(remote.recovery.clone()));
        let browser = BrowserConfig {
            backend: remote.browser,
            headless: true,
            ..BrowserConfig::default()
        };
        let started = start_session(BrowserSessionConfig {
            browser,
            panel_local_id: id.clone(),
            initial_url: request.url.clone(),
            width: horizon_browser::DEFAULT_VIEWPORT.0,
            height: horizon_browser::DEFAULT_VIEWPORT.1,
            frame_slot: Arc::new(FrameSlot::new()),
            coordination: Some(Arc::new(coordination)),
            capture_directory: Some(
                self.root
                    .join("standalone-remote-captures")
                    .join(horizon_browser_control::paths::safe_local_id(&id)),
            ),
            video: Arc::new(VideoCaptureHandle::default()),
            remote: Some(remote),
        });
        let session = match started {
            Ok(session) => session,
            Err(error) => {
                self.holds.cancel(&id);
                return Err(("browser_start_failed".to_owned(), error.to_string()));
            }
        };
        self.sessions.insert(
            id,
            Hosted {
                session,
                visible: request.visible,
                ready: false,
                lost: false,
                url: false,
                warning: None,
                orientation: None,
                lease,
            },
        );
        Ok(())
    }

    /// A slot of the provider's local session limit, shared with every host
    /// on this computer that uses the same provider identity.
    fn lease(&self, remote: &horizon_browser::RemoteSessionRequest) -> Result<Option<SlotLease>, (String, String)> {
        let Some(max_sessions) = self
            .config
            .provider(&remote.provider)
            .ok()
            .and_then(horizon_browser::remote::RemoteProviderProfile::local_session_limit)
        else {
            return Ok(None);
        };
        match remote_slots::acquire_slot(&self.root, &remote.quota_key, max_sessions) {
            Ok(lease) => Ok(Some(lease)),
            Err(SlotError::Busy { .. }) => Err((
                "remote_session_limit_reached".into(),
                "the provider's configured max_sessions are held by hosts on this computer".into(),
            )),
            Err(SlotError::Contended) => Err((
                "remote_quota_contended".into(),
                "another host on this computer was checking the same provider quota; create again".into(),
            )),
            Err(error) => Err(("browser_start_failed".into(), error.to_string())),
        }
    }

    fn pending(&mut self) {
        let workspace = self.workspace();
        for request in std::mem::take(&mut self.pending) {
            let id = panel_id(&request);
            let Some(hosted) = self.sessions.get(&id) else {
                let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                    &request,
                    "browser_start_failed",
                    "the remote session stopped",
                ));
                continue;
            };
            let result = if let Some((code, released)) = &hosted.orientation {
                Some(BrowserCreateResult::failed(
                    &request,
                    code,
                    orientation_message(released),
                ))
            } else if hosted.lost || manifest::now_millis() >= request.deadline_at_millis {
                let message = hosted
                    .warning
                    .clone()
                    .unwrap_or_else(|| "the remote session did not become ready".into());
                Some(BrowserCreateResult::failed(&request, "browser_start_failed", &message))
            } else if hosted.ready {
                let owner = AgentIdentity::new(&request.actor, Some(&self.host));
                Some(
                    match manifest::publish_requested_panel(&id, request.visible, &workspace, owner) {
                        Ok(()) => BrowserCreateResult::ready(
                            &request,
                            id.clone(),
                            match (&request.url, hosted.url) {
                                (None, _) => CreateNavigation::NotRequested,
                                (Some(_), false) => CreateNavigation::Pending,
                                (Some(_), true) => CreateNavigation::Committed,
                            },
                            None,
                            manifest::now_millis()
                                .saturating_sub(request.requested_at_millis)
                                .unsigned_abs(),
                        ),
                        Err(error) => {
                            BrowserCreateResult::failed(&request, "browser_publish_failed", &error.to_string())
                        }
                    },
                )
            } else {
                None
            };
            match result {
                Some(result) => {
                    let ready = matches!(result.outcome, manifest::BrowserCreateOutcome::Ready { .. });
                    if manifest::complete_create_request(&result).is_err() {
                        self.pending.push(request);
                    } else if !ready {
                        self.begin_stop(&id, None);
                    }
                }
                None => self.pending.push(request),
            }
        }
    }

    fn visibility(&mut self) {
        let Ok(requests) = manifest::list_visibility_requests() else {
            return;
        };
        let workspace = self.workspace();
        for request in requests.into_iter().filter(|request| request.actor == self.actor) {
            let Ok(Some(request)) =
                manifest::claim_visibility_request(&request.request_id, &request.actor, &self.host, std::process::id())
            else {
                continue;
            };
            let result = if let Some(hosted) = self.sessions.get_mut(&request.panel_local_id) {
                hosted.visible = request.visible;
                let _ = manifest::sync_host_state(&request.panel_local_id, request.visible, &workspace);
                BrowserVisibilityResult::ready(&request)
            } else {
                BrowserVisibilityResult::failed(&request, "not_found", "the remote session is no longer running")
            };
            let _ = manifest::complete_visibility_request(&result);
        }
    }

    fn closes(&mut self) {
        let Ok(mut requests) = manifest::list_close_requests() else {
            return;
        };
        requests.retain(|request| request.actor == self.actor);
        for request in requests {
            let Ok(Some(request)) =
                manifest::claim_close_request(&request.request_id, &request.actor, &self.host, std::process::id())
            else {
                continue;
            };
            let id = request.panel_local_id.clone();
            if self.sessions.contains_key(&id) {
                self.begin_stop(&id, Some(request));
            } else if let Some(stopping) = self.stopping.get_mut(&id) {
                if let Some(earlier) = stopping.close.replace(request) {
                    let _ = manifest::complete_close_request(&BrowserCloseResult::failed(
                        &earlier,
                        "shutdown_pending",
                        "a later close of the same session replaced this one",
                    ));
                }
            } else {
                let _ = manifest::complete_close_request(&BrowserCloseResult::failed(
                    &request,
                    "not_found",
                    "the remote session is no longer running",
                ));
            }
        }
    }

    fn begin_stop(&mut self, id: &str, close: Option<BrowserCloseRequest>) {
        if let Some(hosted) = self.sessions.remove(id) {
            self.stopping.insert(
                id.to_owned(),
                Stopping {
                    signal: hosted.session.shutdown_signal(),
                    close,
                    lease: hosted.lease,
                },
            );
        }
    }

    /// Finish the stops whose driver ended. A close answers `closed` only for a confirmed
    /// release; the provider slot of an unconfirmed one stays held until its check confirms it.
    fn stopped(&mut self) {
        let now = manifest::now_millis();
        let mut done = Vec::new();
        for (id, stopping) in &mut self.stopping {
            if stopping.signal.is_complete() {
                if let Some(close) = stopping.close.take() {
                    let result = match close_outcome(&stopping.signal) {
                        Ok(()) => BrowserCloseResult::closed(&close),
                        Err((code, message)) => BrowserCloseResult::failed(&close, code, message),
                    };
                    let _ = manifest::complete_close_request(&result);
                }
                done.push(id.clone());
            } else if stopping
                .close
                .as_ref()
                .is_some_and(|close| now >= close.deadline_at_millis)
                && let Some(close) = stopping.close.take()
            {
                let _ = manifest::complete_close_request(&BrowserCloseResult::failed(
                    &close,
                    "shutdown_pending",
                    "the remote session is still being released",
                ));
            }
        }
        for id in &done {
            if let Some(stopping) = self.stopping.remove(id)
                && stopping.signal.holds_remote_allocation()
                && let (Some(allocation), Some(lease)) = (stopping.signal.remote_recovery(), stopping.lease)
            {
                self.retained.push((allocation, lease));
            }
        }
        // A provider can answer a release in a way the driver does not take as
        // proof; its exact session record can still prove the release.
        self.holds.check(&done);
        self.holds.prune();
        self.retained.retain(|(allocation, _)| !allocation.is_released());
    }
}

/// The answer to a close, as the desktop host gives it.
fn close_outcome(signal: &BrowserShutdownSignal) -> Result<(), (&'static str, &'static str)> {
    if !signal.holds_remote_allocation() {
        return Ok(());
    }
    Err(match signal.remote_release() {
        Some(RemoteReleaseOutcome::Failed { .. }) => (
            "release_failed",
            "the session stopped but the provider refused its release; use browser_remote_allocations to reconcile its exact session",
        ),
        _ => (
            "release_unknown",
            "the session stopped but its release is unconfirmed; use browser_remote_allocations to reconcile its exact session",
        ),
    })
}

fn panel_id(request: &BrowserCreateRequest) -> String {
    format!("browser-{}", request.request_id)
}

/// A failure whose message starts with a known code keeps that code.
fn coded(message: &str) -> (String, String) {
    match message.split_once(": ") {
        Some((code, rest)) if CODES.contains(&code) => (code.to_owned(), rest.to_owned()),
        _ => ("browser_start_failed".to_owned(), message.to_owned()),
    }
}

fn orientation_message(released: &RemoteReleaseOutcome) -> &'static str {
    match released {
        RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone => {
            "start orientation could not be verified; the provider confirmed session release"
        }
        RemoteReleaseOutcome::NeverAllocated => {
            "start orientation could not be verified; no remote session was allocated"
        }
        RemoteReleaseOutcome::ReleaseUnknown { .. } | RemoteReleaseOutcome::Failed { .. } => {
            "start orientation could not be verified; release is unconfirmed and capacity remains held, reconcile the retained allocation before creating again"
        }
    }
}
