//! Shared Firefox process ownership and page-scoped classic commands.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::process::{ChromeProcessControl, ProcessLifecycle};
use crate::websocket::{JsonWsError, JsonWsLink};

use super::host::DriverHost;
use super::http::{HttpClient, HttpError, remaining_timeout};
use super::service::WebDriverService;
use super::session::handshake::NewSession;
use super::transport::ClassicTransport;

#[derive(Clone)]
pub struct SharedFirefoxSession {
    profile_id: String,
    state: Arc<Mutex<GroupState>>,
    drivers: Arc<AtomicUsize>,
    stops: Arc<Mutex<Vec<Weak<AtomicBool>>>>,
    classic: Arc<Mutex<()>>,
}

#[derive(Default)]
struct GroupState {
    service: Option<WebDriverService>,
    control: ChromeProcessControl,
    session_id: String,
    capabilities: Value,
    endpoint: String,
    retiring: Arc<AtomicBool>,
    profile_retired: bool,
    creation_uncertain: bool,
    launch_identity: Option<FirefoxLaunchIdentity>,
}

#[derive(PartialEq, Eq)]
struct FirefoxLaunchIdentity {
    profile: std::path::PathBuf,
    browser: Option<String>,
    driver: Option<String>,
    extra_args: Vec<String>,
    headless: bool,
    disclosure: crate::AutomationDisclosurePolicy,
}

impl GroupState {
    fn pin_launch(&mut self, config: &crate::BrowserConfig, profile_id: &str) -> Result<(), String> {
        let identity = FirefoxLaunchIdentity {
            profile: std::path::absolute(config.profile_dir(profile_id)).map_err(|error| error.to_string())?,
            browser: config.firefox_command.clone(),
            driver: config.geckodriver_command.clone(),
            extra_args: config.extra_args.clone(),
            headless: config.headless,
            disclosure: config.automation_disclosure,
        };
        if self.launch_identity.as_ref().is_some_and(|pinned| pinned != &identity) {
            return Err(
                "shared Firefox process configuration does not match its pinned profile and launch settings".into(),
            );
        }
        self.launch_identity.get_or_insert(identity);
        Ok(())
    }
}

impl std::fmt::Debug for SharedFirefoxSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedFirefoxSession")
            .field("profile_id", &self.profile_id)
            .finish_non_exhaustive()
    }
}

