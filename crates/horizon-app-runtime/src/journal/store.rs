use super::{Ledger, validate};
use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;
use uuid::Uuid;
use zeroize::Zeroizing;

pub(super) mod execution;

const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Initialization {
    Allowed,
    Forbidden,
}

pub(super) struct Store {
    directory: File,
    registry: File,
    #[cfg(unix)]
    parent: File,
    #[cfg(unix)]
    basename: std::ffi::OsString,
    namespace: String,
}
impl Store {
    pub(super) fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, || ())
    }
    pub(super) fn open_existing(path: &Path) -> Result<Self> {
        Self::open_mode(path, Initialization::Forbidden, || ())
    }
    pub(super) fn open_with(path: &Path, after_registry: impl FnOnce()) -> Result<Self> {
        Self::open_mode(path, Initialization::Allowed, after_registry)
    }
    fn open_mode(path: &Path, initialization: Initialization, after_registry: impl FnOnce()) -> Result<Self> {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;
        let parent = path.parent().ok_or(Error::JournalUnavailable)?;
        let parent = private_directory(parent, initialization)?;
        let registry = if initialization == Initialization::Allowed {
            private_child(&parent, std::ffi::OsStr::new(".native-journal-registry"))?
        } else {
            existing_child(&parent, std::ffi::OsStr::new(".native-journal-registry"))?
        };
        let registry_lock = open_file(&registry, "journal.lock", initialization == Initialization::Allowed)?;
        registry_lock.lock().map_err(|_| Error::JournalUnavailable)?;
        let filename = path.file_name().ok_or(Error::JournalUnavailable)?;
        let digest = Sha256::digest(filename.as_encoded_bytes());
        let namespace = digest.iter().fold(String::from("namespace-"), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        });
        let expected = match open_file(&registry, &namespace, false) {
            Ok(marker) => Some(small(&marker)?),
            Err(Error::JournalMissing) if initialization == Initialization::Allowed => None,
            Err(error) => return Err(error),
        };
        after_registry();
        let directory = if expected.is_some() {
            existing_child(&parent, filename).map_err(|_| Error::JournalInvalid)?
        } else {
            private_child(&parent, filename)?
        };
        let lock = open_file(&directory, "journal.lock", expected.is_none()).map_err(|error| {
            if expected.is_some() {
                Error::JournalInvalid
            } else {
                error
            }
        })?;
        lock.lock().map_err(|_| Error::JournalUnavailable)?;
        if let Some(expected) = expected {
            check_marker(&directory, &lock)?;
            if small(&lock)? != expected {
                return Err(Error::JournalInvalid);
            }
            open_file(&directory, "journal.json", false).map_err(|_| Error::JournalInvalid)?;
        } else {
            // A durable reservation precedes initialization. An interrupted first open holds state.
            let mut marker = open_file(&registry, &namespace, true)?;
            marker
                .write_all(b"initializing")
                .and_then(|()| marker.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            registry.sync_all().map_err(|_| Error::JournalUnavailable)?;
            initialize(&directory, &lock)?;
            let identity = small(&lock)?;
            marker
                .rewind()
                .and_then(|()| marker.set_len(0))
                .and_then(|()| marker.write_all(&identity))
                .and_then(|()| marker.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            registry.sync_all().map_err(|_| Error::JournalUnavailable)?;
        }
        Ok(Self {
            directory,
            registry,
            #[cfg(unix)]
            parent,
            #[cfg(unix)]
            basename: filename.to_owned(),
            namespace,
        })
    }
    fn check_location(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags};
            for (name, held) in [
                (self.basename.as_os_str(), &self.directory),
                (std::ffi::OsStr::new(".native-journal-registry"), &self.registry),
            ] {
                let observed = File::from(
                    rustix::fs::openat(
                        &self.parent,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(|_| Error::JournalInvalid)?,
                );
                if file_identity(&observed)? != file_identity(held)? {
                    return Err(Error::JournalInvalid);
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            Err(Error::JournalInvalid)
        }
    }
    pub(super) fn access<T>(&self, write: bool, operation: impl FnOnce(&mut Ledger) -> Result<T>) -> Result<T> {
        self.check_location()?;
        let lock = open_file(&self.directory, "journal.lock", false).map_err(|_| Error::JournalInvalid)?;
        lock.lock().map_err(|_| Error::JournalUnavailable)?;
        check_marker(&self.directory, &lock)?;
        let marker = open_file(&self.registry, &self.namespace, false).map_err(|_| Error::JournalInvalid)?;
        if small(&marker)? != small(&lock)? {
            return Err(Error::JournalInvalid);
        }
        let mut ledger = match open_file(&self.directory, "journal.json", false) {
            Ok(file) => {
                let mut bytes = Zeroizing::new(Vec::new());
                file.take(MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::JournalUnavailable)?;
                if bytes.len() as u64 > MAX_BYTES {
                    return Err(Error::JournalInvalid);
                }
                serde_json::from_slice(&bytes).map_err(|_| Error::JournalInvalid)?
            }
            Err(Error::JournalMissing) => return Err(Error::JournalInvalid),
            Err(error) => return Err(error),
        };
        validate(&ledger)?;
        let result = operation(&mut ledger)?;
        self.check_location()?;
        if !write {
            return Ok(result);
        }
        validate(&ledger)?;
        let bytes = Zeroizing::new(serde_json::to_vec(&ledger).map_err(|_| Error::JournalInvalid)?);
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::JournalInvalid);
        }
        let name = format!("journal-{}.partial", Uuid::new_v4());
        let mut temporary = open_file(&self.directory, &name, true)?;
        #[cfg(unix)]
        let _partial = Partial {
            directory: &self.directory,
            name: &name,
        };
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.sync_all())
            .map_err(|_| Error::JournalUnavailable)?;
        replace(&self.directory, &name)?;
        self.directory.sync_all().map_err(|_| Error::JournalUnavailable)?;
        Ok(result)
    }
}

