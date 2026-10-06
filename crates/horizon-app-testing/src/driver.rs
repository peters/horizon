use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use horizon_browser::{ClassicTransport, WebDriverHttpError};
use serde_json::{Value, json};

use crate::catalog::Device;
use crate::contract::{App, Evidence, Platform, application_id, identifier, printable, version};
use crate::recipe::{Action, State, Target};
use crate::tree::{Identity, References, Snapshot};
use crate::{Error, Result};

mod actions;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Launch {
    device: Device,
    app: App,
    arguments: BTreeMap<String, String>,
    provider_app: String,
    tunnel_id: String,
    operation_id: String,
    evidence: Evidence,
}

impl Launch {
    /// Host-only material. Provider app and tunnel identifiers are deliberately not serializable.
    /// # Errors
    /// Refuses arbitrary app URLs, malformed IDs and unbounded launch arguments.
    pub fn new(
        device: Device,
        app: App,
        arguments: BTreeMap<String, String>,
        provider_app: String,
        tunnel_id: String,
        operation_id: String,
        evidence: Evidence,
    ) -> Result<Self> {
        let app_token = provider_app.strip_prefix("bs://").ok_or(Error::ContractInvalid)?;
        let id = match device.platform {
            Platform::Ios => app.bundle_id.as_deref(),
            Platform::Android => app.package.as_deref(),
        };
        if !(16..=128).contains(&app_token.len())
            || !app_token.bytes().all(|b| b.is_ascii_hexdigit())
            || !identifier(&tunnel_id)
            || !identifier(&operation_id)
            || arguments.len() > 32
            || arguments
                .iter()
                .any(|(key, value)| !identifier(key) || !printable(value, 2048))
            || !printable(&device.model, 128)
            || version(&device.os_version).is_none()
            || !id.is_some_and(|id| application_id(device.platform, id))
        {
            return Err(Error::ContractInvalid);
        }
        Ok(Self {
            device,
            app,
            arguments,
            provider_app,
            tunnel_id,
            operation_id,
            evidence,
        })
    }

