//! Bounded project reads through the host's captured root capability.
use crate::{Error, Result};
use std::fs::File;
use std::path::Path;

#[cfg(unix)]
pub(crate) fn open(root: &File, relative: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags};
    let mut file = root.try_clone().map_err(|_| Error::Unavailable)?;
    let mut components = relative.components().peekable();
    if components.peek().is_none() {
        return Err(Error::Unavailable);
    }
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return Err(Error::Unavailable);
        };
        let mut flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        if components.peek().is_some() {
            flags |= OFlags::DIRECTORY;
        }
        file = File::from(rustix::fs::openat(&file, name, flags, Mode::empty()).map_err(|_| Error::Unavailable)?);
    }
    if !file.metadata().map_err(|_| Error::Unavailable)?.is_file() {
        return Err(Error::Unavailable);
    }
    Ok(file)
}
#[cfg(unix)]
pub(crate) fn read(root: &File, relative: &Path) -> Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    open(root, relative)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::Unavailable);
    }
    String::from_utf8(bytes).map_err(|_| Error::Unavailable)
}
#[cfg(not(unix))]
pub(crate) fn read(_root: &File, _relative: &Path) -> Result<String> {
    Err(Error::Unavailable)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn anchored_project_reads_do_not_follow_replaced_root_or_symlink_components() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("project");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("recipe.md"), "owned recipe").unwrap();
        let held = File::open(&root).unwrap();
        let other = folder.path().join("outside");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("recipe.md"), "outside recipe").unwrap();
        symlink(&other, root.join("escape")).unwrap();
        symlink(other.join("recipe.md"), root.join("alias.md")).unwrap();
        for path in ["escape/recipe.md", "alias.md", "../outside/recipe.md", "", "/recipe.md"] {
            assert_eq!(read(&held, Path::new(path)), Err(Error::Unavailable));
        }
        std::fs::rename(&root, folder.path().join("retained")).unwrap();
        symlink(&other, &root).unwrap();
        assert_eq!(read(&held, Path::new("recipe.md")).unwrap(), "owned recipe");
    }

    #[test]
    fn project_read_rejects_nonregular_invalid_utf8_and_oversized_recipe_inputs() {
        let folder = tempfile::tempdir().unwrap();
        let held = File::open(folder.path()).unwrap();
        std::fs::write(folder.path().join("large.md"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
        std::fs::write(folder.path().join("invalid.md"), [0xff, 0xfe]).unwrap();
        #[cfg(not(target_os = "macos"))]
        rustix::fs::mknodat(
            &held,
            "pipe.md",
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        #[cfg(target_os = "macos")]
        assert!(
            std::process::Command::new("/usr/bin/mkfifo")
                .args(["-m", "600"])
                .arg(folder.path().join("pipe.md"))
                .status()
                .unwrap()
                .success()
        );
        std::fs::create_dir(folder.path().join("directory.md")).unwrap();
        for path in ["large.md", "invalid.md", "pipe.md", "directory.md"] {
            assert_eq!(read(&held, Path::new(path)), Err(Error::Unavailable));
        }
        std::fs::write(folder.path().join("boundary.md"), vec![b'x'; 1024 * 1024]).unwrap();
        assert_eq!(read(&held, Path::new("boundary.md")).unwrap().len(), 1024 * 1024);
    }
}
