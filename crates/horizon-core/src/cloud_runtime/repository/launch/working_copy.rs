//! The settings New cloud and a rebuild read while the checked-out commit is the one they
//! use. For New cloud, where a worker runs and how big it is come from the working copy of
//! `.horizon/cloud.yml`, so an edit applies without a commit. What is built and transferred
//! comes from the commit: New cloud builds the image from the committed tree, and the
//! source is a committed archive. A rebuild builds from the working copy, so it reads the
//! working copy's file whole ([`for_rebuild`]).
use super::{CloudConfig, Error, Path, Runner};
use horizon_cloud::{Cancellation, Profile};
use std::{
    collections::BTreeMap,
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
const MISMATCH: &str =
    "The working copy's changes do not fit the committed build settings, so the committed settings apply.";
const NONE_OFFERED: &str =
    "No profile in the working copy can start before you commit it, so the committed settings apply.";

enum WorkingFile {
    Missing,
    Invalid,
    Valid(CloudConfig),
}

/// The settings New cloud offers for the checked-out commit, whose own file is `committed`,
/// with the notes that say which working-copy edits do not apply.
pub(super) fn apply(
    repository: &Path,
    committed: Option<CloudConfig>,
    runner: &Runner<'_>,
) -> super::super::Result<(CloudConfig, Vec<String>)> {
    runner.cancel.check()?;
    let file = read(repository);
    runner.cancel.check()?;
    match (file, committed) {
        (WorkingFile::Valid(working), committed) => merge(committed.as_ref(), working),
        (WorkingFile::Missing, Some(committed)) => Ok((committed, vec![MISSING.into()])),
        (WorkingFile::Invalid, Some(committed)) => Ok((committed, vec![INVALID.into()])),
        (WorkingFile::Missing, None) => Err(Error::Invalid(super::MISSING_CONFIG)),
        (WorkingFile::Invalid, None) => Err(Error::Invalid(super::INVALID_CONFIG)),
    }
}

/// The settings a rebuild reads at the checked-out commit, whose own file is `committed`.
/// The image builds from the working copy too ([`super::super::uncommitted`]), so the
/// working copy's file applies whole, with the commit's source and companions, which
/// name what comes from commits. The commit's file applies when the working copy has no
/// valid one; a note says so.
pub(in crate::cloud_runtime) fn for_rebuild(
    repository: &Path,
    committed: Option<CloudConfig>,
    runner: &Runner<'_>,
) -> super::super::Result<(Option<CloudConfig>, Vec<String>)> {
    runner.cancel.check()?;
    let file = read(repository);
    runner.cancel.check()?;
    Ok(match (file, committed) {
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
            (Some(config), notes)
        }
        (WorkingFile::Missing, Some(committed)) => (Some(committed), vec![MISSING.into()]),
        (WorkingFile::Invalid, Some(committed)) => (Some(committed), vec![INVALID.into()]),
        (_, None) => (None, Vec::new()),
    })
}

fn read(repository: &Path) -> WorkingFile {
    match contents(repository) {
        None => WorkingFile::Missing,
        Some(Ok(yaml)) => CloudConfig::parse(&yaml).map_or(WorkingFile::Invalid, WorkingFile::Valid),
        Some(Err(())) => WorkingFile::Invalid,
    }
}

/// `working` with the commit's build, image and capabilities for each profile that builds
/// in either file, and the commit's source and companions. A profile only in the working
/// copy is offered when it runs an existing image.
fn merge(committed: Option<&CloudConfig>, working: CloudConfig) -> super::super::Result<(CloudConfig, Vec<String>)> {
    let empty = BTreeMap::new();
    let committed_profiles = committed.map_or(&empty, |config| &config.profiles);
    let mut ignored = Vec::new();
    let mut unoffered = Vec::new();
    let mut profiles = BTreeMap::new();
    for (name, profile) in working.profiles {
        match committed_profiles.get(&name) {
            // The recipe's build arguments carry the capabilities, so they are part of
            // what is built.
            Some(built) if built.build.is_some() || profile.build.is_some() => {
                for (field, changed) in [
                    ("build", built.build != profile.build),
                    ("image", built.image != profile.image),
                    ("capabilities", built.capabilities != profile.capabilities),
                ] {
                    note_field(&mut ignored, field, changed);
                }
                let merged = Profile {
                    build: built.build.clone(),
                    image: built.image.clone(),
                    capabilities: built.capabilities.clone(),
                    ..profile
                };
                profiles.insert(name, merged);
            }
            None if profile.build.is_some() => unoffered.push(name),
            _ => {
                profiles.insert(name, profile);
            }
        }
    }
    let (source, companions) = committed
        .map(|config| (config.source.clone(), config.companions.clone()))
        .unwrap_or_default();
    note_field(&mut ignored, "source", source != working.source);
    note_field(&mut ignored, "companions", companions != working.companions);
    let mut notes = Vec::new();
    if !ignored.is_empty() {
        notes.push(format!(
            "Uncommitted changes to {} are not used until you commit them.",
            list(&ignored)
        ));
    }
    if !unoffered.is_empty() {
        notes.push(unoffered_note(&unoffered));
    }
    let fallback = |mut notes: Vec<String>, note: &str| match committed.cloned() {
        Some(committed) => {
            notes.push(note.into());
            Ok((committed, notes))
        }
        None => Err(Error::Invalid(super::MISSING_CONFIG)),
    };
    if profiles.is_empty() {
        return fallback(notes, NONE_OFFERED);
    }
    // A committed recipe can ask for what the working copy's provider cannot give, such
    // as hosted devices on Hetzner.
    if profiles.values().any(|profile| profile.validate(false).is_err()) {
        return fallback(Vec::new(), MISMATCH);
    }
    let default = [Some(&working.default), committed.map(|config| &config.default)]
        .into_iter()
        .flatten()
        .find(|name| profiles.contains_key(*name))
        .or_else(|| profiles.keys().next())
        .cloned()
        .unwrap_or_default();
    let config = CloudConfig {
        version: working.version,
        default,
        profiles,
        companions,
        source,
        placement: working.placement,
    };
    Ok((config, notes))
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

fn unoffered_note(names: &[String]) -> String {
    if let [name] = names {
        format!("Profile {name} builds an image, so New cloud offers it after you commit it.")
    } else {
        format!(
            "Profiles {} build images, so New cloud offers them after you commit them.",
            list(names)
        )
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
