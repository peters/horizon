//! The host side of a profile host: it claims the create, duplicate, close
//! and visibility requests that name its host instance, opens each page in
//! the shared browser of its profile, and stamps each page for the one actor
//! that opened it. It never deletes a profile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use horizon_browser::session::{SharedBrowserSession, SharedSessionGroup, start_shared_session};
use horizon_browser::{
    BackendKind, BrowserConfig, BrowserEvent, BrowserSession, BrowserSessionConfig, BrowserShutdownSignal, FrameSlot,
    VideoCaptureHandle,
};
use horizon_browser_control::manifest::{
    self, AgentIdentity, BrowserCloseRequest, BrowserCloseResult, BrowserCreateRequest, BrowserCreateResult,
    BrowserVisibilityResult, CreateNavigation, ManifestCoordination, ManifestWorkspace,
};
use horizon_browser_control::paths::safe_local_id;

use super::config::{Profile, ProfileConfig, forget_removed_prefs, user_js};
use super::rules::{Close, Refusal, admit, close_action, idle_pages, page_id, refuse};

/// At most this many pages run in one host, also while they stop.
pub const MAX_PAGES: usize = 16;
/// At most this many pages of one actor are open in one host.
pub const MAX_ACTOR_PAGES: usize = 4;
/// A page with no action for this long closes.
pub const IDLE_CLOSE: Duration = Duration::from_hours(1);
const EXIT_TIMEOUT: Duration = Duration::from_secs(20);
const FORCED_EXIT_TIMEOUT: Duration = Duration::from_secs(3);
/// The folder of the persistent profiles under the runtime root.
pub const PROFILES: &str = "browser-profiles";
const CAPTURES: &str = "profile-host-captures";

struct Page {
    profile: String,
    actor: String,
    session: BrowserSession,
    visible: bool,
    ready: bool,
    lost: bool,
    /// The last URL that the page committed.
    url: Option<String>,
    warning: Option<String>,
    opened: SystemTime,
}

struct Stopping {
    actor: String,
    signal: BrowserShutdownSignal,
    close: Option<BrowserCloseRequest>,
}

pub struct ProfileHost {
    config: ProfileConfig,
    host: String,
    root: PathBuf,
    headed: bool,
    idle: Duration,
    groups: BTreeMap<String, SharedSessionGroup>,
    pages: BTreeMap<String, Page>,
    /// Each create that waits for its page, with the ID of that page.
    pending: Vec<(String, BrowserCreateRequest)>,
    stopping: BTreeMap<String, Stopping>,
}

impl ProfileHost {
    /// Make the folder of each profile and write the `user.js` of each
    /// Firefox profile. A profile that is already there stays as it is.
    ///
    /// # Errors
    /// A profile folder or a `user.js` cannot be written.
    pub fn open(
        config: ProfileConfig,
        host: String,
        root: PathBuf,
        headed: bool,
        idle: Duration,
    ) -> std::io::Result<Self> {
        let mut groups = BTreeMap::new();
        for (name, profile) in &config.profiles {
            let dir = profile_dir(&root, name, profile.backend);
            private_dir(&dir)?;
            private_dir(&profile_root(&root, name))?;
            private_dir(&root.join(PROFILES))?;
            if let Some(engine_parent) = dir.parent() {
                private_dir(engine_parent)?;
            }
            if profile.backend == BackendKind::FirefoxBidi {
                forget_removed_prefs(&dir, &profile.firefox_prefs)?;
                write_private(&dir.join("user.js"), user_js(&profile.firefox_prefs).as_bytes())?;
            }
            let id = profile_id(name);
            let group = match profile.backend {
                BackendKind::FirefoxBidi => SharedSessionGroup::new_firefox(id),
                _ => SharedSessionGroup::Chromium(SharedBrowserSession::new(id)),
            };
            groups.insert(name.clone(), group);
        }
        Ok(Self {
            config,
            host,
            root,
            headed,
            idle,
            groups,
            pages: BTreeMap::new(),
            pending: Vec::new(),
            stopping: BTreeMap::new(),
        })
    }

    fn workspace(&self, page: &Page) -> ManifestWorkspace {
        ManifestWorkspace::new(&self.host, &page.profile, vec![page.actor.clone()])
    }

