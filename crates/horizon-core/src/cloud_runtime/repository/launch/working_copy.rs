//! The settings New cloud and a rebuild read while the checked-out commit is the one they
//! use. Both build the image from the working copy (`repository::uncommitted`), so the
//! working copy of `.horizon/cloud.yml` applies whole and an edit applies without a commit.
//! Only the source and the companions come from the commit: the source is a committed
//! archive, and companions name commits of their own.
use super::{CloudConfig, Error, Path, Runner};
use horizon_cloud::Cancellation;
use std::{
    fs::File,
    hash::{Hash, Hasher},
    io::Read,
    process::Command,
    time::Duration,
};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const FOLDER: &str = ".horizon";
const FILE: &str = "cloud.yml";

const MISSING: &str = "The working copy has no .horizon/cloud.yml, so the committed settings apply.";
const INVALID: &str = "The working copy of .horizon/cloud.yml is not valid, so the committed settings apply.";

enum WorkingFile {
    Missing,
    Invalid,
    Valid(CloudConfig),
}

/// The settings New cloud offers for the checked-out commit, whose own file is `committed`,
/// with the notes that say which working-copy edits do not apply ([`whole`]).
pub(super) fn apply(
    repository: &Path,
    committed: Option<CloudConfig>,
    runner: &Runner<'_>,
) -> super::super::Result<(CloudConfig, Vec<String>)> {
    runner.cancel.check()?;
    let file = read(repository);
    runner.cancel.check()?;
    if matches!((&file, &committed), (WorkingFile::Invalid, None)) {
        return Err(Error::Invalid(super::INVALID_CONFIG));
    }
    whole(file, committed).ok_or(Error::Invalid(super::MISSING_CONFIG))
}

/// The settings a rebuild reads at the checked-out commit, whose own file is `committed`,
/// as New cloud reads them ([`whole`]); `None` when neither has a valid file.
pub(in crate::cloud_runtime) fn for_rebuild(
    repository: &Path,
    committed: Option<CloudConfig>,
    runner: &Runner<'_>,
) -> super::super::Result<(Option<CloudConfig>, Vec<String>)> {
    runner.cancel.check()?;
    let file = read(repository);
    runner.cancel.check()?;
    Ok(whole(file, committed).map_or((None, Vec::new()), |(config, notes)| (Some(config), notes)))
}

/// The working copy's `file` whole, with the commit's source and companions, which name
/// what comes from commits. The commit's file applies when the working copy has no valid
/// one; a note says so.
fn whole(file: WorkingFile, committed: Option<CloudConfig>) -> Option<(CloudConfig, Vec<String>)> {
    match (file, committed) {
        (WorkingFile::Valid(working), committed) => {
            let (source, companions) = committed
                .map(|config| (config.source, config.companions))
                .unwrap_or_default();
            let mut ignored = Vec::new();
            note_field(&mut ignored, "source", source != working.source);
            note_field(&mut ignored, "companions", companions != working.companions);
            let notes = (!ignored.is_empty())
                .then(|| {
                    format!(
                        "Uncommitted changes to {} are not used until you commit them.",
                        list(&ignored)
                    )
                })
                .into_iter()
                .collect();
            let config = CloudConfig {
                source,
                companions,
                ..working
            };
            Some((config, notes))
        }
        (WorkingFile::Missing, Some(committed)) => Some((committed, vec![MISSING.into()])),
        (WorkingFile::Invalid, Some(committed)) => Some((committed, vec![INVALID.into()])),
        (_, None) => None,
    }
}

fn read(repository: &Path) -> WorkingFile {
    match contents(repository) {
        None => WorkingFile::Missing,
        Some(Ok(yaml)) => CloudConfig::parse(&yaml).map_or(WorkingFile::Invalid, WorkingFile::Valid),
        Some(Err(())) => WorkingFile::Invalid,
    }
}

fn note_field(fields: &mut Vec<&'static str>, field: &'static str, changed: bool) {
    if changed && !fields.contains(&field) {
        fields.push(field);
    }
}

/// `a`, `a and b`, `a, b and c`.
fn list(items: &[impl AsRef<str>]) -> String {
    match items {
        [] => String::new(),
        [only] => only.as_ref().to_owned(),
        [rest @ .., last] => format!(
            "{} and {}",
            rest.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", "),
            last.as_ref()
        ),
    }
}

/// The text of `.horizon/cloud.yml` in `repository`, a regular file of at most 1 MiB;
/// `None` when there is no such file. A setup agent or an editor can swap the file or its
/// folder while Horizon reads, so on Unix both are opened without following a link and the
/// file without waiting on a pipe, and the opened handle is checked, not the path. Windows
/// opens a link itself, never its target, and holds the folder so it cannot be swapped.
fn contents(repository: &Path) -> Option<Result<String, ()>> {
    let file = match open(repository) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(Err(())),
    };
    let read = || {
        if !file.metadata().map_err(|_| ())?.is_file() {
            return Err(());
        }
        let mut yaml = String::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut yaml)
            .map_err(|_| ())?;
        if yaml.len() as u64 > MAX_CONFIG_BYTES {
            return Err(());
        }
        Ok(yaml)
    };
    Some(read())
}

#[cfg(unix)]
fn open(repository: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    let folder = rustix::fs::open(
        repository.join(FOLDER),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let file = rustix::fs::openat(
        &folder,
        FILE,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(File::from(file))
}

#[cfg(windows)]
fn open(repository: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // Win32 values: open a link itself rather than its target, open a folder, and share
    // the folder for reading and writing but not for deletion or renaming.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    let not_followed = || std::io::Error::other("is a link or not the expected kind of entry");
    // Held open while the file opens, so the path keeps naming this real folder.
    let folder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(repository.join(FOLDER))?;
    if !folder.metadata()?.is_dir() {
        return Err(not_followed());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(repository.join(FOLDER).join(FILE))?;
    if !file.metadata()?.is_file() {
        return Err(not_followed());
    }
    Ok(file)
}

/// What New cloud's settings depend on besides the typed revision: the checked-out commit
/// and the working copy's `.horizon/cloud.yml`. When it changes, the settings are read again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fingerprint {
    head: Option<String>,
    file: Option<u64>,
}

/// The [`Fingerprint`] of the checkout that contains `directory`. It runs one Git command
/// and reads at most 1 MiB, so it suits a poll every second off the UI thread.
#[must_use]
pub fn fingerprint(directory: &str, cancel: &Cancellation) -> Fingerprint {
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let Ok(output) = runner.run(
        "Check the checked-out commit",
        Command::new("git")
            .arg("-C")
            .arg(crate::Config::expand_tilde(directory))
            .args(["rev-parse", "--show-toplevel", "HEAD"]),
        Duration::from_secs(10),
    ) else {
        return Fingerprint::default();
    };
    // The commit is the last line; the folder before it may itself hold a newline.
    let Some((root, head)) = output.strip_suffix('\n').unwrap_or(&output).rsplit_once('\n') else {
        return Fingerprint::default();
    };
    let file = contents(Path::new(root)).map(|text| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    });
    Fingerprint {
        head: Some(head.trim_end().to_owned()),
        file,
    }
}

#[cfg(test)]
mod tests;
