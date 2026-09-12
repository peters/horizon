//! Immutable CPU placement provenance, not provider attestation or creation authority.

use super::{
    CloudStoreError, CloudWorkflowStore, Error, StoredRemoteAllocation, current_unix_millis, ensure_current_schema,
    exact_allocation, open_read_connection, validate_unclaimed,
};
use crate::cloud_run::{
    ArtifactDigest, CloudProvider, WorkerLifetime,
    azure::{AzureDeploymentPlan, AzureDiskSku, AzureError, AzureProfile, valid_subscription_id},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

/// Non-secret version-one profile consistency binding. Not a signature, provider
/// observation, permission to create, or proof that any private key is absent.
/// No deserialization bypasses the validating profile constructor or storage reader.
#[derive(Clone, Eq, PartialEq)]
pub struct RemoteCpuProfileBinding {
    subscription_id: String,
    profile_digest: ArtifactDigest,
}

impl std::fmt::Debug for RemoteCpuProfileBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RemoteCpuProfileBinding { .. }")
    }
}

impl RemoteCpuProfileBinding {
    /// Bind all approved profile fields without I/O or implicit defaults.
    /// # Errors
    /// Rejects an invalid profile before computing its digest.
    pub fn from_profile(profile: &AzureProfile) -> Result<Self, AzureError> {
        profile.validate()?;
        // Exhaustive destructuring deliberately forces a version decision when a
        // profile field is added. Never hash the live struct's serde representation.
        let AzureProfile {
            name,
            subscription_id,
            location,
            vm_size,
            image_pull_identity_id,
            declared_hourly_cost_micros,
            registry_login_server,
            disk_sku,
        } = profile;
        let price = declared_hourly_cost_micros.to_be_bytes();
        let disk: &[u8] = match disk_sku {
            AzureDiskSku::StandardSsdLrs => b"StandardSSD_LRS",
            AzureDiskSku::PremiumLrs => b"Premium_LRS",
        };
        // Frozen v1: domain/version bytes, then eight u64-BE byte lengths and
        // exact UTF-8 fields in the declared order (price is eight BE bytes).
        let mut bytes = b"horizon.remote.cpu-profile.azure.v1\0".to_vec();
        for field in [
            name.as_bytes(),
            subscription_id.as_bytes(),
            location.as_bytes(),
            vm_size.as_bytes(),
            image_pull_identity_id.as_bytes(),
            price.as_slice(),
            registry_login_server.as_bytes(),
            disk,
        ] {
            bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
            bytes.extend_from_slice(field);
        }
        Ok(Self {
            subscription_id: subscription_id.clone(),
            profile_digest: ArtifactDigest::sha256(&bytes),
        })
    }

    #[must_use]
    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }

    #[must_use]
    pub fn profile_digest(&self) -> &ArtifactDigest {
        &self.profile_digest
    }

    /// Compare explicit subscription and all profile bytes, without I/O.
    /// # Errors
    /// Rejects an invalid supplied profile rather than treating it as a match.
    pub fn matches_profile(&self, profile: &AzureProfile) -> Result<bool, AzureError> {
        Self::from_profile(profile).map(|candidate| self == &candidate)
    }
}

