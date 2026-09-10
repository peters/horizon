use super::{
    ArtifactDigest, BundleState, IntakeError, IntakeRequest, IntakeResponse, IntakeRoots, IntakeState, PackProgress,
    PackState, REQUEST_LIMIT, codec,
};
use crate::repository_overlay::{
    bundle::store::{BundleStoreError, RepositoryBundleStore},
    reader::{RepositoryReadError, SelectedRepositoryReader, linux::Root},
    seed::{
        SeedError,
        receive::{
            ExpectedGitPack, PackReceiveLimits, ReceivedGitPack, observe_git_base_pack,
            publication::{PackPublicationError, PackPublicationFailure, publish_sibling_git_pack},
            receive_git_base_pack,
        },
    },
    storage,
};
use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags, mkdirat, openat2, statat};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const INPUTS: &str = "repository-inputs";
const SETUP: &str = "repository-setup";
const CLAIM: &str = "repository-intake.claim";
const DIRECTORIES: [&str; 4] = [INPUTS, "repository-inputs/packs", "repository-inputs/bundles", SETUP];
const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(super) fn execute(
    parent: &Path,
    request: &IntakeRequest,
    input: Option<&mut dyn Read>,
    cancelled: &dyn Fn() -> bool,
) -> IntakeResponse {
    execute_with(parent, request, input, cancelled, &mut File::sync_all, &qualify)
}

pub(super) fn execute_with(
    parent: &Path,
    request: &IntakeRequest,
    input: Option<&mut dyn Read>,
    cancelled: &dyn Fn() -> bool,
    sync: &mut impl FnMut(&File) -> io::Result<()>,
    storage: &impl Fn(&File) -> Result<(), IntakeError>,
) -> IntakeResponse {
    let Ok(encoded) = request.encode() else {
        return IntakeResponse::failure(IntakeError::Invalid);
    };
    let roots = IntakeRoots {
        packs: parent.join(INPUTS).join("packs"),
        bundles: parent.join(INPUTS).join("bundles"),
        setup: parent.join(SETUP),
    };
    let mut response = IntakeResponse::failure(IntakeError::Storage);
    response.intent_sha256 = Some(ArtifactDigest::sha256(&encoded));
    response.roots = Some(roots.clone());
    let mut attempted = false;
    let result = (|| {
        check(cancelled)?;
        let mut boundary = Boundary::open(parent, storage)?;
        let fresh = boundary.admit(&encoded, input.is_some(), &mut attempted, sync, cancelled)?;
        response.state = if fresh {
            IntakeState::Unconfirmed
        } else {
            IntakeState::ClaimedUnknown
        };
        boundary.children(&encoded, fresh, sync, cancelled)?;
        let expected = ExpectedGitPack {
            base_commit: request
                .source
                .commit
                .as_str()
                .parse()
                .map_err(|_| IntakeError::Invalid)?,
            sha256: &request.pack.sha256,
            encoded_bytes: request.pack.encoded_bytes,
        };
        if fresh {
            receive_inputs(
                request,
                expected,
                &roots,
                input.ok_or(IntakeError::Storage)?,
                &mut response,
                cancelled,
            )?;
        } else {
            let pack = observe_git_base_pack(
                &roots.packs.join("base"),
                expected,
                PackReceiveLimits::default(),
                cancelled,
            )
            .map_err(seed_error)?;
            response.pack = Some(progress(PackState::Observed, &pack, false));
            let bundle = RepositoryBundleStore::open(&roots.bundles)
                .and_then(|store| store.get(&request.overlay.sha256))
                .map_err(bundle_error)?;
            if bundle.plan().source() != &request.source {
                return Err(IntakeError::Input);
            }
            response.bundle = Some(BundleState::Observed);
        }
        boundary.verify(&encoded)?;
        check(cancelled)?;
        response.state = if fresh {
            IntakeState::Acknowledged
        } else {
            IntakeState::Observed
        };
        Ok(())
    })();
    if let Err(reason) = result {
        if attempted && response.state == IntakeState::Error {
            response.state = IntakeState::Unconfirmed;
        }
        if !attempted && reason == IntakeError::Unsupported {
            response.state = IntakeState::Unsupported;
        }
    }
    response.reason = result.err();
    response
}

