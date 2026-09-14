//! Remote browser session configuration: provider profiles, selectable
//! targets and local session limits, with credential references instead of
//! credential values.
//!
//! Parsing and validation never touch the network, a credential store or a
//! device. Definitions are portable; credential bindings are machine-local and
//! stripped from exports.

mod error;
mod provider;
mod target;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use error::{CredentialReferenceProblem, EndpointProblem, ExtensionProblem, RemoteConfigError};
pub use provider::{
    ControlEndpoint, CredentialBinding, CredentialReference, CredentialStoreKind, RemoteAdapterKind,
    RemoteAuthentication, RemoteProviderProfile, RemoteSessionLimits,
};
pub use target::{DeviceKind, DeviceRequirement, RemoteTargetProfile};

/// `browser.remote` in Horizon's configuration. Empty by default: nothing
/// remote is selected, discovered or allocated unless configured.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteBrowserConfig {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, RemoteProviderProfile>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, RemoteTargetProfile>,
}

/// Result of merging a portable definition into the local configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportSummary {
    pub providers_added: Vec<String>,
    pub providers_updated: Vec<String>,
    pub targets_added: Vec<String>,
    pub targets_updated: Vec<String>,
}

impl RemoteBrowserConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.targets.is_empty()
    }

    /// Full validation for a machine-local configuration: definitions plus
    /// credential bindings for every authentication reference. Run this before
    /// allocation; configuration loading uses [`Self::validate_definition`] so a
    /// missing binding is an actionable readiness state, not a startup failure.
    ///
    /// # Errors
    /// Returns the first [`RemoteConfigError`]; messages name identifiers only.
    pub fn validate(&self) -> Result<(), RemoteConfigError> {
        self.validate_definition()?;
        for (name, provider) in &self.providers {
            provider.validate_bindings(name)?;
        }
        Ok(())
    }

    /// Validation of the portable part only (no binding requirements).
    ///
    /// # Errors
    /// Returns the first [`RemoteConfigError`].
    pub fn validate_definition(&self) -> Result<(), RemoteConfigError> {
        for (name, provider) in &self.providers {
            if !provider::valid_identifier(name, 64) {
                return Err(RemoteConfigError::InvalidProviderName { provider: name.clone() });
            }
            provider.validate_definition(name)?;
        }
        for (name, target) in &self.targets {
            if !provider::valid_identifier(name, 64) {
                return Err(RemoteConfigError::InvalidTargetName { target: name.clone() });
            }
            if !self.providers.contains_key(&target.provider) {
                return Err(RemoteConfigError::UnknownProvider {
                    target: name.clone(),
                    provider: target.provider.clone(),
                });
            }
            target.validate(name)?;
        }
        Ok(())
    }

    /// The shareable definition: every provider and target with its
    /// authentication references, and no machine-local bindings. Presence of
    /// values on this machine is a live query ([`Self::binding_presence`]),
    /// not part of the portable file.
    #[must_use]
    pub fn export_portable(&self) -> Self {
        let providers = self
            .providers
            .iter()
            .map(|(name, provider)| {
                let mut portable = provider.clone();
                portable.credential_bindings.clear();
                (name.clone(), portable)
            })
            .collect();
        Self {
            providers,
            targets: self.targets.clone(),
        }
    }

    /// Presence of a binding for each authentication reference, per provider,
    /// for readiness displays and export markers. Never returns values.
    #[must_use]
    pub fn binding_presence(&self) -> BTreeMap<String, BTreeMap<CredentialReference, bool>> {
        self.providers
            .iter()
            .map(|(name, provider)| {
                let presence = provider
                    .authentication
                    .references()
                    .into_iter()
                    .map(|reference| (reference.clone(), provider.credential_bindings.contains_key(reference)))
                    .collect();
                (name.clone(), presence)
            })
            .collect()
    }

    /// Merge a portable definition. New providers and targets are added, and an
    /// existing provider is updated only when its endpoint is unchanged and,
    /// while local bindings exist, its authentication references are unchanged,
    /// so a shared file can never redirect a trusted endpoint, the credentials
    /// bound to it, or leave bindings attached to a different authentication
    /// shape. Local bindings are kept; imported bindings are rejected.
    ///
    /// # Errors
    /// Rejects the whole import, leaving `self` untouched, on any validation
    /// failure, imported binding or endpoint conflict.
    pub fn import_portable(&mut self, incoming: &Self) -> Result<ImportSummary, RemoteConfigError> {
        incoming.validate_definition()?;
        let mut merged = self.clone();
        let mut summary = ImportSummary::default();
        for (name, provider) in &incoming.providers {
            if !provider.credential_bindings.is_empty() {
                return Err(RemoteConfigError::ImportCarriesBindings { provider: name.clone() });
            }
            if let Some(existing) = merged.providers.get_mut(name) {
                if existing.endpoint != provider.endpoint {
                    return Err(RemoteConfigError::ImportEndpointConflict { provider: name.clone() });
                }
                if !existing.credential_bindings.is_empty()
                    && existing.authentication.references() != provider.authentication.references()
                {
                    return Err(RemoteConfigError::ImportAuthenticationConflict { provider: name.clone() });
                }
                let bindings = std::mem::take(&mut existing.credential_bindings);
                *existing = provider.clone();
                existing.credential_bindings = bindings;
                summary.providers_updated.push(name.clone());
            } else {
                merged.providers.insert(name.clone(), provider.clone());
                summary.providers_added.push(name.clone());
            }
        }
        for (name, target) in &incoming.targets {
            if merged.targets.insert(name.clone(), target.clone()).is_some() {
                summary.targets_updated.push(name.clone());
            } else {
                summary.targets_added.push(name.clone());
            }
        }
        merged.validate_definition()?;
        *self = merged;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests;