    fn capabilities(&self) -> Value {
        let ios = self.device.platform == Platform::Ios;
        let mut caps = json!({
            "platformName": if ios { "iOS" } else { "Android" },
            "appium:automationName": if ios { "XCUITest" } else { "UiAutomator2" },
            "appium:deviceName": self.device.model,
            "appium:platformVersion": self.device.os_version,
            "appium:app": self.provider_app,
            "appium:newCommandTimeout": 90,
            "appium:noReset": false,
            "appium:fullReset": true,
            "bstack:options": {
                "local": true, "localIdentifier": self.tunnel_id,
                "buildName": format!("horizon-native-{}", self.operation_id),
                "sessionName": format!("native-{}", self.operation_id),
                "video": self.evidence.video, "debug": self.evidence.screenshots,
                "deviceLogs": self.evidence.logs_on_failure, "appiumLogs": self.evidence.logs_on_failure,
            }
        });
        if ios {
            caps["appium:processArguments"] = json!({"env": self.arguments});
        } else {
            // BrowserStack's Android default is legacy Appium; native lifecycle commands require this pinned driver generation.
            caps["bstack:options"]["appiumVersion"] = json!("2.19.0");
            caps["appium:optionalIntentArguments"] = Value::String(
                self.arguments
                    .iter()
                    .map(|(key, value)| format!("--es {} {}", quote(key), quote(value)))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        json!({"capabilities": {"alwaysMatch": caps}})
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub struct NativeDriver {
    transport: Arc<dyn ClassicTransport>,
    session_id: String,
    app_id: String,
    platform: Platform,
    arguments: BTreeMap<String, String>,
    references: References,
    closed: bool,
    deadline: Option<Instant>,
    lifetime_deadline: Option<Instant>,
}

impl NativeDriver {
    /// # Errors
    /// A failed create is uncertain, never a reason to retry allocation speculatively.
    pub fn allocate(transport: Arc<dyn ClassicTransport>, launch: &Launch) -> Result<Self> {
        Self::allocate_with_timeout(transport, launch, Duration::from_secs(180))
    }

    /// # Errors
    /// The trusted host supplies the remaining operation budget. Late successful replies
    /// still return the owned driver so it can be closed exactly before any handle is published.
    pub fn allocate_with_timeout(
        transport: Arc<dyn ClassicTransport>,
        launch: &Launch,
        timeout: Duration,
    ) -> Result<Self> {
        if timeout.is_zero() || timeout > Duration::from_secs(180) {
            return Err(Error::WaitTimeout);
        }
        let app_id = match launch.device.platform {
            Platform::Ios => launch.app.bundle_id.as_ref(),
            Platform::Android => launch.app.package.as_ref(),
        }
        .filter(|id| printable(id, 255))
        .ok_or(Error::ContractInvalid)?
        .clone();
        let response = transport
            .request("POST", "/session", Some(&launch.capabilities()), timeout)
            .map_err(|_| Error::AllocationUncertain)?;
        let session_id = response
            .pointer("/value/sessionId")
            .or_else(|| response.get("sessionId"))
            .and_then(Value::as_str)
            .filter(|s| safe_id(s))
            .ok_or(Error::AllocationUncertain)?
            .to_owned();
        Ok(Self {
            transport,
            session_id,
            app_id,
            platform: launch.device.platform,
            arguments: launch.arguments.clone(),
            references: References::default(),
            closed: false,
            deadline: None,
            lifetime_deadline: None,
        })
    }

    /// Host-only monotonic operation budget; later calls can shorten but never extend it.
    /// # Errors
    /// Commands must have a finite remaining lifetime of at most thirty minutes.
    pub fn limit_to_deadline(&mut self, deadline: Instant) -> Result<()> {
        let deadline = self
            .lifetime_deadline
            .map_or(deadline, |existing| existing.min(deadline));
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || remaining > Duration::from_mins(30) {
            return Err(Error::WaitTimeout);
        }
        self.lifetime_deadline = Some(deadline);
        Ok(())
    }

    /// Allocation journal callbacks may retain the exact provider ID privately; public outputs must use an opaque handle.
    pub fn record_allocation(&self, record: impl FnOnce(&str)) {
        record(&self.session_id);
    }

    /// # Errors
    /// Redacted transport/source failures. Every observation expires older refs.
    pub fn snapshot(&mut self) -> Result<Snapshot> {
        self.references.invalidate();
        let source = self
            .request("GET", "/source", None)?
            .get("value")
            .and_then(Value::as_str)
            .ok_or(Error::DriverInvalid)?
            .to_owned();
        let snapshot = self.references.snapshot(&source, &self.app_id)?;
        Ok(snapshot)
    }

    /// # Errors
    /// Invalid, stale, ambiguous or failing actions return typed failures without provider diagnostics.
    pub fn act(&mut self, action: &Action) -> Result<()> {
        action.validate()?;
        match action {
            Action::Wait {
                target,
                state,
                timeout_millis,
            } => self.wait(target, *state, Duration::from_millis(*timeout_millis)),
            Action::Assert { target, state } => {
                if self.matches(target, *state)? {
                    Ok(())
                } else {
                    Err(Error::AssertionFailed)
                }
            }
            Action::Screenshot {} => self.screenshot().map(|_| ()),
            other => {
                let result = actions::dispatch(self, other);
                self.references.invalidate();
                result
            }
        }
    }

    /// # Errors
    /// No element can satisfy enabled/disabled unless it exists; hidden may be absent.
    pub fn wait(&mut self, target: &Target, state: State, timeout: Duration) -> Result<()> {
        if timeout.is_zero() || timeout > Duration::from_secs(60) {
            return Err(Error::RecipeInvalid);
        }
        let requested = Instant::now() + timeout;
        let deadline = self.lifetime_deadline.map_or(requested, |limit| requested.min(limit));
        self.deadline = Some(deadline);
        let outcome = (|| {
            loop {
                if self.matches(target, state)? {
                    return Ok(());
                }
                let deadline = self
                    .lifetime_deadline
                    .map_or(deadline, |existing| existing.min(deadline));
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::WaitTimeout);
                }
                std::thread::sleep(remaining.min(Duration::from_millis(200)));
            }
        })();
        self.deadline = None;
        outcome
    }

    fn matches(&mut self, target: &Target, state: State) -> Result<bool> {
        let element = match self.element(target) {
            Ok(id) => id,
            Err(Error::TargetMissing | Error::ReferenceExpired) if matches!(target, Target::Ref(_)) => {
                return Err(Error::ReferenceExpired);
            }
            Err(Error::TargetMissing) => return Ok(matches!(state, State::Hidden)),
            Err(Error::ReferenceExpired) => return Ok(false),
            Err(error) => return Err(error),
        };
        let suffix = match state {
            State::Visible | State::Hidden => "displayed",
            State::Enabled | State::Disabled => "enabled",
        };
        let response = match self.request("GET", &format!("/element/{element}/{suffix}"), None) {
            Ok(response) => response,
            Err(Error::ReferenceExpired | Error::TargetMissing) if !matches!(target, Target::Ref(_)) => {
                return Ok(false);
            }
            Err(Error::TargetMissing) if matches!(target, Target::Ref(_)) => return Err(Error::ReferenceExpired),
            Err(error) => return Err(error),
        };
        let observed = response
            .get("value")
            .and_then(Value::as_bool)
            .ok_or(Error::DriverInvalid)?;
        Ok(observed == matches!(state, State::Visible | State::Enabled))
    }

    fn element(&mut self, target: &Target) -> Result<String> {
        let (using, value) = match target {
            Target::Ref(_) => return self.observed_element(target),
            Target::Identifier(value) => {
                if self.platform == Platform::Android {
                    // Resolve the exact source identifier; native ID locators may add an application prefix.
                    self.snapshot()?;
                    return self.observed_element(target);
                }
                ("accessibility id", value.clone())
            }
            Target::Label(_) => {
                self.snapshot()?;
                return self.observed_element(target);
            }
            Target::Coordinates(_) => return Err(Error::RecipeInvalid),
        };
        let response = self.request("POST", "/elements", Some(&json!({"using":using, "value":value})))?;
        let elements = response
            .get("value")
            .and_then(Value::as_array)
            .ok_or(Error::DriverInvalid)?;
        let [element] = elements.as_slice() else {
            return Err(if elements.is_empty() {
                Error::TargetMissing
            } else {
                Error::TargetAmbiguous
            });
        };
        element_id(element)
    }

    fn observed_element(&mut self, target: &Target) -> Result<String> {
        let (xpath, retained, identity) = self.references.binding(target)?;
        let response = self.request("POST", "/elements", Some(&json!({"using":"xpath", "value":xpath})))?;
        let elements = response
            .get("value")
            .and_then(Value::as_array)
            .ok_or(Error::DriverInvalid)?;
        let [element] = elements.as_slice() else {
            return Err(if elements.is_empty() {
                Error::ReferenceExpired
            } else {
                Error::TargetAmbiguous
            });
        };
        let element = element_id(element)?;
        if retained.is_some_and(|known| known != element) {
            return Err(Error::ReferenceExpired);
        }
        self.verify_identity(&element, &identity)?;
        self.references.bind(target, &element)?;
        Ok(element)
    }

    fn verify_identity(&self, element: &str, identity: &Identity) -> Result<()> {
        for (key, expected) in &identity.attributes {
            let response = self.request("GET", &format!("/element/{element}/attribute/{key}"), None)?;
            let value = response.get("value");
            let matches = value.and_then(Value::as_str) == Some(expected.as_str())
                || (key == "password"
                    && value
                        .and_then(Value::as_bool)
                        .is_some_and(|flag| expected == if flag { "true" } else { "false" }));
            if !matches {
                return Err(Error::ReferenceExpired);
            }
        }
        if let Some(bounds) = identity.bounds {
            let response = self.request("GET", &format!("/element/{element}/rect"), None)?;
            for (key, expected) in [
                ("x", bounds.x),
                ("y", bounds.y),
                ("width", bounds.width),
                ("height", bounds.height),
            ] {
                if response.pointer(&format!("/value/{key}")).and_then(Value::as_f64) != Some(expected) {
                    return Err(Error::ReferenceExpired);
                }
            }
        }
        Ok(())
    }

    /// # Errors
    /// Refuses malformed images and never returns base64 provider payloads on failure.
    pub fn screenshot(&self) -> Result<Vec<u8>> {
        let response = self.request("GET", "/screenshot", None)?;
        let encoded = response
            .get("value")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 48 * 1024 * 1024)
            .ok_or(Error::DriverInvalid)?;
        // Android's driver wraps Base64 at 76 columns; accept CR/LF only, preserving strict alphabet/padding checks.
        let encoded = if encoded.contains(['\r', '\n']) {
            std::borrow::Cow::Owned(
                encoded
                    .chars()
                    .filter(|character| !matches!(character, '\r' | '\n'))
                    .collect::<String>(),
            )
        } else {
            std::borrow::Cow::Borrowed(encoded)
        };
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| Error::DriverInvalid)?;
        let decoder = png::Decoder::new_with_limits(
            std::io::Cursor::new(&png),
            png::Limits {
                bytes: 64 * 1024 * 1024,
            },
        );
        let mut reader = decoder.read_info().map_err(|_| Error::DriverInvalid)?;
        let info = reader.info();
        if info.width == 0
            || info.height == 0
            || u64::from(info.width) * u64::from(info.height) > 16_000_000
            || info.animation_control.is_some()
        {
            return Err(Error::DriverInvalid);
        }
        let size = reader
            .output_buffer_size()
            .filter(|size| *size <= 64 * 1024 * 1024)
            .ok_or(Error::DriverInvalid)?;
        reader
            .next_frame(&mut vec![0; size])
            .map_err(|_| Error::DriverInvalid)?;
        reader.finish().map_err(|_| Error::DriverInvalid)?;
        Ok(png)
    }

