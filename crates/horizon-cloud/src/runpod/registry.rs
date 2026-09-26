//! Named pull bindings with durable mutation fences. The caller owns secret policy.
use super::{Cancellation, CloudError, Credential, RunPod, valid_id};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Binding {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum State {
    #[default]
    Prepared,
    Requested {
        name: String,
    },
    Bound(Binding),
    Revoking(Binding),
    Revoked,
}

impl State {
    fn validate_name(&self, expected: &str) -> Result<(), CloudError> {
        let name = match self {
            Self::Requested { name } => Some(name.as_str()),
            Self::Bound(binding) | Self::Revoking(binding) => Some(binding.name.as_str()),
            Self::Prepared | Self::Revoked => None,
        };
        if name.is_some_and(|name| name != expected) {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
}

/// One immutable credential generation. Rotation uses a new operation identity;
/// callers must validate and select the replacement before revoking the old one.
pub struct PullBinding<'a> {
    pub operation_id: &'a str,
    pub username: &'a str,
    /// Must be an explicitly authorized read-only registry credential.
    pub credential: &'a Credential,
}

impl RunPod {
    /// # Errors
    /// Lists binding metadata only, never provider-returned passwords.
    pub fn registry_bindings(&self, cancel: &Cancellation) -> Result<Vec<Binding>, CloudError> {
        let value = self.registry_request("GET", "/registries", None, cancel)?;
        let bindings: Vec<Binding> =
            serde_json::from_value(value.get("registries").cloned().ok_or(CloudError::InvalidResponse)?)
                .map_err(|_| CloudError::InvalidResponse)?;
        if bindings.iter().any(|binding| !valid_id(&binding.id)) {
            return Err(CloudError::InvalidResponse);
        }
        Ok(bindings)
    }

    /// Caller holds its generation lock and durably persists every transition.
    /// An uncertain POST is reconciled by its unique name; absence never permits replay.
    /// # Errors
    /// Refuses cancelled, revoked, conflicting or unresolved operations.
    pub fn ensure_registry_binding(
        &self,
        input: &PullBinding<'_>,
        state: &mut State,
        cancel: &Cancellation,
        mut persist: impl FnMut(&State) -> Result<(), CloudError>,
    ) -> Result<Binding, CloudError> {
        let name = operation_name(input.operation_id)?;
        state.validate_name(&name)?;
        validate_username(input.username)?;
        cancel.check()?;
        if matches!(state, State::Revoking(_) | State::Revoked) {
            return Err(CloudError::Invalid("Registry binding is revoked or being revoked"));
        }
        let existing = self.match_registry_binding(&name, state, cancel)?;
        if let Some(binding) = existing {
            if *state == State::Prepared {
                return Err(CloudError::IdentityMismatch);
            }
            if let State::Bound(expected) = state
                && *expected != binding
            {
                return Err(CloudError::IdentityMismatch);
            }
            transition(state, State::Bound(binding.clone()), &mut persist)?;
            return Ok(binding);
        }
        if *state != State::Prepared {
            return Err(CloudError::Invalid(
                "Registry binding is missing or its creation remains unresolved",
            ));
        }
        cancel.check()?;
        transition(state, State::Requested { name: name.clone() }, &mut persist)?;
        // Preserve Requested after every error, including cancellation. A transport
        // failure cannot establish whether the provider accepted the credential.
        let value = self.registry_request(
            "POST",
            "/registries",
            Some(serde_json::json!({
                "name": name, "username": input.username, "password": input.credential.value(),
            })),
            cancel,
        )?;
        let binding: Binding = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
        if binding.name != name || !valid_id(&binding.id) {
            return Err(CloudError::IdentityMismatch);
        }
        transition(state, State::Bound(binding.clone()), &mut persist)?;
        Ok(binding)
    }

    /// Reconciles a previous POST without requiring or retransmitting its secret.
    /// # Errors
    /// An empty list leaves Requested fenced; duplicate names are conflicts.
    pub fn reconcile_registry_binding(
        &self,
        operation_id: &str,
        state: &mut State,
        cancel: &Cancellation,
        mut persist: impl FnMut(&State) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        let name = operation_name(operation_id)?;
        state.validate_name(&name)?;
        cancel.check()?;
        match state {
            State::Prepared | State::Revoked => Ok(()),
            State::Revoking(_) => self.revoke_registry_binding(operation_id, state, cancel, persist),
            State::Requested { .. } | State::Bound(_) => {
                let binding = self
                    .match_registry_binding(&name, state, cancel)?
                    .ok_or(CloudError::Invalid(
                        "Registry binding is missing or its creation remains unresolved",
                    ))?;
                if let State::Bound(expected) = state
                    && *expected != binding
                {
                    return Err(CloudError::IdentityMismatch);
                }
                transition(state, State::Bound(binding), &mut persist)
            }
        }
    }

    /// Revokes only this generation, retaining intent until absence is observed.
    /// This does not revoke the registry token itself or change already running workers.
    /// # Errors
    /// Refuses identity conflicts and unresolved creates; failed deletes remain retryable.
    pub fn revoke_registry_binding(
        &self,
        operation_id: &str,
        state: &mut State,
        cancel: &Cancellation,
        mut persist: impl FnMut(&State) -> Result<(), CloudError>,
    ) -> Result<(), CloudError> {
        let name = operation_name(operation_id)?;
        state.validate_name(&name)?;
        cancel.check()?;
        if *state == State::Revoked {
            return Ok(());
        }
        let observed = self.match_registry_binding(&name, state, cancel)?;
        if *state == State::Prepared && observed.is_some() {
            return Err(CloudError::IdentityMismatch);
        }
        let Some(binding) = observed else {
            if matches!(state, State::Requested { .. }) {
                return Err(CloudError::Invalid(
                    "Registry creation remains unresolved; reconcile before revoking",
                ));
            }
            return transition(state, State::Revoked, &mut persist);
        };
        if let State::Bound(expected) | State::Revoking(expected) = state
            && *expected != binding
        {
            return Err(CloudError::IdentityMismatch);
        }
        transition(state, State::Revoking(binding.clone()), &mut persist)?;
        self.registry_request("DELETE", &format!("/registries/{}", binding.id), None, cancel)?;
        if self.match_registry_binding(&name, state, cancel)?.is_some() {
            return Err(CloudError::Invalid("Registry revocation is pending; reconcile again"));
        }
        transition(state, State::Revoked, &mut persist)
    }

    fn match_registry_binding(
        &self,
        name: &str,
        state: &State,
        cancel: &Cancellation,
    ) -> Result<Option<Binding>, CloudError> {
        let observed = self.registry_bindings(cancel)?;
        if let State::Bound(expected) | State::Revoking(expected) = state
            && observed
                .iter()
                .any(|binding| binding.id == expected.id && binding != expected)
        {
            return Err(CloudError::IdentityMismatch);
        }
        let mut bindings = observed.into_iter().filter(|binding| binding.name == name);
        let first = bindings.next();
        if bindings.next().is_some() {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(first)
    }

    fn registry_request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        cancel: &Cancellation,
    ) -> Result<serde_json::Value, CloudError> {
        // Provider error bodies may echo a submitted registry password. The
        // ordinary adapter redacts only its compute credential, so discard reasons.
        self.request(method, path, body, cancel).map_err(|error| match error {
            CloudError::Rejected(_) => CloudError::Invalid("Registry binding request was rejected"),
            CloudError::Http(_, _) => CloudError::Invalid("Registry binding provider request failed"),
            other => other,
        })
    }
}

fn operation_name(id: &str) -> Result<String, CloudError> {
    if !valid_id(id) || id.len() > 64 {
        return Err(CloudError::Invalid("Invalid registry operation identity"));
    }
    Ok(format!("horizon-pull-{id}"))
}

fn validate_username(username: &str) -> Result<(), CloudError> {
    if username.is_empty()
        || username.len() > 256
        || username.chars().any(char::is_whitespace)
        || username.chars().any(char::is_control)
    {
        return Err(CloudError::Invalid("Invalid registry username"));
    }
    Ok(())
}

fn transition(
    state: &mut State,
    next: State,
    persist: &mut impl FnMut(&State) -> Result<(), CloudError>,
) -> Result<(), CloudError> {
    persist(&next)?;
    *state = next;
    Ok(())
}
