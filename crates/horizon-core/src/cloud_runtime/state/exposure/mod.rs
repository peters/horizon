//! Original inbound startup intent, never approval or permission to recreate a keeper.
mod binding;
mod provider;
pub use provider::{ProviderPhase, ProviderProgress, Step as ProviderStep};
#[cfg(feature = "cloud-exposure")]
mod lifecycle;
use crate::cloud_runtime::allocation::legacy::unique_json::UniqueValue;
pub use binding::{Binding, ProviderBinding};
use horizon_cloud_protocol::exposure::{EXPOSURE_LEASE_SECONDS, ExposureMode, ExposureRequest};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(in crate::cloud_runtime) const RECORD_VERSION: u32 = 7;
pub(in crate::cloud_runtime) const ALLOCATION_VERSION: u32 = 5;

const MAX_LIFETIME_MILLIS: i64 = EXPOSURE_LEASE_SECONDS.cast_signed() * 1_000;
const MAX_SERVICES: usize = 8;

pub use horizon_cloud_protocol::exposure::Route;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub id: Uuid,
    pub policy: ExposureRequest,
    pub route: Route,
}

impl Service {
    fn valid(&self) -> bool {
        !self.id.is_nil()
            && self.policy.mode != ExposureMode::Off
            && self.policy.validate().is_ok()
            && self.route.validate_for(&self.policy).is_ok()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub operation: Uuid,
    pub service: Service,
    pub started_at_millis: i64,
    pub expires_at_millis: i64,
    pub phase: Phase,
}

/// Configured private gate observations, never public/TLS readiness.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    Opening {},
    Active { ingress: u16 },
    Renewing { ingress: u16 },
    Closing { ingress: Option<u16> },
    Closed { ingress: Option<u16> },
}
impl Phase {
    #[must_use]
    pub const fn ingress(self) -> Option<u16> {
        match self {
            Self::Opening {} => None,
            Self::Active { ingress } | Self::Renewing { ingress } => Some(ingress),
            Self::Closing { ingress } | Self::Closed { ingress } => ingress,
        }
    }
}

impl Intent {
    fn valid(&self) -> bool {
        !self.operation.is_nil()
            && self.service.valid()
            && self
                .phase
                .ingress()
                .is_none_or(|port| self.service.route.accepts_ingress(port))
            && self.started_at_millis > 0
            && self.expires_at_millis > self.started_at_millis
            && self
                .expires_at_millis
                .checked_sub(self.started_at_millis)
                .is_some_and(|duration| duration <= MAX_LIFETIME_MILLIS)
    }
}

/// A keeper observation says nothing about service reachability, source or TLS.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Startup {
    Starting {},
    Observed { custody: Uuid },
    Unknown { custody: Option<Uuid> },
    Finishing { custody: Uuid, operation: Uuid },
    FinishUnknown { custody: Uuid, operation: Uuid },
    Closed { custody: Uuid },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "Fields")]
pub struct Record {
    binding: Binding,
    epoch: Uuid,
    startup: Startup,
    intents: Vec<Intent>,
    observed_at_millis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<ProviderProgress>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fields {
    binding: Binding,
    epoch: Uuid,
    startup: Startup,
    intents: Vec<Intent>,
    observed_at_millis: i64,
    #[serde(default)]
    provider: Option<UniqueValue>,
}

const INVALID: &str = "Invalid retained exposure startup intent";
impl TryFrom<Fields> for Record {
    type Error = &'static str;
    fn try_from(fields: Fields) -> Result<Self, Self::Error> {
        let provider = fields
            .provider
            .map(|value| {
                let provider: ProviderProgress = serde_json::from_value(value.0.clone()).map_err(|_| INVALID)?;
                if serde_json::to_value(&provider).map_err(|_| INVALID)? != value.0 {
                    return Err(INVALID);
                }
                Ok(provider)
            })
            .transpose()?;
        let record = Self {
            binding: fields.binding,
            epoch: fields.epoch,
            startup: fields.startup,
            intents: fields.intents,
            observed_at_millis: fields.observed_at_millis,
            provider,
        };
        if record.valid() { Ok(record) } else { Err(INVALID) }
    }
}

impl Record {
    /// Creates metadata only. The existing owner controller must verify native
    /// approval and original saved membership, then durably save BEFORE any effect.
    /// # Errors
    /// Refuses invalid scope, service, generation or deadline overflow.
    #[cfg(feature = "cloud-exposure")]
    pub fn starting(
        binding: Binding,
        epoch: Uuid,
        service: Service,
        operation: Uuid,
        now: i64,
    ) -> Result<Self, super::Error> {
        let expires_at_millis = now
            .checked_add(MAX_LIFETIME_MILLIS)
            .ok_or(super::Error::Invalid(INVALID))?;
        Self::try_from(Fields {
            binding,
            epoch,
            startup: Startup::Starting {},
            observed_at_millis: now,
            provider: None,
            intents: vec![Intent {
                operation,
                service,
                started_at_millis: now,
                expires_at_millis,
                phase: Phase::Opening {},
            }],
        })
        .map_err(super::Error::Invalid)
    }

