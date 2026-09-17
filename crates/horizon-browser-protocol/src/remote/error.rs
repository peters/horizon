use std::fmt;

/// Why a control endpoint was rejected. Never carries the offending value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointProblem {
    Unparseable,
    MissingHost,
    NotHttps,
    Userinfo,
    QueryString,
    Fragment,
}

impl EndpointProblem {
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Unparseable => "is not a valid URL",
            Self::MissingHost => "has no host",
            Self::NotHttps => "must use https (plain http is allowed only for a loopback grid)",
            Self::Userinfo => "must not embed a username or password",
            Self::QueryString => "must not carry a query string",
            Self::Fragment => "must not carry a fragment",
        }
    }
}

/// Why a capability extension was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionProblem {
    /// Standard capabilities have no namespace; Horizon owns them.
    NotNamespaced,
    /// The extension would duplicate a normalized target field.
    ConflictsWithNormalizedField,
    /// The extension would place a credential into the public capabilities map.
    CarriesCredential,
    /// A nested option key is not a plain identifier, so it cannot be checked.
    InvalidOptionKey,
    /// The adapter merges the device request into this options object, so
    /// it must be an object.
    NotAnObject,
}

impl ExtensionProblem {
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NotNamespaced => "must be a namespaced `vendor:name` capability",
            Self::ConflictsWithNormalizedField => "duplicates a normalized target field (browser, platform, device)",
            Self::CarriesCredential => "would put a credential into the public capabilities map; bind it instead",
            Self::InvalidOptionKey => "has an option key that is not a plain identifier (letters, digits, . _ -)",
            Self::NotAnObject => "must be an object because the provider adapter adds the device request to it",
        }
    }
}

/// Which credential-reference rule failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialReferenceProblem {
    MalformedReference,
    MissingBinding,
    UnusedBinding,
    SlotRequired,
    MalformedSlot,
    /// `store: environment` names a variable that is not a POSIX identifier.
    MalformedVariable,
    /// Another binding for the same endpoint origin uses the same slot, so
    /// both would read and overwrite one OS-store item.
    DuplicateSlot,
}

impl CredentialReferenceProblem {
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MalformedReference => "is not a valid credential reference name",
            Self::MissingBinding => "is referenced by the authentication block but has no credential binding",
            Self::UnusedBinding => "is bound but not referenced by the authentication block",
            Self::SlotRequired => "needs a slot for this credential store",
            Self::MalformedSlot => "has a slot that is not a valid store path",
            Self::MalformedVariable => {
                "needs a 1-128 character environment variable name starting with an ASCII letter or _, followed by ASCII letters, digits, or _"
            }
            Self::DuplicateSlot => "shares its OS-store slot with another binding for the same endpoint origin",
        }
    }
}

/// Validation failure for the remote browser configuration.
///
/// Every variant names identifiers the user chose (provider, target, reference
/// or extension key) and never the configured value, so the message is safe
/// to show, log and export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteConfigError {
    InvalidProviderName {
        provider: String,
    },
    InvalidTargetName {
        target: String,
    },
    InvalidEndpoint {
        provider: String,
        problem: EndpointProblem,
    },
    InvalidCredential {
        provider: String,
        reference: String,
        problem: CredentialReferenceProblem,
    },
    InvalidLimit {
        provider: String,
        field: &'static str,
        min: u32,
        max: u32,
    },
    UnknownProvider {
        target: String,
        provider: String,
    },
    InvalidBrowserName {
        target: String,
    },
    InvalidPlatformName {
        target: String,
    },
    InvalidDeviceField {
        target: String,
        field: &'static str,
    },
    InvalidCapabilityExtension {
        target: String,
        key: String,
        problem: ExtensionProblem,
    },
    /// A portable definition may not carry machine-local credential bindings.
    ImportCarriesBindings {
        provider: String,
    },
    /// An imported provider must not silently change a trusted endpoint.
    ImportEndpointConflict {
        provider: String,
    },
    /// An imported provider must not change which references are needed
    /// while local bindings for the old shape still exist.
    ImportAuthenticationConflict {
        provider: String,
    },
}

impl fmt::Display for RemoteConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProviderName { provider } => {
                write!(
                    formatter,
                    "provider `{provider}`: name must be 1-64 characters of [A-Za-z0-9._-]"
                )
            }
            Self::InvalidTargetName { target } => {
                write!(
                    formatter,
                    "target `{target}`: name must be 1-64 characters of [A-Za-z0-9._-]"
                )
            }
            Self::InvalidEndpoint { provider, problem } => {
                write!(formatter, "provider `{provider}`: endpoint {}", problem.describe())
            }
            Self::InvalidCredential {
                provider,
                reference,
                problem,
            } => {
                write!(
                    formatter,
                    "provider `{provider}`: credential `{reference}` {}",
                    problem.describe()
                )
            }
            Self::InvalidLimit {
                provider,
                field,
                min,
                max,
            } => {
                write!(
                    formatter,
                    "provider `{provider}`: limits.{field} must be between {min} and {max}"
                )
            }
            Self::UnknownProvider { target, provider } => {
                write!(formatter, "target `{target}`: provider `{provider}` is not configured")
            }
            Self::InvalidBrowserName { target } => {
                write!(
                    formatter,
                    "target `{target}`: browser_name must be 1-64 printable ASCII characters"
                )
            }
            Self::InvalidPlatformName { target } => {
                write!(
                    formatter,
                    "target `{target}`: platform_name must be 1-64 printable ASCII characters"
                )
            }
            Self::InvalidDeviceField { target, field } => {
                write!(
                    formatter,
                    "target `{target}`: device.{field} must be 1-128 printable ASCII characters"
                )
            }
            Self::InvalidCapabilityExtension { target, key, problem } => {
                write!(
                    formatter,
                    "target `{target}`: capability extension `{key}` {}",
                    problem.describe()
                )
            }
            Self::ImportCarriesBindings { provider } => {
                write!(
                    formatter,
                    "imported provider `{provider}` carries credential bindings; bindings are machine-local and must be entered here"
                )
            }
            Self::ImportEndpointConflict { provider } => {
                write!(
                    formatter,
                    "imported provider `{provider}` points at a different endpoint than the trusted local definition; remove the local provider first to replace it"
                )
            }
            Self::ImportAuthenticationConflict { provider } => {
                write!(
                    formatter,
                    "imported provider `{provider}` needs different credential references than the ones bound locally; delete the local bindings first, then import again"
                )
            }
        }
    }
}

impl std::error::Error for RemoteConfigError {}

impl fmt::Display for EndpointProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.describe())
    }
}

impl fmt::Display for ExtensionProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.describe())
    }
}

impl fmt::Display for CredentialReferenceProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.describe())
    }
}