impl SharedFirefoxSession {
    pub(crate) fn new(profile_id: String) -> Self {
        Self {
            profile_id,
            state: Arc::default(),
            drivers: Arc::default(),
            stops: Arc::default(),
            classic: Arc::default(),
        }
    }

    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.state
            .try_lock()
            .is_ok_and(|state| self.drivers.load(Ordering::Acquire) == 0 && state.control.is_reaped())
    }

    pub(crate) fn retire_profile_for_cleanup(&self) -> bool {
        let Ok(mut state) = self.state.try_lock() else {
            return false;
        };
        if state.profile_retired {
            return true;
        }
        if self.drivers.load(Ordering::Acquire) != 0 || !state.control.is_reaped() {
            return false;
        }
        state.profile_retired = true;
        true
    }

    pub(crate) fn reserve(&self, stop: Arc<AtomicBool>) -> FirefoxReservation {
        let mut stops = self.stops.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        stops.retain(|stop| stop.strong_count() > 0);
        stops.push(Arc::downgrade(&stop));
        self.drivers.fetch_add(1, Ordering::AcqRel);
        FirefoxReservation {
            group: self.clone(),
            stop,
        }
    }

    pub(super) fn acquire(
        &self,
        config: &crate::BrowserConfig,
        panel_control: &ChromeProcessControl,
        stop: &AtomicBool,
        launch: impl FnOnce(&ChromeProcessControl) -> Result<(DriverHost, NewSession), String>,
    ) -> Result<(DriverHost, NewSession), String> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.profile_retired {
            return Err("shared Firefox profile has been retired for deletion".into());
        }
        if state.creation_uncertain && !state.control.is_reaped() {
            return Err("Firefox page creation is uncertain; close the shared panels before retrying".into());
        }
        if stop.load(Ordering::Acquire) {
            return Err("Firefox page startup cancelled".into());
        }
        state.pin_launch(config, &self.profile_id)?;
        let first = state.service.is_none();
        if first {
            if !state.control.is_reaped() {
                return Err("previous Firefox process has not released its profile".into());
            }
            state.control = ChromeProcessControl::default();
            if panel_control.delegate(Arc::new(StartupControl(state.control.clone()))) {
                let _ = state.control.terminate(Duration::ZERO);
            }
            state.retiring = Arc::new(AtomicBool::new(true));
            let started = launch(&state.control);
            state.control.mark_registration_settled();
            let (host, session) = started?;
            let DriverHost::Local(service) = host else {
                return Err("shared Firefox requires a local service".into());
            };
            state.service = Some(service);
            state.session_id = session.id;
            session
                .capabilities
                .get("webSocketUrl")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .clone_into(&mut state.endpoint);
            state.capabilities = session.capabilities;
        }
        if (!first && state.retiring.load(Ordering::Acquire)) || state.control.is_reaped() {
            return Err("shared Firefox session has stopped".into());
        }
        let mut link = JsonWsLink::connect(&state.endpoint).map_err(|error| error.to_string())?;
        let lifecycle = Arc::new(PageControl {
            control: state.control.clone(),
            retiring: Arc::clone(&state.retiring),
            released: Arc::new(AtomicBool::new(false)),
            stops: Arc::clone(&self.stops),
            cleanup: Arc::new(Mutex::new(PageCleanup {
                endpoint: state.endpoint.clone(),
                context: String::new(),
                registrations: Vec::new(),
                retry_after: Instant::now(),
            })),
            closing: false.into(),
            cleanup_done: Arc::new(false.into()),
            retry_running: Arc::new(false.into()),
        });
        if panel_control.delegate(lifecycle.clone()) {
            let _ = panel_control.terminate(Duration::ZERO);
        }
        let context = state.create_context(&mut link, first)?;
        context.clone_into(
            &mut lifecycle
                .cleanup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .context,
        );
        state.retiring.store(false, Ordering::Release);
        let service = state
            .service
            .as_ref()
            .ok_or_else(|| "Firefox service disappeared".to_string())?;
        let page = SharedFirefoxPage {
            group: self.clone(),
            lifecycle,
            http: service.http,
            context: context.clone(),
            session_id: state.session_id.clone(),
            contexts: std::collections::HashSet::from([context.clone()]),
        };
        let session = NewSession {
            id: state.session_id.clone(),
            capabilities: state.capabilities.clone(),
        };
        drop(state);
        Ok((DriverHost::Shared(page), session))
    }
}

impl GroupState {
    fn create_context(&mut self, link: &mut JsonWsLink, first: bool) -> Result<String, String> {
        // The command may execute without a reply. Retain this generation's
        // cleanup ownership and refuse more creates until it has been reaped.
        self.creation_uncertain = true;
        let context = if first {
            link.call(
                Duration::from_secs(5),
                "browsingContext.getTree",
                &json!({"maxDepth": 0}),
            )
            .result
            .map_err(|error| error.to_string())?
            .pointer("/contexts/0/context")
            .and_then(Value::as_str)
            .map(str::to_owned)
        } else {
            link.call(
                Duration::from_secs(5),
                "browsingContext.create",
                &json!({"type": "window", "background": true}),
            )
            .result
            .map_err(|error| error.to_string())?
            .get("context")
            .and_then(Value::as_str)
            .map(str::to_owned)
        }
        .filter(|context| !context.is_empty())
        .ok_or_else(|| "Firefox did not return the exact page context".to_string())?;
        self.creation_uncertain = false;
        Ok(context)
    }

    fn close(&mut self) {
        self.retiring.store(true, Ordering::Release);
        if let Some(service) = self.service.as_mut() {
            service.delete_session(&self.session_id);
            if service.process.kill() {
                self.service = None;
            }
        } else {
            let _ = self.control.terminate(Duration::from_secs(1));
        }
    }
}

pub(crate) struct FirefoxReservation {
    pub(crate) group: SharedFirefoxSession,
    stop: Arc<AtomicBool>,
}

