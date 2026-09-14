use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::Url;

use super::error::{CredentialReferenceProblem, EndpointProblem, RemoteConfigError};

/// Adapter that turns a provider profile into sessions. The generic `WebDriver`
/// adapter is the baseline; named adapters appear only for demonstrated
/// provider differences.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAdapterKind {
    #[default]
    Webdriver,
}

/// Validated `WebDriver` control endpoint (scheme, host, port and base path).
///
/// HTTPS is required. Plain HTTP is accepted only for loopback hosts so a local
/// test grid stays reachable, and that case is reported by
/// [`ControlEndpoint::is_loopback_http`]. Userinfo, query strings and
/// fragments are rejected so a credential can never ride inside the URL.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ControlEndpoint(String);

impl ControlEndpoint {
    /// Parse and canonicalize an endpoint URL.
    ///
    /// # Errors
    /// Returns the [`EndpointProblem`] without echoing the input.
    pub fn parse(input: &str) -> Result<Self, EndpointProblem> {
        let input = input.trim();
        let url = Url::parse(input).map_err(|_| EndpointProblem::Unparseable)?;
        // `Url::username()` is empty for both "no userinfo" and an empty
        // username, so look at the authority text itself.
        if authority_has_userinfo(input) || !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointProblem::Userinfo);
        }
        if url.query().is_some() {
            return Err(EndpointProblem::QueryString);
        }
        if url.fragment().is_some() {
            return Err(EndpointProblem::Fragment);
        }
        let host = url.host_str().ok_or(EndpointProblem::MissingHost)?;
        match url.scheme() {
            "https" => {}
            "http" if is_loopback_host(host) => {}
            _ => return Err(EndpointProblem::NotHttps),
        }
        let mut canonical = url.to_string();
        while canonical.ends_with('/') {
            canonical.pop();
        }
        Ok(Self(canonical))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Scheme, host and port: the only place a bound credential may be sent.
    #[must_use]
    pub fn origin(&self) -> String {
        Url::parse(&self.0)
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn is_loopback_http(&self) -> bool {
        self.0.starts_with("http://")
    }
}

fn authority_has_userinfo(input: &str) -> bool {
    input
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
        .is_some_and(|authority| authority.contains('@'))
}

fn is_loopback_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    bare == "localhost"
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

impl TryFrom<String> for ControlEndpoint {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).map_err(|problem| format!("endpoint {problem}"))
    }
}

impl From<ControlEndpoint> for String {
    fn from(value: ControlEndpoint) -> Self {
        value.0
    }
}

/// Name of a credential binding, chosen by the user; never the secret itself.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Deserialize, Serialize)]
#[serde(transparent)]
pub struct CredentialReference(String);

impl CredentialReference {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn is_well_formed(&self) -> bool {
        valid_identifier(&self.0, 64)
    }
}

impl From<&str> for CredentialReference {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// How the transport authenticates to the control endpoint. Values are
/// resolved from bindings inside the transport and never appear here.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteAuthentication {
    /// A struct variant so serde rejects stray fields next to `kind: none`.
    None {},
    Basic {
        username_ref: CredentialReference,
        password_ref: CredentialReference,
    },
    Bearer {
        token_ref: CredentialReference,
    },
}

impl Default for RemoteAuthentication {
    fn default() -> Self {
        Self::None {}
    }
}

impl RemoteAuthentication {
    /// Every reference the authentication block needs bound.
    #[must_use]
    pub fn references(&self) -> Vec<&CredentialReference> {
        match self {
            Self::None {} => Vec::new(),
            Self::Basic {
                username_ref,
                password_ref,
            } => vec![username_ref, password_ref],
            Self::Bearer { token_ref } => vec![token_ref],
        }
    }
}

/// Where a bound credential value lives on this machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStoreKind {
    /// Entered in Horizon, held in a scoped in-memory sink, discarded on exit.
    Session,
    /// Persisted in the platform credential store under `slot`.
    OsKeychain,
}

/// Machine-local binding from a reference to a store location. Carries no value.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub store: CredentialStoreKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
}

impl CredentialBinding {
    fn problem(&self) -> Option<CredentialReferenceProblem> {
        match (self.store, self.slot.as_deref()) {
            (CredentialStoreKind::OsKeychain, None) => Some(CredentialReferenceProblem::SlotRequired),
            (_, Some(slot)) if !valid_slot(slot) => Some(CredentialReferenceProblem::MalformedSlot),
            _ => None,
        }
    }
}

