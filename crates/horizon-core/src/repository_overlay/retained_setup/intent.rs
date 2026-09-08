use super::SetupClaimError;
use crate::{
    cloud_run::ArtifactDigest,
    remote_workspace::valid_local_id,
    repository_overlay::{checkout::publication::validate_sibling_name, materialize::valid_path},
};
use std::{fmt, path::PathBuf};

/// Complete immutable setup intent. The retained root determines the scratch slot;
/// a client-supplied retry ID or runtime generation never selects a new claim slot.
#[derive(Clone, Eq, PartialEq)]
pub struct SetupIntent {
    pub(super) workspace_local_id: String,
    pub(super) objects_directory: PathBuf,
    pub(super) bundle_store: PathBuf,
    pub(super) bundle_manifest: ArtifactDigest,
    pub(super) destination: String,
}

impl SetupIntent {
    /// Validate without filesystem access or authorization to transfer any source.
    /// Paths use the existing materialization request policy, not alias inference.
    /// # Errors
    /// Rejects invalid workspace identity, unsupported paths or destination names.
    pub fn new(
        workspace_local_id: String,
        objects_directory: PathBuf,
        bundle_store: PathBuf,
        bundle_manifest: ArtifactDigest,
        destination: String,
    ) -> Result<Self, SetupClaimError> {
        if !valid_local_id(&workspace_local_id)
            || !valid_path(&objects_directory)
            || !valid_path(&bundle_store)
            || validate_sibling_name(&destination).is_err()
        {
            return Err(SetupClaimError::InvalidIntent);
        }
        Ok(Self {
            workspace_local_id,
            objects_directory,
            bundle_store,
            bundle_manifest,
            destination,
        })
    }
}

impl fmt::Debug for SetupIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupIntent").finish_non_exhaustive()
    }
}
