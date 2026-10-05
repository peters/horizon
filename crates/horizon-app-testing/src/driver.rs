use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use horizon_browser::ClassicTransport;
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
            "bstack:options": {
                "local": true, "localIdentifier": self.tunnel_id,
                "buildName": format!("horizon-native-{}", self.operation_id),
                "sessionName": format!("native-{}", self.operation_id),
                "video": self.evidence.video, "debug": self.evidence.logs_on_failure,
            }
        });
        if ios {
            caps["appium:processArguments"] = json!({"env": self.arguments});
        } else {
            caps["appium:settings"] = json!({"disableIdLocatorAutocompletion":true});
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
}

impl NativeDriver {
    /// # Errors
    /// A failed create is uncertain, never a reason to retry allocation speculatively.
    pub fn allocate(transport: Arc<dyn ClassicTransport>, launch: &Launch) -> Result<Self> {
        let app_id = match launch.device.platform {
            Platform::Ios => launch.app.bundle_id.as_ref(),
            Platform::Android => launch.app.package.as_ref(),
        }
        .filter(|id| printable(id, 255))
        .ok_or(Error::ContractInvalid)?
        .clone();
        let response = transport
            .request(
                "POST",
                "/session",
                Some(&launch.capabilities()),
                Duration::from_secs(180),
            )
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
        })
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
        let response = self.request(
            "POST",
            "/elements",
            Some(&json!({
                "using":"xpath", "value":"//*[not(self::AppiumAUT or self::hierarchy)]"
            })),
        )?;
        let elements = response
            .get("value")
            .and_then(Value::as_array)
            .ok_or(Error::DriverInvalid)?;
        let ids = elements.iter().map(element_id).collect::<Result<Vec<_>>>()?;
        let confirmed = self.request("GET", "/source", None)?;
        if confirmed.get("value").and_then(Value::as_str) != Some(source.as_str()) {
            self.references.invalidate();
            return Err(Error::ReferenceExpired);
        }
        self.references.bind_elements(&ids)?;
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
        let deadline = Instant::now() + timeout;
        self.deadline = Some(deadline);
        let outcome = (|| {
            loop {
                if self.matches(target, state)? {
                    return Ok(());
                }
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
            Err(Error::TargetMissing) => return Ok(matches!(state, State::Hidden)),
            Err(error) => return Err(error),
        };
        let suffix = match state {
            State::Visible | State::Hidden => "displayed",
            State::Enabled | State::Disabled => "enabled",
        };
        let observed = self
            .request("GET", &format!("/element/{element}/{suffix}"), None)?
            .get("value")
            .and_then(Value::as_bool)
            .ok_or(Error::DriverInvalid)?;
        Ok(observed == matches!(state, State::Visible | State::Enabled))
    }

    fn element(&mut self, target: &Target) -> Result<String> {
        let (using, value) = match target {
            Target::Ref(_) => return self.observed_element(target),
            Target::Identifier(value) => (
                if self.platform == Platform::Ios {
                    "accessibility id"
                } else {
                    "id"
                },
                value.clone(),
            ),
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

    fn observed_element(&self, target: &Target) -> Result<String> {
        let (element, identity) = self.references.element(target)?;
        self.verify_identity(&element, &identity)?;
        if self.references.element(target)?.0 != element {
            return Err(Error::ReferenceExpired);
        }
        Ok(element)
    }

    fn verify_identity(&self, element: &str, identity: &Identity) -> Result<()> {
        for (key, expected) in &identity.attributes {
            let response = self.request("GET", &format!("/element/{element}/attribute/{key}"), None)?;
            if response.get("value").and_then(Value::as_str) != Some(expected.as_str()) {
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
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded)
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
        self.request("DELETE", "", None)?;
        self.closed = true;
        self.references.invalidate();
        Ok(())
    }

    fn request(&self, method: &str, suffix: &str, body: Option<&Value>) -> Result<Value> {
        if self.closed {
            return Err(Error::SessionClosed);
        }
        let timeout = self.deadline.map_or(COMMAND_TIMEOUT, |d| {
            d.saturating_duration_since(Instant::now()).min(COMMAND_TIMEOUT)
        });
        if timeout.is_zero() {
            return Err(Error::WaitTimeout);
        }
        let response = self
            .transport
            .request(method, &format!("/session/{}{suffix}", self.session_id), body, timeout)
            .map_err(|_| {
                if self.deadline.is_some_and(|d| Instant::now() >= d) {
                    Error::WaitTimeout
                } else {
                    Error::TransportFailed
                }
            });
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Error::WaitTimeout);
        }
        response
    }

    fn execute(&self, script: &str, arguments: &Value) -> Result<Value> {
        self.request(
            "POST",
            "/execute/sync",
            Some(&json!({"script":script,"args":[arguments]})),
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

    struct SlowIdentity;
    impl ClassicTransport for SlowIdentity {
        fn request(
            &self,
            _method: &str,
            path: &str,
            _body: Option<&Value>,
            _timeout: Duration,
        ) -> std::result::Result<Value, WebDriverHttpError> {
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
        references.bind_elements(&["menu-element".into()]).unwrap();
        references.expire_in(Duration::from_millis(5));
        let driver = NativeDriver {
            transport: Arc::new(SlowIdentity),
            session_id: "session".into(),
            app_id: "app".into(),
            platform: Platform::Ios,
            arguments: BTreeMap::new(),
            references,
            closed: false,
            deadline: None,
        };
        assert_eq!(
            driver
                .observed_element(&Target::Ref(snapshot.nodes[0].semantic.reference.clone()))
                .err(),
            Some(Error::ReferenceExpired)
        );
    }
}
