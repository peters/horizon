//! Signed allocation bootstrap commands and private SSH-only startup.
#[cfg(target_os = "linux")]
mod initialize;
#[cfg(target_os = "linux")]
mod inspection;
#[cfg(target_os = "linux")]
mod keys;
#[cfg(target_os = "linux")]
mod membership;
#[cfg(target_os = "linux")]
mod namespaces;
#[cfg(target_os = "linux")]
mod recovery;
#[cfg(target_os = "linux")]
mod runtime;
#[cfg(target_os = "linux")]
mod store;
use std::io;

pub(super) fn inspect() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        inspection::run()
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::other(
        "Allocation inspection requires a qualified Linux worker",
    ))
}

pub(super) fn run() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        recovery::run()
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::other(
        "Allocation recovery requires a qualified Linux worker",
    ))
}

pub(super) fn initialize(abandon: bool) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        initialize::run(abandon)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = abandon;
        Err(io::Error::other(
            "Allocation initialization requires a qualified Linux worker",
        ))
    }
}

pub(super) fn membership(action: horizon_cloud_protocol::signed::Action) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        membership::run(action)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = action;
        Err(io::Error::other(
            "Project reservations require a qualified Linux worker",
        ))
    }
}

pub(super) fn prepare() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use recovery::{BOOTSTRAP, Bootstrap, ROOT, decode};
        use runtime::{RUN_ROOT, Runtime};
        use rustix::fs::{Mode, OFlags, openat};
        use std::{fs::File, path::Path};
        use store::{Store, invalid, open_directory, regular};
        if std::env::args().len() != 2 {
            return Err(invalid());
        }
        let runtime = Runtime::capture()?;
        let directory = open_directory(Path::new(RUN_ROOT))?;
        match openat(
            &directory,
            "startup.lock",
            OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => {
                File::from(fd).sync_all()?;
                directory.sync_all()?;
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let lock = regular(&directory, "startup.lock")?;
        lock.try_lock().map_err(|_| invalid())?;
        match std::fs::symlink_metadata(ROOT) {
            Ok(_) => {
                let store = Store::open(Path::new(ROOT))?;
                let record: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
                record.validate(&store, &runtime)?;
                if record.version != 2 {
                    return Err(invalid());
                }
                membership::startup(&store, &record)?;
                keys::install(Path::new(RUN_ROOT), &store.host_key()?)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Store::require_pristine(Path::new("/workspace"))?;
                keys::fresh(Path::new(RUN_ROOT))?;
                Store::require_pristine(Path::new("/workspace"))?;
            }
            Err(error) => return Err(error),
        }
        keys::seal(Path::new(RUN_ROOT))?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::other("Allocation startup requires a qualified Linux worker"))
}
