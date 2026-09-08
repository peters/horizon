//! Shared Linux storage qualification, without synchronization or write authority.

use super::paths;
use rustix::fs::{CWD, fstatfs, major, minor, readlinkat_raw};
use std::{fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum StorageQualificationError {
    #[error("repository storage is not a supported journaled filesystem")]
    Unsupported,
    #[error("repository storage qualification failed")]
    Storage,
}

use StorageQualificationError as Error;

/// Requires trusted kernel metadata and an unchanged mount configuration.
pub(super) fn qualify(parent: &File) -> Result<(), Error> {
    if fstatfs(parent).map_err(|_| Error::Storage)?.f_type != libc::EXT4_SUPER_MAGIC {
        return Err(Error::Unsupported);
    }
    let device = parent.metadata().map_err(|_| Error::Storage)?.dev();
    let mut buffer = [0; 4097];
    let length = readlinkat_raw(
        CWD,
        format!("/sys/dev/block/{}:{}", major(device), minor(device)),
        &mut buffer[..],
    )
    .map_err(|_| Error::Unsupported)?;
    if length == buffer.len() {
        return Err(Error::Unsupported);
    }
    let target = std::str::from_utf8(&buffer[..length]).map_err(|_| Error::Unsupported)?;
    let name = Path::new(target)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.len() <= 255 && paths::validate(name).is_ok())
        .ok_or(Error::Unsupported)?;
    let mut options = String::new();
    File::open(format!("/proc/fs/ext4/{name}/options"))
        .map_err(|_| Error::Unsupported)?
        .take(4097)
        .read_to_string(&mut options)
        .map_err(|_| Error::Unsupported)?;
    journaled_options(&options)
}

fn journaled_options(options: &str) -> Result<(), Error> {
    if options.len() > 4096 || !options.ends_with('\n') {
        return Err(Error::Unsupported);
    }
    let lines: std::collections::BTreeSet<_> = options.split_terminator('\n').collect();
    if lines.len() != options.split_terminator('\n').count()
        || lines
            .iter()
            .any(|line| line.is_empty() || line.bytes().any(|b| b.is_ascii_control() || b == b' '))
        || !lines.contains("rw")
        || !lines.contains("barrier")
        || lines.contains("ro")
        || lines.contains("nobarrier")
        || lines.iter().filter(|line| line.starts_with("data=")).count() != 1
        || !(lines.contains("data=ordered") || lines.contains("data=journal"))
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_contract_requires_exact_noncontradictory_bounded_kernel_options() {
        for mode in ["ordered", "journal"] {
            assert_eq!(journaled_options(&format!("rw\nbarrier\ndata={mode}\n")), Ok(()));
        }
        for tail in [
            "",
            "data=writeback\n",
            "data=ordered",
            "data=ordered\r\n",
            "data=ordered\nro\n",
            "data=ordered\nnobarrier\n",
            "data=ordered\nbarrier\n",
            "data=ordered\ndata=journal\n",
            "\n",
        ] {
            assert_eq!(
                journaled_options(&format!("rw\nbarrier\n{tail}")),
                Err(Error::Unsupported)
            );
        }
        assert!(journaled_options("rw\ndata=ordered\n").is_err());
        assert!(journaled_options(&"rw\nbarrier\ndata=ordered\n".repeat(200)).is_err());
    }
}
