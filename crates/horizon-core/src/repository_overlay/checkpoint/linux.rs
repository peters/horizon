use super::{
    CheckpointError as Error, CheckpointFailure, CheckpointGeneration, CheckpointRequest, GenerationManifest,
    PackIdentity, storage::Generation,
};
use crate::{
    cloud_run::{ArtifactDigest, GitCommitSha},
    repository_overlay::{
        OverlayPlanError,
        bundle::{OverlayBundleError, RepositoryOverlayBundle, codec, store::RepositoryBundleStore},
        capture::{GitCaptureError, capture_selected_revision},
        namespace::{NamespaceError, ResolvedRepositoryOverlay, resolve_namespaces_from_source},
        reader::RepositoryReadError,
        seed::{
            GitObjectInspector, GitObjectMetadata, GitObjectSource, GitObjectStream, SeedError,
            export::{PackExportLimits, prepare_git_base_pack},
            packed::{PackedGitObjectSource, PackedSourceLimits},
            prepare_git_seed,
            receive::{
                PackReceiveLimits, observe_git_base_pack,
                publication::{
                    NamedPackPublication, NamedPackPublicationFailure, PackPublicationError, publish_named_git_pack,
                },
                receive_named_git_base_pack,
            },
        },
    },
};
use git2::{ObjectType, Oid};
use std::{
    fs::File,
    io::{Cursor, Read},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CONTENT_BYTES: u64 = 16 * 1024 * 1024;
const PACK_BYTES: u64 = 32 * 1024 * 1024;
const SOURCE_BYTES: u64 = 32 * 1024 * 1024;
const OBJECTS: usize = 2048;
const LFS_PREFIX: &[u8] = b"version https://git-lfs.github.com/spec/v1";

pub(super) fn run(
    request: &CheckpointRequest,
    checkout: &Path,
    identity: &impl Fn() -> Result<(), Error>,
    cancelled: &impl Fn() -> bool,
) -> Result<CheckpointGeneration, CheckpointFailure> {
    let preflight = || {
        request.validate()?;
        if cancelled() {
            return Err(Error::Cancelled);
        }
        identity()?;
        Generation::admit(request, checkout)
    };
    let parent = preflight().map_err(|reason| CheckpointFailure { reason, retained: None })?;
    let path = request.parent.join(&request.attempt_name);
    let execute = || {
        let generation = Generation::claim(request, parent)?;
        let deadline = Instant::now();
        let stop = || cancelled() || deadline.elapsed() > Duration::from_secs(120);
        let manifest = capture(request, checkout, identity, &generation, &stop)?;
        let bytes = serde_json::to_vec(&manifest).map_err(|_| Error::Storage)?;
        let digest = ArtifactDigest::sha256(&bytes);
        generation.finish(&bytes)?;
        identity()?;
        if stop() {
            return Err(Error::Cancelled);
        }
        Ok(CheckpointGeneration {
            path: generation.path,
            manifest_sha256: digest,
            manifest,
        })
    };
    execute().map_err(|reason| CheckpointFailure {
        reason,
        retained: Some(path),
    })
}

fn sample(request: &CheckpointRequest, checkout: &Path) -> Result<RepositoryOverlayBundle, Error> {
    let selected: Vec<_> = request.selected.iter().map(String::as_str).collect();
    let bundle = capture_selected_revision(
        checkout,
        request.preparation.source.clone(),
        &request.preparation.work_branch,
        &selected,
    )
    .map_err(capture_error)?;
    if bundle.file_bytes() as u64 > CONTENT_BYTES {
        return Err(Error::Capacity);
    }
    if bundle.blobs().any(|blob| blob.bytes().starts_with(LFS_PREFIX)) {
        return Err(Error::Unsupported);
    }
    Ok(bundle)
}

fn capture(
    request: &CheckpointRequest,
    checkout: &Path,
    identity: &impl Fn() -> Result<(), Error>,
    generation: &Generation,
    cancelled: &impl Fn() -> bool,
) -> Result<GenerationManifest, Error> {
    let started_at_millis = now()?;
    let bundle = sample(request, checkout)?;
    let encoded = codec::encode(&bundle).map_err(|_| Error::Capacity)?;
    if encoded.len() as u64 > CONTENT_BYTES {
        return Err(Error::Capacity);
    }
    let overlay_record = ArtifactDigest::sha256(&encoded);
    let overlay_bytes = encoded.len() as u64;
    drop(encoded);
    let limits = PackedSourceLimits {
        address_space_bytes: 512 * 1024 * 1024,
        cpu_seconds: 15,
        object_timeout: Duration::from_secs(15),
    };
    let scratch = generation.path.join("scratch");
    let mut source = Source {
        inner: PackedGitObjectSource::new(&scratch, &checkout.join(".git/objects"), limits, cancelled)
            .map_err(|e| seed_error(e.reason))?,
        bytes: 0,
        objects: 0,
    };
    let resolved = resolve_namespaces_from_source(&mut source, bundle, cancelled).map_err(namespace_error)?;
    for namespace in [resolved.base(), resolved.index(), resolved.working_tree()] {
        if namespace.entries().len() > 1024 || namespace.logical_bytes() > CONTENT_BYTES {
            return Err(Error::Capacity);
        }
    }
    let pack_identity = retain_pack(generation, &resolved, source, limits, cancelled)?;
    let store = RepositoryBundleStore::open_named(&generation.path.join("bundles")).map_err(|_| Error::Storage)?;
    let overlay_manifest = store.put(resolved.bundle()).map_err(|_| Error::Storage)?;
    if store.get(&overlay_manifest).map_err(|_| Error::Storage)? != *resolved.bundle() {
        return Err(Error::Storage);
    }
    generation.check()?;
    identity()?;
    if sample(request, checkout)? != *resolved.bundle() {
        return Err(Error::Changed);
    }
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let verified_at_millis = now()?;
    if verified_at_millis < started_at_millis {
        return Err(Error::Identity);
    }
    let mut selected = request.selected.clone();
    selected.sort_unstable();
    Ok(GenerationManifest {
        version: 1,
        coverage: "complete current base closure plus selected dirty layers; not atomic or full workspace recovery"
            .into(),
        preparation: request.preparation.clone(),
        complete_base_closure_consent: request.complete_base_closure_consent,
        retained_volume_attested: request.retained_volume_attested,
        selected,
        pack: pack_identity,
        overlay_manifest,
        overlay_record,
        overlay_bytes,
        started_at_millis,
        verified_at_millis,
    })
}

fn retain_pack(
    generation: &Generation,
    resolved: &ResolvedRepositoryOverlay,
    mut source: Source<'_>,
    limits: PackedSourceLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<PackIdentity, Error> {
    let scratch = generation.path.join("scratch");
    generation.check()?;
    let seed = prepare_git_seed(&scratch, resolved, &mut source, cancelled).map_err(|e| seed_error(e.reason))?;
    drop(source);
    generation.check()?;
    let pack = prepare_git_base_pack(
        &scratch,
        &seed,
        PackExportLimits {
            source: limits,
            encoded_bytes: PACK_BYTES,
        },
        cancelled,
    )
    .map_err(|e| seed_error(e.reason))?;
    generation.check()?;
    let identity = PackIdentity {
        base_commit: GitCommitSha::parse(pack.base_commit().to_string()).map_err(|_| Error::Identity)?,
        sha256: pack.sha256().clone(),
        encoded_bytes: pack.encoded_bytes(),
    };
    let receive_limits = PackReceiveLimits {
        source: limits,
        encoded_bytes: PACK_BYTES,
    };
    let packs = generation.path.join("packs");
    let received = receive_named_git_base_pack(
        &packs,
        "incoming",
        (&pack).into(),
        &mut File::open(pack.path()).map_err(|_| Error::Storage)?,
        receive_limits,
        cancelled,
    )
    .map_err(|e| seed_error(e.reason))?;
    generation.check()?;
    let published =
        publish_named_git_pack(&packs, received, receive_limits, cancelled).map_err(|error| match error {
            NamedPackPublicationFailure::Retained {
                reason: PackPublicationError::Verification(reason),
                ..
            }
            | NamedPackPublicationFailure::PublishedUnsynchronized {
                reason: PackPublicationError::Verification(reason),
                ..
            } => seed_error(reason),
            _ => Error::Storage,
        })?;
    let received = match &published {
        NamedPackPublication::Published(pack) => pack.pack(),
        NamedPackPublication::Existing { pack, .. } => pack,
    };
    observe_git_base_pack(received.path(), received.into(), receive_limits, cancelled).map_err(seed_error)?;
    generation.check()?;
    Ok(identity)
}

fn now() -> Result<u64, Error> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Identity)?
            .as_millis(),
    )
    .map_err(|_| Error::Identity)
}