    /// One pass over every queue and every page.
    pub fn tick(&mut self) {
        self.drain();
        self.creates();
        self.pending();
        self.visibility();
        self.closes();
        self.idle_pages();
        self.stopped();
        for (id, page) in &self.pages {
            let _ = manifest::sync_host_state(id, page.visible, &self.workspace(page));
        }
        self.refuse_host_only();
    }

    /// Stop every page and keep every profile. Returns whether each browser
    /// stopped before the deadline.
    pub fn shutdown(mut self) -> bool {
        let ids: Vec<_> = self.pages.keys().cloned().collect();
        for id in ids {
            self.begin_stop(&id, None);
        }
        for (_, pending) in std::mem::take(&mut self.pending) {
            let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                &pending,
                "browser_start_failed",
                "the profile host is stopping",
            ));
        }
        let deadline = std::time::Instant::now() + EXIT_TIMEOUT;
        let mut stopped = true;
        for stopping in self.stopping.values() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            stopped &= stopping.signal.wait(left) || stopping.signal.force_cleanup(FORCED_EXIT_TIMEOUT);
        }
        self.stopped();
        for (_, stopping) in std::mem::take(&mut self.stopping) {
            if let Some(close) = stopping.close {
                let _ = manifest::complete_close_request(&BrowserCloseResult::failed(
                    &close,
                    "shutdown_pending",
                    "the profile host stopped before the page closed",
                ));
            }
        }
        stopped
    }

    fn drain(&mut self) {
        let mut lost = Vec::new();
        for (id, page) in &mut self.pages {
            for event in page.session.event_rx.try_iter() {
                match event {
                    BrowserEvent::Ready => page.ready = true,
                    BrowserEvent::UrlChanged(url) => page.url = (!url.is_empty()).then_some(url),
                    BrowserEvent::Warning(message) => page.warning = Some(message),
                    BrowserEvent::Stopped { .. } => page.lost = true,
                    BrowserEvent::Frame { .. } => page.session.frame_slot.release_notification(),
                    _ => {}
                }
            }
            if page.lost && !self.pending.iter().any(|(pending, _)| pending == id) {
                lost.push(id.clone());
            }
        }
        for id in lost {
            self.begin_stop(&id, None);
        }
    }

    fn running(&self) -> usize {
        self.pages.len() + self.stopping.len()
    }

    fn creates(&mut self) {
        let Ok(mut requests) = manifest::list_create_requests() else {
            return;
        };
        requests.retain(|request| request.host_instance.as_deref() == Some(self.host.as_str()));
        for request in requests {
            let Ok(Some(mut request)) =
                manifest::claim_create_request(&request.request_id, &request.actor, &self.host, std::process::id())
            else {
                continue;
            };
            let source_page = request.duplicate_from.as_deref().and_then(|id| self.pages.get(id));
            let source = source_page.map(|page| (page.profile.as_str(), page.actor.as_str()));
            let source_url = source_page.and_then(|page| page.url.clone());
            // A page that stops still holds a browser page, so it counts too.
            let mine = self.pages.values().filter(|page| page.actor == request.actor).count()
                + self
                    .stopping
                    .values()
                    .filter(|stopping| stopping.actor == request.actor)
                    .count();
            let admitted = admit(&self.config, &request, source, self.running(), mine).map(str::to_owned);
            if admitted.is_ok() && request.duplicate_from.is_some() {
                // A duplicate opens the current URL of its source page.
                request.url = source_url;
            }
            let started = admitted.and_then(|name| self.start(&request, &name));
            match started {
                Ok(id) => self.pending.push((id, request)),
                Err(refusal) => {
                    let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                        &request,
                        refusal.code,
                        &refusal.message,
                    ));
                }
            }
        }
    }

    fn start(&mut self, request: &BrowserCreateRequest, name: &str) -> Result<String, Refusal> {
        let (Some(profile), Some(group)) = (self.config.profile(name), self.groups.get(name)) else {
            return Err(refuse(
                "target_unknown",
                format!("profile `{name}` is not in the host file"),
            ));
        };
        let id = loop {
            let id = page_id(name);
            if !self.pages.contains_key(&id) && !self.stopping.contains_key(&id) {
                break id;
            }
        };
        let session = start_shared_session(
            BrowserSessionConfig {
                browser: browser_config(&self.root, name, profile, self.headed),
                panel_local_id: id.clone(),
                initial_url: request.url.clone(),
                width: horizon_browser::DEFAULT_VIEWPORT.0,
                height: horizon_browser::DEFAULT_VIEWPORT.1,
                frame_slot: Arc::new(FrameSlot::new()),
                coordination: Some(Arc::new(ManifestCoordination::default())),
                capture_directory: Some(self.root.join(CAPTURES).join(safe_local_id(&id))),
                video: Arc::new(VideoCaptureHandle::default()),
                remote: None,
            },
            group.clone(),
        )
        .map_err(|error| refuse("browser_start_failed", error.to_string()))?;
        self.pages.insert(
            id.clone(),
            Page {
                profile: name.to_owned(),
                actor: request.actor.clone(),
                session,
                visible: request.visible,
                ready: false,
                lost: false,
                url: None,
                warning: None,
                opened: SystemTime::now(),
            },
        );
        Ok(id)
    }

    fn pending(&mut self) {
        for (id, request) in std::mem::take(&mut self.pending) {
            let Some(page) = self.pages.get(&id) else {
                let _ = manifest::complete_create_request(&BrowserCreateResult::failed(
                    &request,
                    "browser_start_failed",
                    "the page stopped",
                ));
                continue;
            };
            let result = if page.lost || manifest::now_millis() >= request.deadline_at_millis {
                let message = page
                    .warning
                    .clone()
                    .unwrap_or_else(|| "the page did not become ready".into());
                Some(BrowserCreateResult::failed(&request, "browser_start_failed", &message))
            } else if page.ready {
                let owner = AgentIdentity::new(&request.actor, Some(&self.host));
                Some(
                    match manifest::publish_requested_panel(&id, request.visible, &self.workspace(page), owner) {
                        Ok(()) => BrowserCreateResult::ready(
                            &request,
                            id.clone(),
                            match (&request.url, page.url.is_some()) {
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
                        self.pending.push((id, request));
                    } else if !ready {
                        self.begin_stop(&id, None);
                    }
                }
                None => self.pending.push((id, request)),
            }
        }
    }

    fn visibility(&mut self) {
        let Ok(requests) = manifest::list_visibility_requests() else {
            return;
        };
        for request in requests
            .into_iter()
            .filter(|request| request.host_instance.as_deref() == Some(self.host.as_str()))
        {
            let Ok(Some(request)) =
                manifest::claim_visibility_request(&request.request_id, &request.actor, &self.host, std::process::id())
            else {
                continue;
            };
            let workspace = self
                .pages
                .get(&request.panel_local_id)
                .filter(|page| page.actor == request.actor)
                .map(|page| self.workspace(page));
            let result = match (self.pages.get_mut(&request.panel_local_id), workspace) {
                (Some(page), Some(workspace)) => {
                    page.visible = request.visible;
                    let _ = manifest::sync_host_state(&request.panel_local_id, request.visible, &workspace);
                    BrowserVisibilityResult::ready(&request)
                }
                _ => BrowserVisibilityResult::failed(&request, "not_found", "the page is no longer open"),
            };
            let _ = manifest::complete_visibility_request(&result);
        }
    }

    fn closes(&mut self) {
        let Ok(mut requests) = manifest::list_close_requests() else {
            return;
        };
        requests.retain(|request| request.host_instance.as_deref() == Some(self.host.as_str()));
        for request in requests {
            let Ok(Some(request)) =
                manifest::claim_close_request(&request.request_id, &request.actor, &self.host, std::process::id())
            else {
                continue;
            };
            let id = request.panel_local_id.clone();
            let open = self.pages.get(&id).map(|page| page.actor.as_str());
            let stopping = self.stopping.get(&id).map(|stopping| stopping.actor.as_str());
            match close_action(open, stopping, &request.actor) {
                Close::Stop => self.begin_stop(&id, Some(request)),
                Close::Wait => {
                    let earlier = self
                        .stopping
                        .get_mut(&id)
                        .and_then(|stopping| stopping.close.replace(request));
                    if let Some(earlier) = earlier {
                        let _ = manifest::complete_close_request(&BrowserCloseResult::failed(
                            &earlier,
                            "shutdown_pending",
                            "a later close of the same page replaced this one",
                        ));
                    }
                }
                Close::NotFound => {
                    let _ = manifest::complete_close_request(&BrowserCloseResult::failed(
                        &request,
                        "not_found",
                        "the page is no longer open",
                    ));
                }
            }
        }
    }

    /// Close each ready page whose last action is older than the idle limit.
    /// An action is each line of the audit journal of the page.
    fn idle_pages(&mut self) {
        let ready = self
            .pages
            .iter()
            .filter(|(_, page)| page.ready)
            .map(|(id, page)| (id.as_str(), page.opened));
        let idle = idle_pages(&self.root, ready, SystemTime::now(), self.idle);
        for id in idle {
            tracing::info!(page = %id, "the page had no action within the idle limit and closes");
            self.begin_stop(&id, None);
        }
    }

    fn begin_stop(&mut self, id: &str, close: Option<BrowserCloseRequest>) {
        if let Some(page) = self.pages.remove(id) {
            self.stopping.insert(
                id.to_owned(),
                Stopping {
                    signal: page.session.shutdown_signal(),
                    actor: page.actor,
                    close,
                },
            );
        }
    }

    fn stopped(&mut self) {
        let now = manifest::now_millis();
        let mut done = Vec::new();
        for (id, stopping) in &mut self.stopping {
            if stopping.signal.is_complete() {
                if let Some(close) = stopping.close.take() {
                    let _ = manifest::complete_close_request(&BrowserCloseResult::closed(&close));
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
                    "the page is still closing",
                ));
            }
        }
        for id in done {
            self.stopping.remove(&id);
            let _ = std::fs::remove_dir_all(self.root.join(CAPTURES).join(safe_local_id(&id)));
        }
    }

    /// Requests that only a Horizon host answers get a refusal, not a wait.
    fn refuse_host_only(&self) {
        use horizon_browser_control::manifest::{cast, device, provider_usage, recovery};
        if let Ok(requests) = cast::claim(&self.host) {
            for request in requests {
                let _ = cast::complete(
                    &request,
                    cast::CastOutcome::failed("cast_unavailable: requires a Horizon host"),
                );
            }
        }
        if let Ok(requests) = device::claim(&self.host) {
            for request in requests {
                let outcome = device::Outcome::failed(
                    "viewer_requires_horizon",
                    "A profile host has no Horizon window to show a device viewer.",
                );
                let _ = device::complete(&request, outcome);
            }
        }
        if let Ok(requests) = provider_usage::claim_provider_usage_requests(&self.host) {
            for request in requests {
                let error = "provider_usage_unavailable: a profile host has no remote provider and no Horizon window";
                let _ = provider_usage::complete_provider_usage(&request.result(Vec::new(), Some(error.into())));
            }
        }
        if let Ok(requests) = recovery::claim_recovery_requests(&self.host) {
            for request in requests {
                let _ = recovery::complete_recovery(&request.result(Vec::new(), None));
            }
        }
    }
}