    /// # Errors
    /// Release is idempotent after confirmed success; uncertain release remains retryable by the journal owner.
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        // Cleanup remains allowed after expiry; no action may extend the operation budget.
        let expired = self.lifetime_deadline.take();
        let response = match self.request("DELETE", "", None) {
            Ok(response) => response,
            Err(error) => {
                self.lifetime_deadline = expired;
                return Err(error);
            }
        };
        if response.as_object().is_none_or(|object| object.len() != 1) || response.get("value") != Some(&Value::Null) {
            self.lifetime_deadline = expired;
            return Err(Error::DriverInvalid);
        }
        self.closed = true;
        self.references.invalidate();
        Ok(())
    }

    fn request(&self, method: &str, suffix: &str, body: Option<&Value>) -> Result<Value> {
        self.request_with_limit(method, suffix, body, COMMAND_TIMEOUT)
    }

    fn request_with_limit(&self, method: &str, suffix: &str, body: Option<&Value>, limit: Duration) -> Result<Value> {
        if self.closed {
            return Err(Error::SessionClosed);
        }
        let deadline = match (self.deadline, self.lifetime_deadline) {
            (Some(wait), Some(lifetime)) => Some(wait.min(lifetime)),
            (wait, lifetime) => wait.or(lifetime),
        };
        let timeout = deadline.map_or(limit, |d| d.saturating_duration_since(Instant::now()).min(limit));
        if timeout.is_zero() {
            return Err(Error::WaitTimeout);
        }
        let response = self
            .transport
            .request(method, &format!("/session/{}{suffix}", self.session_id), body, timeout)
            .map_err(|error| {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return Error::WaitTimeout;
                }
                match error {
                    WebDriverHttpError::WebDriver { error, .. }
                        if error == "stale element reference" || error == "no such element" =>
                    {
                        Error::ReferenceExpired
                    }
                    _ => Error::TransportFailed,
                }
            });
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Error::WaitTimeout);
        }
        response
    }

    fn execute(&self, script: &str, arguments: &Value) -> Result<Value> {
        // Cold launches on older real iPhones can exceed the regular 15s command budget.
        // One request receives up to 60s, always capped by the original session deadline.
        let limit = if matches!(script, "mobile: launchApp" | "mobile: startActivity") {
            Duration::from_secs(60)
        } else {
            COMMAND_TIMEOUT
        };
        self.request_with_limit(
            "POST",
            "/execute/sync",
            Some(&json!({"script":script,"args":[arguments]})),
            limit,
        )
    }
}