pub(super) fn capture_error(error: GitCaptureError) -> Error {
    match error {
        GitCaptureError::Changed | GitCaptureError::Read(RepositoryReadError::Changed) => Error::Changed,
        GitCaptureError::Read(RepositoryReadError::TooLarge)
        | GitCaptureError::Policy(
            OverlayPlanError::ChangeLimit | OverlayPlanError::MetadataLimit | OverlayPlanError::ContentLimit,
        )
        | GitCaptureError::Bundle(OverlayBundleError::FileLimit | OverlayBundleError::BundleLimit) => Error::Capacity,
        _ => Error::Unsupported,
    }
}

pub(super) fn namespace_error(error: NamespaceError) -> Error {
    match error {
        NamespaceError::Limit
        | NamespaceError::Policy(
            OverlayPlanError::ChangeLimit | OverlayPlanError::MetadataLimit | OverlayPlanError::ContentLimit,
        ) => Error::Capacity,
        NamespaceError::Source(reason) => seed_error(reason),
        _ => Error::Unsupported,
    }
}

fn seed_error(error: SeedError) -> Error {
    match error {
        SeedError::Limit => Error::Capacity,
        SeedError::Cancelled => Error::Cancelled,
        SeedError::Storage | SeedError::Source => Error::Storage,
        SeedError::UnsafeParent | SeedError::Object => Error::Identity,
        SeedError::Unsupported => Error::Unsupported,
    }
}

