use super::{Change, Error, Journal, Owner, Result, execute};
use crate::cloud_runtime::{Cancellation, bootstrap_initialization};
#[cfg(target_os = "linux")]
use crate::cloud_runtime::{command::Runner, repository};
use horizon_cloud_protocol::{
    ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Artifact, Receipt, Request, Source},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Import the selected committed revision into a prepared project. Artifacts are
/// retained before any remote intent and reused across retries, including when a
/// symbolic revision now resolves differently. Use `resume` after uncertain I/O.
/// No source refresh, checkout, session or credential grant is performed.
/// # Errors
/// Rejects changed inputs, missing source material and conflicting ownership.
pub fn import_source(
    owner: &mut Owner,
    allocation: &bootstrap_initialization::Request,
    project: &ProjectIdentity,
    repository: &Path,
    revision: &str,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Receipt> {
    let descriptor = prepare(owner, project, repository, revision, cancellation)?;
    execute(
        owner,
        allocation,
        &Change::ImportSource(project.clone(), descriptor),
        cancellation,
        timeout,
    )
}
pub(in crate::cloud_runtime) fn prepare(
    owner: &mut Owner,
    project: &ProjectIdentity,
    repository: &Path,
    revision: &str,
    cancellation: &Cancellation,
) -> Result<Source> {
    prepare_with(owner, project, repository, revision, cancellation, &mut |_| Ok(()))
}
pub(in crate::cloud_runtime) fn prepare_with(
    owner: &mut Owner,
    project: &ProjectIdentity,
    repository: &Path,
    revision: &str,
    cancellation: &Cancellation,
    checkpoint: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<Source> {
    let mut journal = Journal::load(owner)?.ok_or(Error::Missing)?;
    let repository = repository.canonicalize()?;
    let artifacts = if let Some(artifacts) = journal.sources.iter().find(|entry| &entry.project == project) {
        if artifacts.repository != repository || artifacts.selection != revision {
            return Err(Error::Invalid);
        }
        artifacts.clone()
    } else {
        if journal.pending.is_some() {
            return Err(Error::Pending);
        }
        if journal.generation.is_some() {
            return Err(Error::Pending);
        }
        if !journal.manifest.members.iter().any(|member| {
            &member.identity == project && member.state == horizon_cloud_protocol::membership::State::Preparing
        }) {
            return Err(Error::Invalid);
        }
        let generation = Generation::new(project, &repository, revision, cancellation)?;
        journal.generation = Some(generation.clone());
        journal.save(owner)?;
        let artifacts = Artifacts::create(owner, &generation, cancellation, checkpoint)?;
        journal.generation = None;
        journal.sources.push(artifacts.clone());
        journal.save(owner)?;
        artifacts
    };
    artifacts.verify(owner.artifact_root()?, cancellation, None)?;
    Ok(artifacts.descriptor)
}

/// An incomplete generation blocks another export; uncertain owned paths are retained.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cloud_runtime) struct Generation {
    pub project: ProjectIdentity,
    repository: PathBuf,
    selection: String,
    revision: String,
    directory: String,
}
impl Generation {
    fn new(project: &ProjectIdentity, repository: &Path, selection: &str, cancellation: &Cancellation) -> Result<Self> {
        if !cfg!(target_os = "linux") {
            return Err(Error::Invalid);
        }
        let runner = crate::cloud_runtime::command::Runner {
            cancel: cancellation,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let generation = Self {
            project: project.clone(),
            repository: repository.into(),
            selection: selection.into(),
            revision: crate::cloud_runtime::repository::resolve_with_runner(repository, selection, &runner)?,
            directory: format!("source-{}", horizon_cloud_protocol::OperationId::generate()),
        };
        generation.validate()?;
        Ok(generation)
    }
    pub(super) fn validate(&self) -> Result<()> {
        if !self.repository.is_absolute()
            || self.selection.len() > 4096
            || self.revision.len() != 40
            || !self.revision.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !self.directory.starts_with("source-")
            || !self
                .directory
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cloud_runtime) struct Artifacts {
    pub project: ProjectIdentity,
    repository: PathBuf,
    selection: String,
    directory: String,
    identity: Identity,
    pack_identity: Identity,
    material_identity: Identity,
    pub descriptor: Source,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn of(file: &File) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = file.metadata()?;
            if meta.mode() & 0o077 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(Error::Invalid);
            }
            Ok(Self {
                device: meta.dev(),
                inode: meta.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Err(Error::Invalid)
        }
    }
    fn require(&self, file: &File) -> Result<()> {
        if self != &Self::of(file)? {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
fn open(path: &Path, directory: bool) -> Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags, openat};
        let mut file = File::open("/")?;
        let components: Vec<_> = path.components().collect();
        if !path.is_absolute() {
            return Err(Error::Invalid);
        }
        for (index, component) in components.iter().enumerate() {
            let name = match component {
                std::path::Component::RootDir => continue,
                std::path::Component::Normal(name) => name,
                _ => return Err(Error::Invalid),
            };
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
            let flags = if directory || index + 1 < components.len() {
                flags | OFlags::DIRECTORY
            } else {
                flags
            };
            file = File::from(openat(&file, *name, flags, Mode::empty()).map_err(std::io::Error::from)?);
        }
        if !directory
            && (!file.metadata()?.is_file() || {
                use std::os::unix::fs::MetadataExt;
                file.metadata()?.nlink() != 1
            })
        {
            return Err(Error::Invalid);
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, directory);
        Err(Error::Invalid)
    }
}
fn fingerprint(
    file: &mut File,
    cancellation: &Cancellation,
    mut output: Option<&mut File>,
    limit: u64,
    deadline: Option<Instant>,
) -> Result<Artifact> {
    file.rewind()?;
    let mut hash = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0; 16384];
    loop {
        cancellation.check().map_err(crate::cloud_runtime::Error::from)?;
        if let Some(deadline) = deadline {
            super::remaining(deadline)?;
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length += read as u64;
        if length > limit {
            return Err(Error::Invalid);
        }
        hash.update(&buffer[..read]);
        if let Some(output) = &mut output {
            output.write_all(&buffer[..read])?;
        }
    }
    Ok(Artifact {
        length,
        sha256: hash.finalize().into(),
    })
}
#[cfg(target_os = "linux")]
fn export(repository: &Path, revision: &str, retained: &Path, owner_root: &Path, runner: &Runner<'_>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Scratch must stay outside the replaceable owner tree, even with a custom TMPDIR.
    let scratch = tempfile::Builder::new()
        .prefix("horizon-source-material-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?;
    if scratch.path().starts_with(owner_root) {
        return Err(Error::Invalid);
    }
    repository::bounded_source(
        repository,
        revision,
        retained,
        scratch.path(),
        runner,
        Source::MAX_BYTES,
    )?;
    scratch.close()?;
    Ok(())
}

impl Artifacts {
    #[cfg(target_os = "linux")]
    fn create(
        owner: &Owner,
        generation: &Generation,
        cancellation: &Cancellation,
        checkpoint: &mut impl FnMut(&Path) -> Result<()>,
    ) -> Result<Self> {
        use rustix::fs::{Mode, OFlags, mkdirat, openat};
        use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
        let root = owner.artifact_root()?;
        let runner = Runner {
            cancel: cancellation,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let parent = open(root, true)?;
        owner.artifact_root()?;
        mkdirat(&parent, &generation.directory, Mode::from_raw_mode(0o700)).map_err(std::io::Error::from)?;
        let path = root.join(&generation.directory);
        let opened = File::from(
            openat(
                &parent,
                &generation.directory,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        );
        let identity = Identity::of(&opened)?;
        let anchored = PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), opened.as_raw_fd()));
        checkpoint(&path)?;
        owner.artifact_root()?;
        identity.require(&open(&path, true)?)?;
        export(&generation.repository, &generation.revision, &anchored, root, &runner)?;
        owner.artifact_root()?;
        identity.require(&open(&path, true)?)?;
        let payload = |name| -> Result<File> {
            use rustix::fs::{Mode, OFlags, openat};
            use std::os::unix::fs::MetadataExt;
            let file = File::from(
                openat(
                    &opened,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(std::io::Error::from)?,
            );
            let meta = file.metadata()?;
            if !meta.is_file() || meta.nlink() != 1 {
                return Err(Error::Invalid);
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
            Ok(file)
        };
        let mut pack = payload("pack")?;
        let mut material = payload("source-material.tar")?;
        let descriptor = Source {
            version: 1,
            revision: generation.revision.clone(),
            pack: fingerprint(&mut pack, cancellation, None, Source::MAX_BYTES, None)?,
            material: fingerprint(&mut material, cancellation, None, Source::MAX_BYTES, None)?,
        };
        descriptor.validate().map_err(|_| Error::Invalid)?;
        pack.sync_all()?;
        material.sync_all()?;
        opened.sync_all()?;
        open(root, true)?.sync_all()?;
        let artifacts = Self {
            project: generation.project.clone(),
            repository: generation.repository.clone(),
            selection: generation.selection.clone(),
            directory: generation.directory.clone(),
            identity,
            pack_identity: Identity::of(&pack)?,
            material_identity: Identity::of(&material)?,
            descriptor,
        };
        artifacts.verify(owner.artifact_root()?, cancellation, None)?;
        // Retain even if the subsequent host journal save is uncertain. Such an
        // unreferenced artifact directory is never adopted by a later operation.
        Ok(artifacts)
    }
    #[cfg(not(target_os = "linux"))]
    fn create(
        _owner: &Owner,
        _generation: &Generation,
        _cancellation: &Cancellation,
        _checkpoint: &mut impl FnMut(&Path) -> Result<()>,
    ) -> Result<Self> {
        Err(Error::Invalid)
    }
    pub fn validate(&self, owner: &Owner) -> Result<()> {
        self.descriptor.validate().map_err(|_| Error::Invalid)?;
        if !self.repository.is_absolute()
            || self.selection.len() > 4096
            || !self.directory.starts_with("source-")
            || !self
                .directory
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(Error::Invalid);
        }
        self.identity
            .require(&open(&owner.artifact_root()?.join(&self.directory), true)?)
    }
    fn files(&self, root: &Path) -> Result<(File, File)> {
        let path = root.join(&self.directory);
        self.identity.require(&open(&path, true)?)?;
        let pack = open(&path.join("pack"), false)?;
        let material = open(&path.join("source-material.tar"), false)?;
        self.pack_identity.require(&pack)?;
        self.material_identity.require(&material)?;
        Ok((pack, material))
    }
    pub(in crate::cloud_runtime) fn verify(
        &self,
        root: &Path,
        cancellation: &Cancellation,
        deadline: Option<Instant>,
    ) -> Result<()> {
        let (mut pack, mut material) = self.files(root)?;
        if fingerprint(&mut pack, cancellation, None, self.descriptor.pack.length, deadline)? != self.descriptor.pack
            || fingerprint(
                &mut material,
                cancellation,
                None,
                self.descriptor.material.length,
                deadline,
            )? != self.descriptor.material
        {
            return Err(Error::Invalid);
        }
        self.files(root)?;
        Ok(())
    }
    pub fn frame(&self, root: &Path, request: &[u8], cancellation: &Cancellation, deadline: Instant) -> Result<File> {
        self.frame_with_limit(root, request, cancellation, Source::MAX_BYTES, deadline)
    }
    pub(in crate::cloud_runtime) fn frame_with_limit(
        &self,
        root: &Path,
        request: &[u8],
        cancellation: &Cancellation,
        limit: u64,
        deadline: Instant,
    ) -> Result<File> {
        super::remaining(deadline)?;
        self.descriptor.validate().map_err(|_| Error::Invalid)?;
        let retained = self.descriptor.pack.length + self.descriptor.material.length;
        if request.len() > Source::MAX_REQUEST_BYTES || retained * 2 + request.len() as u64 + 4 > limit {
            return Err(Error::Invalid);
        }
        let mut output = tempfile::tempfile()?;
        output.write_all(&u32::try_from(request.len()).map_err(|_| Error::Invalid)?.to_be_bytes())?;
        output.write_all(request)?;
        let (mut pack, mut material) = self.files(root)?;
        if fingerprint(
            &mut pack,
            cancellation,
            Some(&mut output),
            self.descriptor.pack.length,
            Some(deadline),
        )? != self.descriptor.pack
            || fingerprint(
                &mut material,
                cancellation,
                Some(&mut output),
                self.descriptor.material.length,
                Some(deadline),
            )? != self.descriptor.material
        {
            return Err(Error::Invalid);
        }
        self.files(root)?;
        output.rewind()?;
        Ok(output)
    }
}
pub(in crate::cloud_runtime) fn for_request<'a>(journal: &'a Journal, bytes: &[u8]) -> Result<&'a Artifacts> {
    let request: RecoveryRequest = serde_json::from_slice(bytes).map_err(|_| Error::Invalid)?;
    let Request::ImportSource { descriptor } = serde_json::from_str(&request.payload).map_err(|_| Error::Invalid)?
    else {
        return Err(Error::Invalid);
    };
    let signed =
        horizon_cloud_protocol::signed::SignedIntent::parse(request.message.as_bytes()).map_err(|_| Error::Invalid)?;
    let intent = signed
        .verify(&journal.binding.startup.controller, request.payload.as_bytes())
        .map_err(|_| Error::Invalid)?;
    let horizon_cloud_protocol::signed::Target::Project { identity } = intent.target() else {
        return Err(Error::Invalid);
    };
    journal
        .sources
        .iter()
        .find(|artifact| artifact.project == *identity && artifact.descriptor == descriptor)
        .ok_or(Error::Invalid)
}