/// The ID of the shared browser of a profile.
fn profile_id(name: &str) -> String {
    format!("profile-{name}")
}

/// The folder of a profile, `<root>/browser-profiles/profile-<name>`. The
/// engine keeps its data in a folder under it.
#[must_use]
pub fn profile_root(root: &Path, name: &str) -> PathBuf {
    root.join(PROFILES).join(profile_id(name))
}

/// The folder that the engine of a profile uses.
#[must_use]
pub fn profile_dir(root: &Path, name: &str, backend: BackendKind) -> PathBuf {
    let config = BrowserConfig {
        backend,
        profile_root: Some(profile_root(root, name)),
        ..BrowserConfig::default()
    };
    config.profile_dir(&profile_id(name))
}

fn browser_config(root: &Path, name: &str, profile: &Profile, headed: bool) -> BrowserConfig {
    BrowserConfig {
        backend: profile.backend,
        headless: !headed,
        extra_args: profile.args.clone(),
        profile_root: Some(profile_root(root, name)),
        // On a shared display, a new page never takes the keyboard of the person.
        background_windows: headed,
        ..BrowserConfig::default()
    }
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Replace `path` with `bytes`, readable only by the owner.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::Builder::new().prefix(".profile-host").tempfile_in(dir)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.as_file().set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