struct Source<'a> {
    inner: PackedGitObjectSource<'a>,
    bytes: u64,
    objects: usize,
}
impl GitObjectInspector for Source<'_> {
    fn inspect(&mut self, object: Oid) -> Result<GitObjectMetadata, SeedError> {
        let metadata = self.inner.inspect(object)?;
        if metadata.bytes > CONTENT_BYTES {
            return Err(SeedError::Limit);
        }
        Ok(metadata)
    }
}
impl GitObjectSource for Source<'_> {
    fn open(&mut self, object: Oid) -> Result<GitObjectStream<'_>, SeedError> {
        let stream = self.inner.open(object)?;
        self.bytes = self.bytes.checked_add(stream.bytes).ok_or(SeedError::Limit)?;
        self.objects += 1;
        if stream.bytes > CONTENT_BYTES || self.bytes > SOURCE_BYTES || self.objects > OBJECTS {
            return Err(SeedError::Limit);
        }
        let kind = stream.kind;
        let length = stream.bytes;
        let mut bytes = Vec::new();
        stream
            .reader
            .take(length + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::ConnectionAborted => SeedError::Cancelled,
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::OutOfMemory => SeedError::Limit,
                _ => SeedError::Source,
            })?;
        if bytes.len() as u64 != length {
            return Err(SeedError::Object);
        }
        if kind == ObjectType::Blob && bytes.starts_with(LFS_PREFIX) {
            return Err(SeedError::Unsupported);
        }
        Ok(GitObjectStream {
            kind,
            bytes: length,
            reader: Box::new(Cursor::new(bytes)),
        })
    }
}
