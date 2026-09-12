use super::{RemoteSshIdentity, RemoteSshIdentityError as Error, command};
use crate::cloud_run::{CloudJobId, CloudWorkflowId, interactive_worker::valid_ssh_public_key};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

const KEY_FILE_LIMIT: u64 = 4096;
const STORE_DIRECTORY: &str = "remote-ssh-identities";

pub(super) fn prepare(home: &Path, workflow: CloudWorkflowId, job: CloudJobId) -> Result<RemoteSshIdentity, Error> {
    let name = key_name(workflow, job)?;
    let directory = directory(home, true)?;
    let destination = directory.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => return retained(&destination, &directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return Err(Error::Storage),
    }
    let staging = StagingDirectory::create(&directory)?;
    let candidate = staging.0.join("identity");
    command::generate(&candidate)?;
    let identity = load(&candidate)?;
    private_file(&candidate)?.sync_all().map_err(|_| Error::Storage)?;
    match atomicwrites::move_atomic(&candidate, &destination) {
        Ok(()) => {
            sync_directory(&directory)?;
            Ok(RemoteSshIdentity {
                private_key_path: destination,
                public_key: identity.public_key,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => retained(&destination, &directory),
        Err(_) => Err(Error::Storage),
    }
}

pub(super) fn recover(
    home: &Path,
    workflow: CloudWorkflowId,
    job: CloudJobId,
    expected: &str,
) -> Result<RemoteSshIdentity, Error> {
    if !valid_public_key(expected) {
        return Err(Error::InvalidIdentity);
    }
    let name = key_name(workflow, job)?;
    let identity = load(&directory(home, false)?.join(name))?;
    if identity.public_key != expected {
        return Err(Error::Mismatch);
    }
    Ok(identity)
}

pub(super) fn validate_home(home: &Path) -> Result<(), Error> {
    // A trailing separator or `.` must not make symlink_metadata follow the final link.
    let root = trusted_home(&home.components().collect::<PathBuf>())?;
    match check_directory(&root, false, false) {
        Ok(()) => Ok(()),
        // A sticky parent protects an existing owned child, not an absent final entry.
        Err(Error::Missing) => check_directory(root.parent().ok_or(Error::InsecurePath)?, false, false),
        Err(error) => Err(error),
    }
}

fn directory(home: &Path, create: bool) -> Result<PathBuf, Error> {
    let root = trusted_home(home)?;
    check_directory(&root, create, false)?;
    let directory = root.join(STORE_DIRECTORY);
    check_directory(&directory, create, true)?;
    if create {
        // Make both newly created directory entries durable before reserving a public request.
        sync_directory(root.parent().ok_or(Error::InsecurePath)?)?;
        sync_directory(&root)?;
    }
    Ok(directory)
}

fn trusted_home(home: &Path) -> Result<PathBuf, Error> {
    let absolute = std::path::absolute(home).map_err(|_| Error::Storage)?;
    if absolute.components().any(|component| component == Component::ParentDir) {
        return Err(Error::InsecurePath);
    }
    let parent = absolute.parent().ok_or(Error::InsecurePath)?;
    let uid = rustix::process::geteuid().as_raw();
    let mut ancestor = PathBuf::new();
    // Walk from the trusted root before creating anything. A sticky ancestor
    // is safe only because every child below it must also belong to us or root.
    for component in parent.components() {
        ancestor.push(component);
        let metadata = fs::symlink_metadata(&ancestor).map_err(|error| storage_error(&error))?;
        let mode = metadata.permissions().mode();
        if !metadata.is_dir()
            || (metadata.uid() != uid && metadata.uid() != 0)
            || (mode & 0o022 != 0 && mode & 0o1000 == 0)
        {
            return Err(Error::InsecurePath);
        }
    }
    Ok(absolute)
}

fn check_directory(path: &Path, create: bool, private: bool) -> Result<(), Error> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(Error::Storage),
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| storage_error(&error))?;
    let forbidden = if private { 0o077 } else { 0o022 };
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & forbidden != 0
    {
        return Err(Error::InsecurePath);
    }
    Ok(())
}

fn key_name(workflow: CloudWorkflowId, job: CloudJobId) -> Result<String, Error> {
    if workflow.to_string() == uuid::Uuid::nil().to_string() || job.to_string() == uuid::Uuid::nil().to_string() {
        return Err(Error::InvalidIdentity);
    }
    Ok(format!("{workflow}-{job}.key"))
}

fn retained(path: &Path, directory: &Path) -> Result<RemoteSshIdentity, Error> {
    let identity = load(path)?;
    private_file(path)?.sync_all().map_err(|_| Error::Storage)?;
    sync_directory(directory)?;
    Ok(identity)
}

fn sync_directory(path: &Path) -> Result<(), Error> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| Error::Storage)
}

fn load(path: &Path) -> Result<RemoteSshIdentity, Error> {
    // Hold the inspected file open while the trusted key utility verifies its contents.
    // The enclosing directories are owner-only; same-user tampering is outside this boundary.
    let file = private_file(path)?;
    let public_key = command::public_key(path)?;
    if !valid_public_key(&public_key) {
        return Err(Error::InvalidIdentity);
    }
    drop(file);
    Ok(RemoteSshIdentity {
        private_key_path: path.to_path_buf(),
        public_key,
    })
}

fn private_file(path: &Path) -> Result<File, Error> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                Error::InsecurePath
            } else {
                storage_error(&error)
            }
        })?;
    let metadata = file.metadata().map_err(|_| Error::Storage)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(Error::InsecurePath);
    }
    if metadata.len() == 0 || metadata.len() > KEY_FILE_LIMIT {
        return Err(Error::InvalidIdentity);
    }
    Ok(file)
}

fn valid_public_key(key: &str) -> bool {
    key.len() <= 128 && key.split(' ').count() == 2 && valid_ssh_public_key(key)
}

fn storage_error(error: &io::Error) -> Error {
    if error.kind() == io::ErrorKind::NotFound {
        Error::Missing
    } else {
        Error::Storage
    }
}

struct StagingDirectory(PathBuf);

impl StagingDirectory {
    fn create(parent: &Path) -> Result<Self, Error> {
        let path = parent.join(format!(".new-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|_| Error::Storage)?;
        Ok(Self(path))
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        // Only this successfully created random staging directory is owned by the operation.
        let _ = fs::remove_dir_all(&self.0);
    }
}