impl Drop for FirefoxReservation {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if self.group.drivers.fetch_sub(1, Ordering::AcqRel) == 1 {
            let mut state = self
                .group
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.group.drivers.load(Ordering::Acquire) == 0 {
                state.close();
            }
        }
    }
}

struct StartupControl(ChromeProcessControl);
impl ProcessLifecycle for StartupControl {
    fn is_reaped(&self) -> bool {
        self.0.is_reaped()
    }
    fn terminate(&self, timeout: Duration) -> bool {
        self.0.terminate(timeout)
    }
}

struct PageCleanup {
    endpoint: String,
    context: String,
    registrations: Vec<(&'static str, String)>,
    retry_after: Instant,
}

impl PageCleanup {
    fn attempt(&mut self, released: &AtomicBool) -> bool {
        if self.context.is_empty() {
            return false;
        }
        let Ok(mut link) = JsonWsLink::connect(&self.endpoint) else {
            return false;
        };
        self.registrations.retain(|(method, id)| {
            let params = match *method {
                "session.unsubscribe" => json!({"subscriptions": [id]}),
                "network.removeIntercept" => json!({"intercept": id}),
                "network.removeDataCollector" => json!({"collector": id}),
                _ => json!({"script": id}),
            };
            !registration_removed(&link.call(Duration::from_secs(1), method, &params).result)
        });
        if !released.load(Ordering::Acquire) {
            let gone = link
                .call(
                    Duration::from_secs(2),
                    "browsingContext.close",
                    &json!({"context": self.context}),
                )
                .result
                .is_ok()
                || link
                    .call(
                        Duration::from_secs(1),
                        "browsingContext.getTree",
                        &json!({"maxDepth": 0}),
                    )
                    .result
                    .is_ok_and(|tree| context_absent(&tree, &self.context));
            if gone {
                released.store(true, Ordering::Release);
            }
        }
        released.load(Ordering::Acquire) && self.registrations.is_empty()
    }
}

fn registration_removed(result: &Result<Value, JsonWsError>) -> bool {
    match result {
        Ok(_) => true,
        Err(JsonWsError::Protocol { method, message }) => {
            let code = message.split_once(':').map_or(message.as_str(), |(code, _)| code);
            // Unsubscribe always sends one well-formed subscription ID; its
            // invalid-argument response therefore proves that ID is absent.
            matches!(
                (method.as_str(), code),
                ("session.unsubscribe", "invalid argument")
                    | ("network.removeIntercept", "no such intercept")
                    | ("script.removePreloadScript", "no such script")
                    | ("network.removeDataCollector", "no such network collector")
            )
        }
        Err(_) => false,
    }
}

struct PageControl {
    control: ChromeProcessControl,
    retiring: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
    stops: Arc<Mutex<Vec<Weak<AtomicBool>>>>,
    cleanup: Arc<Mutex<PageCleanup>>,
    closing: AtomicBool,
    cleanup_done: Arc<AtomicBool>,
    retry_running: Arc<AtomicBool>,
}

impl PageControl {
    fn close(&self) {
        self.closing.store(true, Ordering::Release);
        if self.cleanup_done.load(Ordering::Acquire) || self.control.is_reaped() {
            return;
        }
        let Ok(mut cleanup) = self.cleanup.try_lock() else {
            return;
        };
        self.cleanup_done
            .store(cleanup.attempt(&self.released), Ordering::Release);
        cleanup.retry_after = Instant::now() + Duration::from_secs(1);
    }

