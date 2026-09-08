//! Explicit one-shot materialization, never task admission or an idempotent remote job.

use super::{
    bundle::store::BundleStoreError,
    checkout::{
        PrivateCheckoutFailure,
        publication::{PublicationFailure, PublishedCheckout, validate_sibling_name},
    },
    namespace::NamespaceError,
    seed::SeedError,
};
use crate::cloud_run::ArtifactDigest;
use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

/// Maximum UTF-8 bytes in each explicitly nominated request path.
pub const MAX_REQUEST_PATH_BYTES: usize = 4096;

/// Explicitly authorized complete base closure, verified overlay and fresh destination.
/// Source/ancestry remain stable and scratch is exclusively controlled by the caller.
pub struct MaterializationRequest<'a> {
    pub objects_directory: &'a Path,
    pub bundle_store: &'a Path,
    pub bundle_manifest: &'a ArtifactDigest,
    pub scratch_parent: &'a Path,
    pub destination: &'a str,
}

/// Acknowledged publication plus retained source metadata. Drop never cleans either.
pub struct MaterializedRepository {
    metadata: PathBuf,
    checkout: PublishedCheckout,
}

impl MaterializedRepository {
    #[must_use]
    pub fn source_metadata(&self) -> &Path {
        &self.metadata
    }
    #[must_use]
    pub fn checkout(&self) -> &PublishedCheckout {
        &self.checkout
    }
}

impl fmt::Debug for MaterializedRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedRepository").finish_non_exhaustive()
    }
}

/// All known state remains inspectable. No failure authorizes retry or cleanup.
pub struct MaterializationFailure {
    pub problem: MaterializationProblem,
    metadata: Option<PathBuf>,
}

impl MaterializationFailure {
    #[must_use]
    pub fn source_metadata(&self) -> Option<&Path> {
        self.metadata.as_deref()
    }
}

impl fmt::Debug for MaterializationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.problem, f)
    }
}
impl fmt::Display for MaterializationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.problem, f)
    }
}
impl std::error::Error for MaterializationFailure {}

#[derive(Debug, thiserror::Error)]
pub enum MaterializationProblem {
    #[error("repository materialization requires explicit supported paths and a fresh destination name")]
    InvalidRequest,
    #[error(transparent)]
    Bundle(#[from] BundleStoreError),
    #[error(transparent)]
    Source(#[from] SeedError),
    #[error(transparent)]
    Namespace(#[from] NamespaceError),
    #[error(transparent)]
    Preparation(#[from] PrivateCheckoutFailure),
    #[error(transparent)]
    Publication(#[from] Box<PublicationFailure>),
}

/// Load one expected private bundle and materialize/publish it through one isolated source.
/// Requires the existing source, private-ancestry, trusted-tool and qualified-storage
/// contracts of the component APIs. Default packed decoding ceilings apply. No source
/// config/history transfer, transport, task start, retry or cleanup is performed.
/// Run off the UI thread. Component I/O owns blocking latency. Lost response or abrupt
/// process termination is an unknown observed outcome, not proof of a clean rollback.
/// # Errors
/// Rejects invalid/unsupported requests or component failures, retaining all known
/// residues and exact publication state. Successful receipt Drop also retains data.
pub fn materialize_repository(
    request: &MaterializationRequest<'_>,
    cancelled: impl Fn() -> bool,
) -> Result<MaterializedRepository, MaterializationFailure> {
    let validation = if cancelled() {
        Err(SeedError::Cancelled.into())
    } else if ![request.objects_directory, request.bundle_store, request.scratch_parent]
        .into_iter()
        .all(valid_path)
        || validate_sibling_name(request.destination).is_err()
    {
        Err(MaterializationProblem::InvalidRequest)
    } else {
        Ok(())
    };
    validation.map_err(|problem| MaterializationFailure {
        problem,
        metadata: None,
    })?;
    #[cfg(target_os = "linux")]
    {
        run(request, &cancelled)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(MaterializationFailure {
            problem: SeedError::Unsupported.into(),
            metadata: None,
        })
    }
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|text| text.len() <= MAX_REQUEST_PATH_BYTES && !text.contains('\0'))
        && path
            .components()
            .all(|part| matches!(part, Component::Prefix(_) | Component::RootDir | Component::Normal(_)))
}

#[cfg(target_os = "linux")]
fn run(
    request: &MaterializationRequest<'_>,
    cancelled: &impl Fn() -> bool,
) -> Result<MaterializedRepository, MaterializationFailure> {
    use super::{
        bundle::store::RepositoryBundleStore,
        checkout::{prepare_private_checkout, publication::publish_sibling_checkout},
        namespace::resolve_namespaces_from_source,
        seed::packed::{PackedGitObjectSource, PackedSourceLimits},
    };

    let bundle = RepositoryBundleStore::open(request.bundle_store).and_then(|store| store.get(request.bundle_manifest));
    if cancelled() {
        return Err(MaterializationFailure {
            problem: SeedError::Cancelled.into(),
            metadata: None,
        });
    }
    let bundle = bundle.map_err(|error| MaterializationFailure {
        problem: error.into(),
        metadata: None,
    })?;
    let mut source = PackedGitObjectSource::new(
        request.scratch_parent,
        request.objects_directory,
        PackedSourceLimits::default(),
        cancelled,
    )
    .map_err(|failure| MaterializationFailure {
        metadata: failure.residue().map(std::path::Path::to_owned),
        problem: failure.reason.into(),
    })?;
    let metadata = source.metadata_path().to_owned();
    let result = (|| {
        let resolved = resolve_namespaces_from_source(&mut source, bundle, cancelled)?;
        let checkout = prepare_private_checkout(request.scratch_parent, &resolved, &mut source, cancelled)?;
        publish_sibling_checkout(checkout, request.destination, cancelled)
            .map_err(|failure| MaterializationProblem::Publication(Box::new(failure)))
    })();
    match result {
        Ok(checkout) => Ok(MaterializedRepository { metadata, checkout }),
        Err(problem) => Err(MaterializationFailure {
            problem,
            metadata: Some(metadata),
        }),
    }
}

#[cfg(test)]
mod tests;