fn safe_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn element_id(element: &Value) -> Result<String> {
    element
        .get("element-6066-11e4-a52e-4f735466cecf")
        .or_else(|| element.get("ELEMENT"))
        .and_then(Value::as_str)
        .filter(|s| safe_id(s))
        .map(str::to_owned)
        .ok_or(Error::DriverInvalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::WebDriverHttpError;
    use std::sync::Mutex;

    struct SlowIdentity;
    impl ClassicTransport for SlowIdentity {
        fn request(
            &self,
            _method: &str,
            path: &str,
            _body: Option<&Value>,
            _timeout: Duration,
        ) -> std::result::Result<Value, WebDriverHttpError> {
            if path.ends_with("/elements") {
                return Ok(json!({"value":[{"element-6066-11e4-a52e-4f735466cecf":"menu-element"}]}));
            }
            if path.ends_with("/attribute/type") {
                std::thread::sleep(Duration::from_millis(20));
                return Ok(json!({"value":"XCUIElementTypeButton"}));
            }
            Ok(json!({"value":"menu.open"}))
        }
    }

    #[test]
    fn reference_expiring_during_identity_verification_cannot_dispatch() {
        let mut references = References::default();
        let snapshot = references
            .snapshot("<XCUIElementTypeButton name='menu.open'/>", "app")
            .unwrap();
        references
            .bind(
                &Target::Ref(snapshot.nodes[0].semantic.reference.clone()),
                "menu-element",
            )
            .unwrap();
        references.expire_in(Duration::from_millis(5));
        let mut driver = NativeDriver {
            transport: Arc::new(SlowIdentity),
            session_id: "session".into(),
            app_id: "app".into(),
            platform: Platform::Ios,
            arguments: BTreeMap::new(),
            references,
            closed: false,
            deadline: None,
            lifetime_deadline: None,
        };
        assert_eq!(
            driver
                .observed_element(&Target::Ref(snapshot.nodes[0].semantic.reference.clone()))
                .err(),
            Some(Error::ReferenceExpired)
        );
    }

    struct LateAction(Mutex<Vec<(String, Duration)>>);
    impl ClassicTransport for LateAction {
        fn request(
            &self,
            method: &str,
            _path: &str,
            _body: Option<&Value>,
            timeout: Duration,
        ) -> std::result::Result<Value, WebDriverHttpError> {
            self.0.lock().unwrap().push((method.to_owned(), timeout));
            if method != "DELETE" {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(json!({"value":null}))
        }
    }

    #[test]
    fn late_action_cannot_pass_expiry_but_exact_cleanup_remains_allowed() {
        let transport = Arc::new(LateAction(Mutex::new(Vec::new())));
        let mut driver = NativeDriver {
            transport: transport.clone(),
            session_id: "owned-session".into(),
            app_id: "app".into(),
            platform: Platform::Ios,
            arguments: BTreeMap::new(),
            references: References::default(),
            closed: false,
            deadline: None,
            lifetime_deadline: None,
        };
        let deadline = Instant::now() + Duration::from_millis(10);
        driver.limit_to_deadline(deadline).unwrap();
        assert_eq!(driver.request("POST", "/actions", None).err(), Some(Error::WaitTimeout));
        assert_eq!(driver.request("POST", "/actions", None).err(), Some(Error::WaitTimeout));
        assert_eq!(
            driver.limit_to_deadline(Instant::now() + Duration::from_secs(30)).err(),
            Some(Error::WaitTimeout)
        );
        driver.close().unwrap();
        driver.close().unwrap();
        let calls = transport.0.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "POST");
        assert!(calls[0].1 <= Duration::from_millis(10));
        assert_eq!(calls[1].0, "DELETE");
        assert_eq!(calls[1].1, COMMAND_TIMEOUT);
    }
    #[test]
    fn cold_launch_uses_one_longer_request_without_renewing_the_original_deadline() {
        let transport = Arc::new(LateAction(Mutex::new(Vec::new())));
        let mut driver = NativeDriver {
            transport: transport.clone(),
            session_id: "owned-session".into(),
            app_id: "app".into(),
            platform: Platform::Ios,
            arguments: BTreeMap::new(),
            references: References::default(),
            closed: false,
            deadline: None,
            lifetime_deadline: None,
        };
        driver.execute("mobile: launchApp", &json!({})).unwrap();
        let deadline = Instant::now() + Duration::from_millis(10);
        driver.limit_to_deadline(deadline).unwrap();
        assert_eq!(driver.execute("mobile: launchApp", &json!({})), Err(Error::WaitTimeout));
        assert_eq!(driver.lifetime_deadline, Some(deadline));
        let calls = transport.0.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1, Duration::from_secs(60));
        assert!(calls[1].1 <= Duration::from_millis(10));
    }
}