fn receive_inputs(
    request: &IntakeRequest,
    expected: ExpectedGitPack<'_>,
    roots: &IntakeRoots,
    mut input: &mut dyn Read,
    response: &mut IntakeResponse,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), IntakeError> {
    let pack = match receive_git_base_pack(
        &roots.packs,
        expected,
        &mut (&mut input).take(expected.encoded_bytes),
        PackReceiveLimits::default(),
        cancelled,
    ) {
        Ok(pack) => pack,
        Err(failure) => {
            response.pack = Some(PackProgress {
                state: PackState::ReceiveUnconfirmed,
                source: failure.residue().map(Path::to_path_buf),
                destination: None,
                objects: None,
            });
            return Err(seed_error(failure.reason));
        }
    };
    response.pack = Some(progress(PackState::Unpublished, &pack, true));
    let length = usize::try_from(request.overlay.encoded_bytes).map_err(|_| IntakeError::Invalid)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| IntakeError::Input)?;
    let mut buffer = [0_u8; 16 * 1024];
    while bytes.len() < length {
        check(cancelled)?;
        let remaining = (length - bytes.len()).min(buffer.len());
        let count = input.read(&mut buffer[..remaining]).map_err(|_| IntakeError::Input)?;
        if count == 0 {
            return Err(IntakeError::Input);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    check(cancelled)?;
    if input.read(&mut buffer[..1]).map_err(|_| IntakeError::Input)? != 0 {
        return Err(IntakeError::Input);
    }
    let bundle = codec::decode(&bytes).map_err(|_| IntakeError::Input)?;
    drop(bytes);
    if bundle.plan().source() != &request.source || bundle.manifest_sha256() != &request.overlay.sha256 {
        return Err(IntakeError::Input);
    }
    check(cancelled)?;
    match publish_sibling_git_pack(pack, "base", PackReceiveLimits::default(), cancelled) {
        Ok(published) => response.pack = Some(progress(PackState::Acknowledged, published.pack(), false)),
        Err(failure) => {
            let reason = match &failure {
                PackPublicationFailure::Unpublished { reason, .. }
                | PackPublicationFailure::PublishedUnsynchronized { reason, .. } => publication_error(*reason),
                PackPublicationFailure::RenameUnconfirmed { .. } => IntakeError::Storage,
            };
            response.pack = Some(match failure {
                PackPublicationFailure::Unpublished { pack, .. } => progress(PackState::Unpublished, &pack, true),
                PackPublicationFailure::PublishedUnsynchronized { pack, .. } => {
                    progress(PackState::PublishedUnsynchronized, pack.pack(), false)
                }
                PackPublicationFailure::RenameUnconfirmed { pack, destination } => PackProgress {
                    destination: Some(destination),
                    ..progress(PackState::RenameUnconfirmed, &pack, true)
                },
            });
            return Err(reason);
        }
    }
    check(cancelled)?;
    response.bundle = Some(BundleState::WriteUnconfirmed);
    RepositoryBundleStore::open(&roots.bundles)
        .and_then(|store| store.put(&bundle))
        .map_err(bundle_error)?;
    response.bundle = Some(BundleState::Acknowledged);
    Ok(())
}

fn progress(state: PackState, pack: &ReceivedGitPack, source: bool) -> PackProgress {
    PackProgress {
        state,
        source: source.then(|| pack.path().to_owned()),
        destination: (!source).then(|| pack.path().to_owned()),
        objects: Some(pack.objects()),
    }
}

struct Boundary {
    reader: Root,
    handle: File,
    path: PathBuf,
    claim: Option<File>,
    children: Vec<File>,
}

impl Boundary {
    fn open(path: &Path, storage: &impl Fn(&File) -> Result<(), IntakeError>) -> Result<Self, IntakeError> {
        let reader = SelectedRepositoryReader::open(path).map_err(read_error)?.root;
        let handle = open(reader.handle(), ".", OFlags::RDONLY | OFlags::DIRECTORY)?;
        private(&handle)?;
        storage(&handle)?;
        Ok(Self {
            reader,
            handle,
            path: path.to_owned(),
            claim: None,
            children: Vec::new(),
        })
    }

    fn read_claim(&self, encoded: &[u8]) -> Result<Option<File>, IntakeError> {
        match self.reader.read_private_file(CLAIM, REQUEST_LIMIT) {
            Ok(record) => {
                let decoded = IntakeRequest::decode(&record.bytes).map_err(|_| IntakeError::Storage)?;
                if decoded.encode()? != record.bytes {
                    return Err(IntakeError::Storage);
                }
                if record.bytes != encoded {
                    return Err(IntakeError::Conflict);
                }
                Ok(Some(record.file))
            }
            Err(RepositoryReadError::Missing) => Ok(None),
            Err(error) => Err(read_error(error)),
        }
    }

    fn verify(&self, encoded: &[u8]) -> Result<(), IntakeError> {
        private(&self.handle)?;
        let named = SelectedRepositoryReader::open(&self.path).map_err(read_error)?;
        same(&self.handle, named.root.handle())?;
        if let Some(claim) = &self.claim {
            same(claim, &self.read_claim(encoded)?.ok_or(IntakeError::Storage)?)?;
        }
        for (name, held) in DIRECTORIES.iter().zip(&self.children) {
            let named = open(&self.handle, name, OFlags::RDONLY | OFlags::DIRECTORY)?;
            private(&named)?;
            same(held, &named)?;
        }
        Ok(())
    }

    fn admit(
        &mut self,
        encoded: &[u8],
        write: bool,
        attempted: &mut bool,
        sync: &mut impl FnMut(&File) -> io::Result<()>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool, IntakeError> {
        self.verify(encoded)?;
        self.claim = self.read_claim(encoded)?;
        if self.claim.is_some() {
            return Ok(false);
        }
        if !write {
            return Err(IntakeError::Storage);
        }
        for name in [INPUTS, SETUP] {
            if !matches!(
                statat(&self.handle, name, AtFlags::SYMLINK_NOFOLLOW),
                Err(rustix::io::Errno::NOENT)
            ) {
                return Err(IntakeError::Storage);
            }
        }
        check(cancelled)?;
        *attempted = true;
        let mut claim = match openat2(
            &self.handle,
            CLAIM,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            CONFINED,
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::EXIST) => {
                self.claim = self.read_claim(encoded)?;
                return self.claim.as_ref().map(|_| false).ok_or(IntakeError::Storage);
            }
            Err(error) => return Err(storage_error(error)),
        };
        claim.write_all(encoded).map_err(|_| IntakeError::Storage)?;
        self.claim = Some(claim);
        sync(self.claim.as_ref().ok_or(IntakeError::Storage)?)
            .and_then(|()| sync(&self.handle))
            .map_err(|_| IntakeError::Storage)?;
        self.verify(encoded)?;
        Ok(true)
    }

    fn children(
        &mut self,
        encoded: &[u8],
        create: bool,
        sync: &mut impl FnMut(&File) -> io::Result<()>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), IntakeError> {
        for (index, name) in DIRECTORIES.iter().enumerate() {
            check(cancelled)?;
            self.verify(encoded)?;
            let (parent, leaf) = match index {
                1 => (&self.children[0], "packs"),
                2 => (&self.children[0], "bundles"),
                _ => (&self.handle, *name),
            };
            if create {
                mkdirat(parent, leaf, Mode::RUSR | Mode::WUSR | Mode::XUSR).map_err(storage_error)?;
            }
            let child = open(parent, leaf, OFlags::RDONLY | OFlags::DIRECTORY)?;
            private(&child)?;
            self.children.push(child);
        }
        if create {
            for file in self.children.iter().rev().chain(std::iter::once(&self.handle)) {
                check(cancelled)?;
                sync(file).map_err(|_| IntakeError::Storage)?;
            }
        }
        self.verify(encoded)
    }
}

fn open(parent: &File, name: &str, flags: OFlags) -> Result<File, IntakeError> {
    openat2(parent, name, flags | OFlags::CLOEXEC, Mode::empty(), CONFINED)
        .map(File::from)
        .map_err(storage_error)
}
fn private(file: &File) -> Result<(), IntakeError> {
    let metadata = file.metadata().map_err(|_| IntakeError::Storage)?;
    (metadata.is_dir()
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o7777 == 0o700
        && metadata.nlink() != 0)
        .then_some(())
        .ok_or(IntakeError::Storage)
}
fn same(left: &File, right: &File) -> Result<(), IntakeError> {
    let left = left.metadata().map_err(|_| IntakeError::Storage)?;
    let right = right.metadata().map_err(|_| IntakeError::Storage)?;
    ((left.dev(), left.ino()) == (right.dev(), right.ino()))
        .then_some(())
        .ok_or(IntakeError::Storage)
}
fn qualify(file: &File) -> Result<(), IntakeError> {
    storage::qualify(file).map_err(|error| match error {
        storage::StorageQualificationError::Unsupported => IntakeError::Unsupported,
        storage::StorageQualificationError::Storage => IntakeError::Storage,
    })
}
fn storage_error(error: rustix::io::Errno) -> IntakeError {
    match error {
        rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::INVAL => IntakeError::Unsupported,
        _ => IntakeError::Storage,
    }
}
fn read_error(error: RepositoryReadError) -> IntakeError {
    match error {
        RepositoryReadError::Unsupported => IntakeError::Unsupported,
        _ => IntakeError::Storage,
    }
}
fn bundle_error(error: BundleStoreError) -> IntakeError {
    match error {
        BundleStoreError::Unsupported | BundleStoreError::Read(RepositoryReadError::Unsupported) => {
            IntakeError::Unsupported
        }
        BundleStoreError::Read(RepositoryReadError::Missing | RepositoryReadError::TooLarge)
        | BundleStoreError::Missing
        | BundleStoreError::Conflict
        | BundleStoreError::DigestMismatch
        | BundleStoreError::Codec(_) => IntakeError::Input,
        BundleStoreError::UnsafeDirectory | BundleStoreError::WriteFailed | BundleStoreError::Read(_) => {
            IntakeError::Storage
        }
    }
}
fn publication_error(error: PackPublicationError) -> IntakeError {
    match error {
        PackPublicationError::Unsupported => IntakeError::Unsupported,
        PackPublicationError::Verification(error) => seed_error(error),
        PackPublicationError::InvalidName | PackPublicationError::DestinationExists | PackPublicationError::Storage => {
            IntakeError::Storage
        }
    }
}
fn seed_error(error: SeedError) -> IntakeError {
    match error {
        SeedError::Cancelled => IntakeError::Cancelled,
        SeedError::Unsupported => IntakeError::Unsupported,
        SeedError::Storage | SeedError::UnsafeParent => IntakeError::Storage,
        _ => IntakeError::Input,
    }
}
fn check(cancelled: &dyn Fn() -> bool) -> Result<(), IntakeError> {
    (!cancelled()).then_some(()).ok_or(IntakeError::Cancelled)
}

#[cfg(test)]
mod tests;