    pub(in crate::cloud_runtime) fn valid(&self) -> bool {
        self.binding.valid()
            && self.provider.as_ref().is_none_or(|provider| provider.valid(self.epoch))
            && !self.epoch.is_nil()
            && self.observed_at_millis > 0
            && match self.startup {
                Startup::Starting {} => self.intents.iter().all(|intent| intent.phase == (Phase::Opening {})),
                Startup::Observed { custody } | Startup::Unknown { custody: Some(custody) } => !custody.is_nil(),
                Startup::Closed { custody } => {
                    self.provider.as_ref().is_none_or(ProviderProgress::absent)
                        && !custody.is_nil()
                        && self
                            .intents
                            .iter()
                            .all(|intent| matches!(intent.phase, Phase::Closed { .. }))
                }
                Startup::Finishing { custody, operation } | Startup::FinishUnknown { custody, operation } => {
                    !custody.is_nil()
                        && !operation.is_nil()
                        && self.intents.iter().all(|intent| {
                            intent.operation != operation
                                && matches!(intent.phase, Phase::Closing { .. } | Phase::Closed { .. })
                        })
                }
                Startup::Unknown { custody: None } => {
                    self.intents.iter().all(|intent| intent.phase == (Phase::Opening {}))
                }
            }
            && !self.intents.is_empty()
            && self.intents.len() <= MAX_SERVICES
            && self.intents.iter().enumerate().all(|(i, intent)| {
                intent.valid()
                    && intent.started_at_millis <= self.observed_at_millis
                    && self.intents[..i]
                        .iter()
                        .all(|other| other.operation != intent.operation && other.service.id != intent.service.id)
                    && self.intents[..i].iter().all(|other| {
                        intent
                            .phase
                            .ingress()
                            .is_none_or(|port| other.phase.ingress() != Some(port))
                    })
            })
    }

    #[must_use]
    pub fn provider(&self) -> Option<&ProviderProgress> {
        self.provider.as_ref()
    }
    #[must_use]
    pub const fn binding(&self) -> &Binding {
        &self.binding
    }
    #[must_use]
    pub const fn epoch(&self) -> Uuid {
        self.epoch
    }
    #[must_use]
    pub const fn startup(&self) -> Startup {
        self.startup
    }
    #[must_use]
    pub fn intents(&self) -> &[Intent] {
        &self.intents
    }

    /// Records an actual same-epoch worker observation. This does not open a service.
    /// # Errors
    /// Refuses uncertain replacement, different custody, clock rollback or expired startup.
    #[cfg(feature = "cloud-exposure")]
    pub fn observed(&mut self, epoch: Uuid, custody: Uuid, now: i64) -> Result<(), super::Error> {
        if epoch != self.epoch
            || now < self.observed_at_millis
            || custody.is_nil()
            || !match self.startup {
                Startup::Starting {} => true,
                Startup::Observed { custody: old } => old == custody,
                Startup::Unknown { .. }
                | Startup::Finishing { .. }
                | Startup::FinishUnknown { .. }
                | Startup::Closed { .. } => false,
            }
            || self
                .intents
                .iter()
                .any(|intent| now < intent.started_at_millis || now >= intent.expires_at_millis)
        {
            return Err(super::Error::Invalid("Exposure startup remains unresolved or expired"));
        }
        self.startup = Startup::Observed { custody };
        self.observed_at_millis = now;
        Ok(())
    }

    /// Retains uncertainty for the original generation without changing its deadline.
    /// # Errors
    /// Refuses a successor epoch; no restart/reopen operation is provided here.
    #[cfg(feature = "cloud-exposure")]
    pub fn uncertain(&mut self, epoch: Uuid) -> Result<(), super::Error> {
        if epoch != self.epoch {
            return Err(super::Error::Invalid(INVALID));
        }
        self.startup = match self.startup {
            Startup::Starting {} => Startup::Unknown { custody: None },
            Startup::Observed { custody } => Startup::Unknown { custody: Some(custody) },
            Startup::Unknown { custody } => Startup::Unknown { custody },
            Startup::Finishing { custody, operation } | Startup::FinishUnknown { custody, operation } => {
                Startup::FinishUnknown { custody, operation }
            }
            Startup::Closed { .. } => return Err(super::Error::Invalid(INVALID)),
        };
        Ok(())
    }
    /// Retain an actual held Ready even if saving Observed failed. No new handle.
    /// # Errors
    /// Refuses another epoch/custody or receipt-only adoption of unknown history.
    #[cfg(feature = "cloud-exposure")]
    pub fn uncertain_observed(&mut self, epoch: Uuid, custody: Uuid) -> Result<(), super::Error> {
        if epoch != self.epoch || custody.is_nil() {
            return Err(super::Error::Invalid(INVALID));
        }
        if self.startup == (Startup::Starting {}) {
            self.startup = Startup::Unknown { custody: Some(custody) };
            return Ok(());
        }
        if !matches!(self.startup, Startup::Observed { custody: old } | Startup::Finishing { custody: old, .. } | Startup::FinishUnknown { custody: old, .. } | Startup::Unknown { custody: Some(old) } if old == custody)
        {
            return Err(super::Error::Invalid(INVALID));
        }
        self.uncertain(epoch)
    }
}

#[cfg(all(test, feature = "cloud-exposure"))]
mod tests;