/// Bounds Horizon enforces locally; provider quotas remain authoritative.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteSessionLimits {
    pub max_sessions: u32,
    pub allocation_timeout_seconds: u32,
    pub idle_release_seconds: u32,
    pub max_session_seconds: u32,
}

impl RemoteSessionLimits {
    pub const MAX_SESSIONS: (u32, u32) = (1, 8);
    pub const ALLOCATION_TIMEOUT_SECONDS: (u32, u32) = (10, 600);
    pub const IDLE_RELEASE_SECONDS: (u32, u32) = (30, 3_600);
    pub const MAX_SESSION_SECONDS: (u32, u32) = (60, 14_400);

    fn validate(self, provider: &str) -> Result<(), RemoteConfigError> {
        let checks = [
            ("max_sessions", self.max_sessions, Self::MAX_SESSIONS),
            (
                "allocation_timeout_seconds",
                self.allocation_timeout_seconds,
                Self::ALLOCATION_TIMEOUT_SECONDS,
            ),
            (
                "idle_release_seconds",
                self.idle_release_seconds,
                Self::IDLE_RELEASE_SECONDS,
            ),
            (
                "max_session_seconds",
                self.max_session_seconds,
                Self::MAX_SESSION_SECONDS,
            ),
        ];
        for (field, value, (min, max)) in checks {
            if !(min..=max).contains(&value) {
                return Err(RemoteConfigError::InvalidLimit {
                    provider: provider.to_string(),
                    field,
                    min,
                    max,
                });
            }
        }
        Ok(())
    }
}

impl Default for RemoteSessionLimits {
    fn default() -> Self {
        Self {
            max_sessions: 1,
            allocation_timeout_seconds: 120,
            idle_release_seconds: 180,
            max_session_seconds: 1_800,
        }
    }
}

/// One remote device service: endpoint, authentication shape, local bindings and limits.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteProviderProfile {
    #[serde(default)]
    pub adapter: RemoteAdapterKind,
    pub endpoint: ControlEndpoint,
    #[serde(default)]
    pub authentication: RemoteAuthentication,
    /// Machine-local; stripped from portable exports.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub credential_bindings: BTreeMap<CredentialReference, CredentialBinding>,
    #[serde(default)]
    pub limits: RemoteSessionLimits,
}

impl RemoteProviderProfile {
    /// Structural validation: limits, reference names, and the shape of every
    /// binding that is present. A binding that is merely missing is not a
    /// definition error; that is the readiness state [`Self::validate_bindings`] reports.
    pub(super) fn validate_definition(&self, provider: &str) -> Result<(), RemoteConfigError> {
        self.limits.validate(provider)?;
        for reference in self.authentication.references() {
            if !reference.is_well_formed() {
                return Err(credential_error(
                    provider,
                    reference,
                    CredentialReferenceProblem::MalformedReference,
                ));
            }
        }
        for (reference, binding) in &self.credential_bindings {
            if !reference.is_well_formed() {
                return Err(credential_error(
                    provider,
                    reference,
                    CredentialReferenceProblem::MalformedReference,
                ));
            }
            if let Some(problem) = binding.problem() {
                return Err(credential_error(provider, reference, problem));
            }
        }
        Ok(())
    }

    /// Bindings must cover exactly the references the authentication block needs.
    pub(super) fn validate_bindings(&self, provider: &str) -> Result<(), RemoteConfigError> {
        let required = self.authentication.references();
        for reference in &required {
            if !self.credential_bindings.contains_key(*reference) {
                return Err(credential_error(
                    provider,
                    reference,
                    CredentialReferenceProblem::MissingBinding,
                ));
            }
        }
        for reference in self.credential_bindings.keys() {
            if !required.contains(&reference) {
                return Err(credential_error(
                    provider,
                    reference,
                    CredentialReferenceProblem::UnusedBinding,
                ));
            }
        }
        Ok(())
    }
}

fn credential_error(
    provider: &str,
    reference: &CredentialReference,
    problem: CredentialReferenceProblem,
) -> RemoteConfigError {
    RemoteConfigError::InvalidCredential {
        provider: provider.to_string(),
        reference: reference.as_str().to_string(),
        problem,
    }
}

pub(super) fn valid_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_slot(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}