    fn schedule_retry(&self) {
        if !self.closing.load(Ordering::Acquire) || self.retry_running.load(Ordering::Acquire) {
            return;
        }
        let Ok(mut cleanup) = self.cleanup.try_lock() else {
            return;
        };
        if Instant::now() < cleanup.retry_after || (cleanup.endpoint.is_empty() || cleanup.context.is_empty()) {
            return;
        }
        cleanup.retry_after = Instant::now() + Duration::from_secs(1);
        drop(cleanup);
        if self.retry_running.swap(true, Ordering::AcqRel) {
            return;
        }
        let cleanup = Arc::clone(&self.cleanup);
        let released = Arc::clone(&self.released);
        let done = Arc::clone(&self.cleanup_done);
        let running = Arc::clone(&self.retry_running);
        let process = self.control.clone();
        if std::thread::Builder::new()
            .name("firefox-page-cleanup".into())
            .spawn(move || {
                let mut cleanup = cleanup.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if !done.load(Ordering::Acquire) {
                    done.store(process.is_reaped() || cleanup.attempt(&released), Ordering::Release);
                }
                cleanup.retry_after = Instant::now() + Duration::from_secs(1);
                running.store(false, Ordering::Release);
            })
            .is_err()
        {
            self.retry_running.store(false, Ordering::Release);
        }
    }
}

impl ProcessLifecycle for PageControl {
    fn is_reaped(&self) -> bool {
        if self.control.is_reaped()
            || (self.cleanup_done.load(Ordering::Acquire) && !self.retiring.load(Ordering::Acquire))
        {
            return true;
        }
        if !self.cleanup_done.load(Ordering::Acquire) {
            self.schedule_retry();
        }
        false
    }
    fn terminate(&self, timeout: Duration) -> bool {
        if self.is_reaped() {
            return true;
        }
        let Ok(stops) = self.stops.try_lock() else {
            return false;
        };
        // A finished driver no longer contributes to the reservation count.
        // Keep new reservations excluded until the exact process is stopped.
        if stops
            .iter()
            .filter_map(Weak::upgrade)
            .any(|stop| !stop.load(Ordering::Acquire))
        {
            return false;
        }
        self.control.terminate(timeout)
    }
}

pub(super) struct SharedFirefoxPage {
    group: SharedFirefoxSession,
    lifecycle: Arc<PageControl>,
    http: HttpClient,
    pub(super) context: String,
    session_id: String,
    contexts: std::collections::HashSet<String>,
}

impl SharedFirefoxPage {
    pub(super) fn accepts_event(&mut self, event: &Value) -> bool {
        let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        let context = params
            .get("context")
            .or_else(|| params.pointer("/source/context"))
            .and_then(Value::as_str);
        if method == "browsingContext.contextCreated"
            && params
                .get("parent")
                .and_then(Value::as_str)
                .is_some_and(|parent| self.contexts.contains(parent))
            && let Some(context) = context
        {
            self.contexts.insert(context.to_owned());
        }
        let owned = context.is_some_and(|context| self.contexts.contains(context));
        if method == "browsingContext.contextDestroyed"
            && let Some(context) = context
        {
            self.contexts.remove(context);
        }
        owned
    }

    pub(super) fn is_closed(&self) -> bool {
        self.lifecycle.released.load(Ordering::Acquire) || self.lifecycle.control.is_reaped()
    }
    pub(super) fn context_destroyed(&self) {
        self.lifecycle.released.store(true, Ordering::Release);
    }
    pub(super) fn remember(&mut self, method: &'static str, id: String) {
        self.lifecycle
            .cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .registrations
            .push((method, id));
    }

    pub(super) fn record_bidi_result(&mut self, method: &str, params: &Value, result: &Result<Value, JsonWsError>) {
        let registration = match method {
            "session.subscribe" => Some(("subscription", "session.unsubscribe")),
            "network.addIntercept" => Some(("intercept", "network.removeIntercept")),
            "script.addPreloadScript" => Some(("script", "script.removePreloadScript")),
            "network.addDataCollector" => Some(("collector", "network.removeDataCollector")),
            _ => None,
        };
        if let Some((field, remove)) = registration {
            if let Ok(value) = result
                && let Some(id) = value.get(field).and_then(Value::as_str)
            {
                self.remember(remove, id.to_owned());
            }
        } else if registration_removed(result) {
            self.lifecycle
                .cleanup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .registrations
                .retain(|(remove, id)| {
                    if *remove != method {
                        return true;
                    }
                    let field = match method {
                        "network.removeIntercept" => "intercept",
                        "network.removeDataCollector" => "collector",
                        "script.removePreloadScript" => "script",
                        _ => {
                            return !params
                                .get("subscriptions")
                                .and_then(Value::as_array)
                                .is_some_and(|ids| ids.iter().any(|value| value.as_str() == Some(id)));
                        }
                    };
                    params.get(field).and_then(Value::as_str) != Some(id)
                });
        }
    }