#[cfg(unix)]
struct Partial<'a> {
    directory: &'a File,
    name: &'a str,
}
#[cfg(unix)]
impl Drop for Partial<'_> {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = rustix::fs::unlinkat(self.directory, self.name, rustix::fs::AtFlags::empty());
        }
    }
}

fn small(mut file: &File) -> Result<Vec<u8>> {
    file.rewind().map_err(|_| Error::JournalUnavailable)?;
    let mut bytes = Vec::new();
    file.take(129)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::JournalUnavailable)?;
    if bytes.len() > 128 {
        return Err(Error::JournalInvalid);
    }
    Ok(bytes)
}

fn check_marker(directory: &File, lock: &File) -> Result<()> {
    let marker = open_file(directory, "initialized", false).map_err(|_| Error::JournalInvalid)?;
    let expected = small(&marker)?;
    let text = std::str::from_utf8(&expected).map_err(|_| Error::JournalInvalid)?;
    let mut fields = text
        .strip_prefix("native-lock-v2:")
        .ok_or(Error::JournalInvalid)?
        .split(':');
    let nonce = fields
        .next()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(Error::JournalInvalid)?;
    let device = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(Error::JournalInvalid)?;
    let inode = fields
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(Error::JournalInvalid)?;
    if nonce.is_nil() || fields.next().is_some() || file_identity(lock)? != (device, inode) || small(lock)? != expected
    {
        return Err(Error::JournalInvalid);
    }
    Ok(())
}

fn file_identity(file: &File) -> Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(|_| Error::JournalUnavailable)?;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Err(Error::JournalUnavailable)
    }
}

