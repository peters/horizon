use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub(super) fn read_existing(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn write_text_atomic(path: &Path, content: &str) -> io::Result<()> {
    let destination = write_destination(path)?;
    if std::fs::read_to_string(&destination).ok().as_deref() == Some(content) {
        return Ok(());
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    temp_file.flush()?;
    temp_file.persist(&destination).map_err(|error| error.error)?;
    Ok(())
}

fn write_destination(path: &Path) -> io::Result<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(path)?;
            if target.is_absolute() {
                Ok(target)
            } else {
                Ok(path.parent().unwrap_or_else(|| Path::new(".")).join(target))
            }
        }
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error),
    }
}