    pub(super) fn close(&mut self) {
        self.lifecycle.close();
    }
}

impl Drop for SharedFirefoxPage {
    fn drop(&mut self) {
        self.close();
    }
}

impl ClassicTransport for SharedFirefoxPage {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        read_timeout: Duration,
    ) -> Result<Value, HttpError> {
        let prefix = format!("/session/{}/", self.session_id);
        let suffix = path
            .strip_prefix(&prefix)
            .ok_or_else(|| HttpError::InvalidResponse("wrong shared session".into()))?;
        if !page_command(method, suffix) {
            return Err(HttpError::InvalidResponse(
                "session-global command refused for shared Firefox page".into(),
            ));
        }
        let deadline = Instant::now() + read_timeout;
        let _lock = loop {
            match self.group.classic.try_lock() {
                Ok(lock) => break lock,
                Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    remaining_timeout(deadline)?;
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        };
        if self.is_closed() {
            return Err(HttpError::InvalidResponse("shared Firefox page has closed".into()));
        }
        self.http.request_until(
            "POST",
            &format!("{prefix}window"),
            Some(&json!({"handle": self.context})),
            deadline,
        )?;
        self.http.request_until(method, path, body, deadline)
    }
}

fn page_command(method: &str, suffix: &str) -> bool {
    matches!(
        (method, suffix),
        ("GET", "url" | "title" | "screenshot") | ("POST", "execute/sync" | "back" | "forward")
    )
}

fn context_absent(tree: &Value, target: &str) -> bool {
    tree.get("contexts").and_then(Value::as_array).is_some_and(|contexts| {
        contexts.iter().all(|item| {
            item.get("context")
                .and_then(Value::as_str)
                .is_some_and(|id| id != target)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_server::{Reply, Server};
    use super::*;

    struct RetainedProcess {
        reaped: AtomicBool,
        terminations: AtomicUsize,
    }
    impl ProcessLifecycle for RetainedProcess {
        fn is_reaped(&self) -> bool {
            self.reaped.load(Ordering::Acquire)
        }
        fn terminate(&self, _: Duration) -> bool {
            self.terminations.fetch_add(1, Ordering::AcqRel);
            self.is_reaped()
        }
    }

    fn page(group: SharedFirefoxSession, port: u16, context: &str) -> (SharedFirefoxPage, Arc<RetainedProcess>) {
        let retained = Arc::new(RetainedProcess {
            reaped: false.into(),
            terminations: 0.into(),
        });
        let control = ChromeProcessControl::default();
        assert!(!control.delegate(retained.clone()));
        let lifecycle = Arc::new(PageControl {
            control,
            retiring: Arc::new(false.into()),
            released: Arc::new(false.into()),
            stops: Arc::clone(&group.stops),
            cleanup: Arc::new(Mutex::new(PageCleanup {
                endpoint: String::new(),
                context: context.into(),
                registrations: Vec::new(),
                retry_after: Instant::now(),
            })),
            closing: false.into(),
            cleanup_done: Arc::new(false.into()),
            retry_running: Arc::new(false.into()),
        });
        (
            SharedFirefoxPage {
                group,
                lifecycle,
                http: HttpClient::new(([127, 0, 0, 1], port).into()).expect("client"),
                context: context.into(),
                session_id: "session".into(),
                contexts: std::collections::HashSet::from([context.into()]),
            },
            retained,
        )
    }

    #[test]
    fn concurrent_commands_select_their_page_without_interleaving() {
        let server = Server::start((0..4).map(|_| Reply::json(200, &json!({"value": null}))).collect());
        let group = SharedFirefoxSession::new("profile".into());
        let (left, _) = page(group.clone(), server.port, "left");
        let (right, _) = page(group, server.port, "right");
        let run = |page: SharedFirefoxPage| {
            std::thread::spawn(move || page.request("GET", "/session/session/title", None, Duration::from_secs(2)))
        };
        let left = run(left);
        let right = run(right);
        assert!(left.join().expect("left").is_ok());
        assert!(right.join().expect("right").is_ok());
        let requests = server.recorded();
        assert_eq!(
            requests.iter().map(|request| request.path.as_str()).collect::<Vec<_>>(),
            [
                "/session/session/window",
                "/session/session/title",
                "/session/session/window",
                "/session/session/title"
            ]
        );
        let handles = [0, 2].map(|index| {
            serde_json::from_str::<Value>(&requests[index].body).expect("selection")["handle"]
                .as_str()
                .expect("handle")
                .to_owned()
        });
        assert_ne!(handles[0], handles[1]);
    }

    #[test]
    fn failed_page_selection_never_dispatches_the_command() {
        let server = Server::start(vec![Reply::json(
            404,
            &json!({"value":{"error":"no such window","message":"gone"}}),
        )]);
        let (page, _) = page(SharedFirefoxSession::new("profile".into()), server.port, "gone");
        assert!(page.get("/session/session/title").is_err());
        assert_eq!(server.recorded().len(), 1);
    }

    #[test]
    fn forbidden_routes_cannot_mutate_session_global_state() {
        let (page, _) = page(SharedFirefoxSession::new("profile".into()), 1, "page");
        for (method, path) in [
            ("DELETE", "/session/session"),
            ("POST", "/session/session/timeouts"),
            ("DELETE", "/session/session/actions"),
            ("POST", "/session/session/window"),
            ("POST", "/session/session/frame"),
            ("GET", "/session/other/title"),
        ] {
            assert!(matches!(
                page.request(method, path, None, Duration::from_secs(1)),
                Err(HttpError::InvalidResponse(_))
            ));
        }
    }

    #[test]
    fn waiting_for_a_sibling_cannot_exceed_the_command_budget() {
        let group = SharedFirefoxSession::new("profile".into());
        let (page, _) = page(group.clone(), 1, "page");
        let _held = group.classic.lock().expect("hold command lock");
        let start = Instant::now();
        assert!(matches!(
            page.request("GET", "/session/session/title", None, Duration::from_millis(30)),
            Err(HttpError::Io(_))
        ));
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn final_closed_page_stays_pending_until_exact_process_reap() {
        let (page, process) = page(SharedFirefoxSession::new("profile".into()), 1, "page");
        page.lifecycle.released.store(true, Ordering::Release);
        page.lifecycle.cleanup_done.store(true, Ordering::Release);
        assert!(page.lifecycle.is_reaped());
        page.lifecycle.retiring.store(true, Ordering::Release);
        assert!(!page.lifecycle.is_reaped());
        assert!(!page.lifecycle.terminate(Duration::ZERO));
        assert_eq!(process.terminations.load(Ordering::Acquire), 1);
        process.reaped.store(true, Ordering::Release);
        assert!(page.lifecycle.is_reaped());
    }

    #[test]
    fn failed_close_cannot_force_stop_the_only_remaining_sibling() {
        let group = SharedFirefoxSession::new("profile".into());
        let first = group.reserve(Arc::new(false.into()));
        let sibling_stop = Arc::new(false.into());
        let sibling = group.reserve(Arc::clone(&sibling_stop));
        let (page, process) = page(group, 1, "failed-close");
        drop(first);
        assert!(!page.lifecycle.terminate(Duration::ZERO));
        assert_eq!(process.terminations.load(Ordering::Acquire), 0);
        sibling_stop.store(true, Ordering::Release);
        assert!(!page.lifecycle.terminate(Duration::ZERO));
        assert_eq!(process.terminations.load(Ordering::Acquire), 1);
        drop(sibling);
    }

    #[test]
    fn malformed_context_lists_do_not_prove_target_closure() {
        for tree in [
            json!({}),
            json!({"contexts":[{}]}),
            json!({"contexts":[{"context":"other"},{}]}),
            json!({"contexts":[{"context":"page"}]}),
        ] {
            assert!(!context_absent(&tree, "page"));
        }
        assert!(context_absent(&json!({"contexts":[]}), "page"));
        assert!(context_absent(&json!({"contexts":[{"context":"other"}]}), "page"));
    }

    #[test]
    fn only_owned_contexts_and_descendants_receive_events() {
        let (mut page, _) = page(SharedFirefoxSession::new("profile".into()), 1, "root");
        assert!(!page.accepts_event(&json!({"method":"network.authRequired","params":{"context":"sibling"}})));
        assert!(page.accepts_event(
            &json!({"method":"browsingContext.contextCreated","params":{"context":"child","parent":"root"}})
        ));
        assert!(page.accepts_event(&json!({"method":"network.authRequired","params":{"context":"child"}})));
        assert!(page.accepts_event(&json!({"method":"browsingContext.contextDestroyed","params":{"context":"child"}})));
        assert!(!page.accepts_event(&json!({"method":"network.authRequired","params":{"context":"child"}})));
    }

    #[test]
    fn cleanup_retirement_prevents_a_new_launch() {
        let group = SharedFirefoxSession::new("profile".into());
        assert!(group.retire_profile_for_cleanup());
        let stop = Arc::new(false.into());
        let reservation = group.reserve(Arc::clone(&stop));
        assert!(
            group
                .acquire(
                    &crate::BrowserConfig::default(),
                    &ChromeProcessControl::default(),
                    &stop,
                    |_| panic!("retired profile must not launch")
                )
                .is_err()
        );
        assert!(group.retire_profile_for_cleanup());
        drop(reservation);
    }
    #[test]
    fn firefox_group_rejects_profile_or_process_configuration_changes() {
        let mut state = GroupState::default();
        let mut config = crate::BrowserConfig {
            backend: crate::BackendKind::FirefoxBidi,
            profile_root: Some(std::path::PathBuf::from("profile-a")),
            ..crate::BrowserConfig::default()
        };
        assert!(state.pin_launch(&config, "group").is_ok());
        config.quality = 20;
        assert!(state.pin_launch(&config, "group").is_ok());
        config.profile_root = Some(std::path::PathBuf::from("profile-b"));
        assert!(state.pin_launch(&config, "group").is_err());
        config.profile_root = Some(std::path::PathBuf::from("profile-a"));
        config.headless = !config.headless;
        assert!(state.pin_launch(&config, "group").is_err());
    }
    #[test]
    fn ambiguous_creation_retains_cleanup_and_blocks_more_windows_until_process_reap() {
        use std::net::TcpListener;
        use tungstenite::Message;
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("connect");
            let mut socket = tungstenite::accept(stream).expect("handshake");
            let Message::Text(text) = socket.read().expect("create") else {
                panic!("text")
            };
            let command: Value = serde_json::from_str(&text).expect("json");
            assert_eq!(command["method"], "browsingContext.create");
            // The window was created, but its identifying response was lost.
            socket.close(None).expect("lose reply");
        });
        let group = SharedFirefoxSession::new("profile".into());
        let sibling = group.reserve(Arc::new(false.into()));
        let (page, process) = page(group.clone(), 1, "");
        let control = Arc::clone(&page.lifecycle);
        let mut link = JsonWsLink::connect(&format!("ws://{address}/")).expect("link");
        {
            let mut state = group.state.lock().expect("state");
            state.control = control.control.clone();
            assert!(state.create_context(&mut link, false).is_err());
        }
        drop(page);
        for _ in 0..3 {
            assert!(
                group
                    .acquire(
                        &crate::BrowserConfig::default(),
                        &ChromeProcessControl::default(),
                        &AtomicBool::new(false),
                        |_| panic!("must not create again")
                    )
                    .is_err()
            );
            assert!(!control.is_reaped());
            assert!(!control.terminate(Duration::ZERO));
        }
        assert!(!control.retry_running.load(Ordering::Acquire));
        assert_eq!(process.terminations.load(Ordering::Acquire), 0);
        process.reaped.store(true, Ordering::Release);
        assert!(control.is_reaped());
        server.join().expect("server");
        drop(sibling);
    }

    #[test]
    fn cleanup_accepts_only_command_specific_absence_errors() {
        for (method, code) in [
            ("session.unsubscribe", "invalid argument"),
            ("network.removeIntercept", "no such intercept"),
            ("script.removePreloadScript", "no such script"),
            ("network.removeDataCollector", "no such network collector"),
        ] {
            assert!(registration_removed(&Err(JsonWsError::Protocol {
                method: method.into(),
                message: format!("{code}: absent"),
            })));
            assert!(!registration_removed(&Err(JsonWsError::Protocol {
                method: method.into(),
                message: "unknown error: try again".into(),
            })));
            assert!(!registration_removed(&Err(JsonWsError::Timeout {
                method: method.into()
            })));
        }
        assert!(!registration_removed(&Err(JsonWsError::Protocol {
            method: "network.removeIntercept".into(),
            message: "invalid argument: malformed request".into(),
        })));
        assert!(registration_removed(&Ok(json!({}))));
    }
    #[test]
    fn dropped_page_retries_exact_cleanup_without_blocking_or_killing_a_sibling() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use tungstenite::Message;
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture");
        let address = listener.local_addr().expect("address");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut commands = Vec::new();
            for retry in [false, true] {
                let (stream, _) = listener.accept().expect("connect");
                stream.set_read_timeout(Some(Duration::from_secs(3))).expect("timeout");
                let mut socket = tungstenite::accept(stream).expect("handshake");
                if retry {
                    started_tx.send(()).expect("started");
                    release_rx.recv_timeout(Duration::from_secs(3)).expect("release");
                }
                for _ in 0..if retry { 4 } else { 5 } {
                    let Message::Text(text) = socket.read().expect("command") else {
                        panic!("text");
                    };
                    let command: Value = serde_json::from_str(&text).expect("json");
                    let response = if retry && command["method"] == "session.unsubscribe" {
                        // The first removal succeeded but its reply was lost.
                        json!({"id":command["id"],"type":"error","error":"invalid argument","message":"unknown subscription"})
                    } else if retry {
                        json!({"id":command["id"],"result":{}})
                    } else if command["method"] == "browsingContext.getTree" {
                        json!({"id":command["id"],"result":{"contexts":[{"context":"orphan"},{"context":"sibling"}]}})
                    } else {
                        json!({"id":command["id"],"type":"error","error":"unknown error","message":"retry later"})
                    };
                    socket.send(Message::Text(response.to_string().into())).expect("reply");
                    commands.push(command);
                }
            }
            commands
        });
        let group = SharedFirefoxSession::new("profile".into());
        let sibling = group.reserve(Arc::new(false.into()));
        let (mut page, process) = page(group, 1, "orphan");
        let control = Arc::clone(&page.lifecycle);
        control.cleanup.lock().expect("cleanup").endpoint = format!("ws://{address}/");
        for (method, result) in [
            ("session.subscribe", json!({"subscription":"owned-subscription"})),
            ("network.addDataCollector", json!({"collector":"owned-collector"})),
            ("script.addPreloadScript", json!({"script":"owned-preload"})),
        ] {
            page.record_bidi_result(method, &json!({}), &Ok(result));
        }
        page.record_bidi_result(
            "network.removeDataCollector",
            &json!({"collector":"owned-collector"}),
            &Err(JsonWsError::Timeout {
                method: "network.removeDataCollector".into(),
            }),
        );
        drop(page);
        assert!(!control.cleanup_done.load(Ordering::Acquire));
        assert_eq!(control.cleanup.lock().expect("cleanup").registrations.len(), 3);
        assert!(!control.terminate(Duration::ZERO));
        assert_eq!(process.terminations.load(Ordering::Acquire), 0);
        control.cleanup.lock().expect("cleanup").retry_after = Instant::now();
        assert!(!control.is_reaped());
        started_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("background retry");
        let start = Instant::now();
        for _ in 0..100 {
            assert!(!control.is_reaped());
        }
        assert!(start.elapsed() < Duration::from_millis(100));
        release_tx.send(()).expect("finish retry");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !control.is_reaped() {
            assert!(Instant::now() < deadline, "cleanup completed");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(process.terminations.load(Ordering::Acquire), 0);
        let commands = server.join().expect("server");
        assert_eq!(commands.len(), 9);
        for command in commands
            .iter()
            .filter(|command| command["method"] == "browsingContext.close")
        {
            assert_eq!(command["params"]["context"], "orphan");
        }
        drop(sibling);
    }
}