impl CloudWorkflowStore {
    /// Record one immutable CPU profile binding for this exact allocation.
    /// First recording must precede client-key preparation/reservation and provider
    /// dispatch. The caller must enforce private-key-file ordering; this database
    /// cannot prove absence of a file. No credentials, provider I/O, snapshot writes,
    /// revision bumps or creation-grant consumption/renewal occur here.
    /// An exact existing binding is idempotent even after setup expires or closes.
    /// Run synchronously off the render thread.
    /// # Errors
    /// Refuses stale/foreign allocations, unsupported complete deployment targets,
    /// late first recording, replacement, corrupt rows and storage errors.
    pub fn record_remote_cpu_profile_binding(
        &self,
        expected: &StoredRemoteAllocation,
        profile: &AzureProfile,
    ) -> Result<(), Error> {
        let proposed = RemoteCpuProfileBinding::from_profile(profile).map_err(|_| Error::RuntimeSetupUnavailable)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        let state = current.workspace().state();
        AzureDeploymentPlan::validate_target(profile, &state.spec.target)
            .map_err(|_| Error::RuntimeSetupUnavailable)?;
        if let Some(existing) = load_binding(&transaction, &current)? {
            return if existing == proposed {
                Ok(())
            } else {
                Err(Error::ReplacementIdentityMismatch)
            };
        }
        validate_unclaimed(&transaction, &current)?;
        let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        let first_pin: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_first_pin_intents WHERE workspace_local_id = ?1)",
            [&state.spec.workspace_local_id],
            |row| row.get(0),
        )?;
        if runtime.ssh_public_key.is_some()
            || first_pin
            || current.workflow().workflow().retain_until_millis <= current_unix_millis()?
        {
            return Err(Error::RuntimeSetupUnavailable);
        }
        transaction.execute(
            "INSERT INTO remote_provider_bindings
             (workspace_local_id, session_id, generation, workflow_id, job_id, version,
              provider, subscription_id, profile_digest)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, 'azure', ?6, ?7)",
            params![
                state.spec.workspace_local_id,
                current.workspace().session_id(),
                i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?,
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string(),
                proposed.subscription_id,
                proposed.profile_digest.as_str()
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Read immutable provenance from one exact consistent allocation snapshot.
    /// No creation, schema migration/backfill, expiry renewal or provider I/O.
    /// Valid schema-four/five/six stores and genuinely absent metadata return `None`.
    /// Bound records remain inspectable after setup expiry or management intent.
    /// Missing provenance never authorizes adoption from current configuration.
    /// # Errors
    /// Rejects stale/foreign allocations, malformed/colliding rows and storage errors.
    pub fn load_remote_cpu_profile_binding(
        &self,
        expected: &StoredRemoteAllocation,
    ) -> Result<Option<RemoteCpuProfileBinding>, Error> {
        let mut connection = open_read_connection(self.path())?;
        let transaction = connection.transaction()?;
        ensure_current_schema(&transaction)?;
        let current = exact_allocation(&transaction, expected)?;
        let version = transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        if matches!(version, 4..=6) {
            return Ok(None);
        }
        load_binding(&transaction, &current)
    }
}

fn load_binding(
    connection: &Connection,
    allocation: &StoredRemoteAllocation,
) -> Result<Option<RemoteCpuProfileBinding>, Error> {
    let state = allocation.workspace().state();
    let runtime = state.runtime.as_ref().ok_or(Error::UnboundRuntime)?;
    let row = connection
        .query_row(
            "SELECT workspace_local_id = ?1 AND session_id = ?2 AND generation = ?3
                AND workflow_id = ?4 AND job_id = ?5 AND version = 1 AND provider = 'azure'
                AND (SELECT COUNT(*) FROM remote_provider_bindings
                     WHERE workspace_local_id = ?1 OR workflow_id = ?4 OR job_id = ?5) = 1,
                CAST(substr(CAST(subscription_id AS BLOB), 1, 37) AS TEXT),
                CAST(substr(CAST(profile_digest AS BLOB), 1, 65) AS TEXT)
         FROM remote_provider_bindings WHERE workspace_local_id = ?1 OR workflow_id = ?4 OR job_id = ?5",
            params![
                state.spec.workspace_local_id,
                allocation.workspace().session_id(),
                i64::try_from(runtime.generation).map_err(|_| Error::GenerationExhausted)?,
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string()
            ],
            |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((matches, subscription_id, digest)) = row else {
        return Ok(None);
    };
    if !matches
        || !valid_subscription_id(&subscription_id)
        || state.spec.target.provider != CloudProvider::Azure
        || state.spec.target.lifetime != WorkerLifetime::Persistent
    {
        return Err(CloudStoreError::InvalidRemoteAllocation.into());
    }
    let profile_digest = ArtifactDigest::parse_sha256(&digest).map_err(|_| CloudStoreError::InvalidRemoteAllocation)?;
    if profile_digest.as_str() != digest {
        return Err(CloudStoreError::InvalidRemoteAllocation.into());
    }
    Ok(Some(RemoteCpuProfileBinding {
        subscription_id,
        profile_digest,
    }))
}

#[cfg(test)]
mod tests;
