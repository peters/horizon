use crate::{Error, Result};
use std::path::Path;

#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags};
#[cfg(unix)]
use std::{fs::File, io::Write};

#[cfg(unix)]
pub(crate) struct Directory {
    file: File,
}

#[cfg(unix)]
impl Directory {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        if !path.is_absolute() {
            return Err(Error::StateUnavailable);
        }
        let mut file = File::open("/").map_err(|_| Error::StateUnavailable)?;
        for component in path.components().skip(1) {
            let std::path::Component::Normal(name) = component else {
                return Err(Error::StateUnavailable);
            };
            file = File::from(
                rustix::fs::openat(
                    &file,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| Error::StateUnavailable)?,
            );
        }
        let metadata = file.metadata().map_err(|_| Error::StateUnavailable)?;
        if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(Error::StateUnavailable);
        }
        Ok(Self { file })
    }

    pub(crate) fn new_file(&self, name: &str) -> Result<File> {
        let file = File::from(
            rustix::fs::openat(
                &self.file,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|_| Error::StateUnavailable)?,
        );
        self.file.sync_all().map_err(|_| Error::StateUnavailable)?;
        Ok(file)
    }

    pub(crate) fn save(&self, value: &impl serde::Serialize) -> Result<()> {
        let name = format!("process-{}.partial", uuid::Uuid::new_v4());
        let cleanup = Partial {
            directory: &self.file,
            name: &name,
        };
        let mut file = self.new_file(&name)?;
        serde_json::to_writer(&mut file, value).map_err(|_| Error::StateUnavailable)?;
        file.flush()
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::StateUnavailable)?;
        rustix::fs::renameat(&self.file, &name, &self.file, "process.json").map_err(|_| Error::StateUnavailable)?;
        self.file.sync_all().map_err(|_| Error::StateUnavailable)?;
        drop(cleanup);
        Ok(())
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
        let _ = rustix::fs::unlinkat(self.directory, self.name, AtFlags::empty());
    }
}

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        Directory::open(path).map(|_| ())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::StateUnavailable)
    }
}