fn initialize(directory: &File, mut lock: &File) -> Result<()> {
    match open_file(directory, "initialized", false) {
        Ok(_) => {
            check_marker(directory, lock)?;
            open_file(directory, "journal.json", false).map_err(|_| Error::JournalInvalid)?;
            Ok(())
        }
        Err(Error::JournalMissing) => {
            if !small(lock)?.is_empty()
                || !matches!(open_file(directory, "journal.json", false), Err(Error::JournalMissing))
            {
                return Err(Error::JournalInvalid);
            }
            let (device, inode) = file_identity(lock)?;
            let marker = format!("native-lock-v2:{}:{device}:{inode}", Uuid::new_v4());
            lock.write_all(marker.as_bytes())
                .and_then(|()| lock.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            let mut file = open_file(directory, "initialized", true)?;
            file.write_all(marker.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            directory.sync_all().map_err(|_| Error::JournalUnavailable)?;
            let mut file = open_file(directory, "journal.json", true)?;
            file.write_all(br#"{"version":1,"records":{}}"#)
                .and_then(|()| file.sync_all())
                .map_err(|_| Error::JournalUnavailable)?;
            directory.sync_all().map_err(|_| Error::JournalUnavailable)
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn private_directory(path: &Path, initialization: Initialization) -> Result<File> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute() {
        return Err(Error::JournalUnavailable);
    }
    let mut current = File::open("/").map_err(|_| Error::JournalUnavailable)?;
    for component in path.components().skip(1) {
        let std::path::Component::Normal(name) = component else {
            return Err(Error::JournalUnavailable);
        };
        match rustix::fs::openat(
            &current,
            name,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(next) => current = File::from(next),
            Err(rustix::io::Errno::NOENT) if initialization == Initialization::Allowed => {
                rustix::fs::mkdirat(
                    &current,
                    name,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR | rustix::fs::Mode::XUSR,
                )
                .or_else(|error| {
                    if error == rustix::io::Errno::EXIST {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|_| Error::JournalUnavailable)?;
                current.sync_all().map_err(|_| Error::JournalUnavailable)?;
                current = File::from(
                    rustix::fs::openat(
                        &current,
                        name,
                        rustix::fs::OFlags::RDONLY
                            | rustix::fs::OFlags::DIRECTORY
                            | rustix::fs::OFlags::NOFOLLOW
                            | rustix::fs::OFlags::CLOEXEC,
                        rustix::fs::Mode::empty(),
                    )
                    .map_err(|_| Error::JournalUnavailable)?,
                );
            }
            Err(_) => return Err(Error::JournalUnavailable),
        }
    }
    let metadata = current.metadata().map_err(|_| Error::JournalUnavailable)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(Error::JournalUnavailable);
    }
    Ok(current)
}

#[cfg(unix)]
fn private_child(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    use rustix::fs::{Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let open = || rustix::fs::openat(parent, name, flags, Mode::empty());
    let child = match open() {
        Ok(file) => file,
        Err(rustix::io::Errno::NOENT) => {
            rustix::fs::mkdirat(parent, name, Mode::RWXU)
                .or_else(|error| {
                    if error == rustix::io::Errno::EXIST {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|_| Error::JournalUnavailable)?;
            parent.sync_all().map_err(|_| Error::JournalUnavailable)?;
            open().map_err(|_| Error::JournalUnavailable)?
        }
        Err(_) => return Err(Error::JournalUnavailable),
    };
    validate_private_child(File::from(child))
}

#[cfg(unix)]
fn existing_child(parent: &File, name: &std::ffi::OsStr) -> Result<File> {
    use rustix::fs::{Mode, OFlags};
    let child = File::from(
        rustix::fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| Error::JournalUnavailable)?,
    );
    validate_private_child(child)
}

#[cfg(unix)]
fn validate_private_child(child: File) -> Result<File> {
    use std::os::unix::fs::MetadataExt;
    let metadata = child.metadata().map_err(|_| Error::JournalUnavailable)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(Error::JournalUnavailable);
    }
    Ok(child)
}

#[cfg(not(unix))]
fn existing_child(_parent: &File, _name: &std::ffi::OsStr) -> Result<File> {
    Err(Error::JournalUnavailable)
}

#[cfg(not(unix))]
fn private_child(_parent: &File, _name: &std::ffi::OsStr) -> Result<File> {
    Err(Error::JournalUnavailable)
}

#[cfg(unix)]
fn open_file(directory: &File, name: &str, create: bool) -> Result<File> {
    use std::os::unix::fs::MetadataExt;
    let mut flags = rustix::fs::OFlags::RDWR
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::NONBLOCK
        | rustix::fs::OFlags::CLOEXEC;
    if create {
        flags |= rustix::fs::OFlags::CREATE;
        flags |= rustix::fs::OFlags::EXCL;
    }
    let file = File::from(
        rustix::fs::openat(directory, name, flags, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)
            .or_else(|error| {
                // Concurrent non-exclusive O_CREAT can return ENOENT on macOS.
                // Only the shared lock may already exist; reopen without creating or truncating it.
                if create && name == "journal.lock" && error == rustix::io::Errno::EXIST {
                    rustix::fs::openat(
                        directory,
                        name,
                        flags & !(rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL),
                        rustix::fs::Mode::empty(),
                    )
                } else {
                    Err(error)
                }
            })
            .map_err(|error| {
                #[cfg(test)]
                if create {
                    let stage = if matches!(name, "journal.lock" | "initialized" | "journal.json") {
                        name
                    } else {
                        "marker"
                    };
                    eprintln!("native journal create failed: file={stage} errno={error:?}");
                }
                if error == rustix::io::Errno::NOENT {
                    Error::JournalMissing
                } else {
                    Error::JournalUnavailable
                }
            })?,
    );
    let metadata = file.metadata().map_err(|_| Error::JournalUnavailable)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(Error::JournalUnavailable);
    }
    Ok(file)
}

#[cfg(unix)]
fn replace(directory: &File, name: &str) -> Result<()> {
    #[cfg(test)]
    if FAIL_REPLACE.with(std::cell::Cell::get) {
        return Err(Error::JournalUnavailable);
    }
    rustix::fs::renameat(directory, name, directory, "journal.json").map_err(|_| Error::JournalUnavailable)
}

#[cfg(not(unix))]
fn private_directory(_path: &Path, _initialization: Initialization) -> Result<File> {
    Err(Error::JournalUnavailable)
}
#[cfg(not(unix))]
fn open_file(_directory: &File, _name: &str, _create: bool) -> Result<File> {
    Err(Error::JournalUnavailable)
}
#[cfg(not(unix))]
fn replace(_directory: &File, _name: &str) -> Result<()> {
    Err(Error::JournalUnavailable)
}

#[cfg(test)]
thread_local! { pub(super) static FAIL_REPLACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[cfg(all(test, unix))]
mod tests;
