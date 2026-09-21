//! Host-path policy for browser file attachments.
//!
//! A `set_files` action makes the browser read files from this host, so the
//! paths an agent may name are confined to explicit roots: the agent's work
//! root (Horizon exports it as `HORIZON_WORK_ROOT`), the process working
//! directory when no work root is set, and any extra roots listed in
//! `HORIZON_BROWSER_ATTACHMENT_ROOTS`. Every path is resolved through its
//! symlinks before the root check, so a link inside a root cannot reach out
//! of it. Authorization alone does not bind the browser's later read to the
//! checked file (the pathname could be re-pointed in between), so the queue
//! stages a private copy of each authorized file and hands the engine those
//! copies. A page reads an attached `File` lazily, often only when the form
//! is submitted, so the copies stay for the panel's lifetime: each panel
//! keeps its attachment actions for a bounded time, count and size, pruned
//! when the next attachment is staged, and a queue failure releases its own
//! staging right away.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest single file `set_files` stages.
pub const MAX_ATTACHMENT_BYTES: u64 = 512 * 1024 * 1024;
/// How long a panel's staged attachments stay readable for its page.
pub const ATTACHMENT_RETENTION: Duration = Duration::from_hours(24);
/// Most attachment actions one panel keeps staged at a time.
pub const MAX_RETAINED_ATTACHMENT_ACTIONS: usize = 32;
/// Most staged bytes one panel keeps at a time.
pub const MAX_RETAINED_ATTACHMENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Marker inside an action's staging directory from staging until the
/// action's result is consumed: queued, dispatched and in-flight actions
/// all carry it, and pruning never evicts a marked action within retention.
const PENDING_MARKER: &str = ".pending";
/// Stamp written at staging and never touched again: its modification time
/// is the action's age, unaffected by the marker being cleared later.
const STAGED_STAMP: &str = ".staged";

/// Horizon's work-root variable; kept in sync with `horizon_core::agent_work::WORK_ROOT_ENV`.
pub const WORK_ROOT_ENV: &str = "HORIZON_WORK_ROOT";
/// Extra attachment roots, separated like `PATH`.
pub const ATTACHMENT_ROOTS_ENV: &str = "HORIZON_BROWSER_ATTACHMENT_ROOTS";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AttachmentPolicyError {
    #[error("no attachment root is available: set {WORK_ROOT_ENV} or {ATTACHMENT_ROOTS_ENV}")]
    NoRoots,
    #[error("attachment path cannot be resolved ({reason}): {path}")]
    Unresolvable { path: String, reason: String },
    #[error("attachment path is not a regular file: {path}")]
    NotAFile { path: String },
    #[error("attachment path is outside the allowed roots [{roots}]: {path}")]
    OutsideRoots { path: String, roots: String },
    #[error("attachment exceeds {limit} bytes: {path}")]
    TooLarge { path: String, limit: u64 },
    #[error("attachments total {requested} bytes, above the {limit} byte budget one panel may keep staged")]
    OverBudget { requested: u64, limit: u64 },
    #[error(
        "the panel's staged attachments for still-queued actions leave no room within {limit_actions} actions and {limit_bytes} bytes; let the queue drain first"
    )]
    StagingFull { limit_actions: usize, limit_bytes: u64 },
    #[error("attachment could not be staged ({reason}): {path}")]
    Staging { path: String, reason: String },
}

/// The resolved roots an attachment path must fall under.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachmentPolicy {
    roots: Vec<PathBuf>,
}

/// One attachment that passed the root check, held open so the staged copy
/// is read from this very handle and the pathname can no longer matter.
#[derive(Debug)]
pub struct AuthorizedFile {
    path: PathBuf,
    file: std::fs::File,
    size: u64,
}

impl AuthorizedFile {
    /// Where the open handle resolved to when it was checked.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
}

impl AttachmentPolicyError {
    /// The I/O classification a queue reports for this refusal: policy
    /// refusals are permission errors, malformed requests are invalid
    /// input, an oversized file is a size error, and a staging failure is
    /// an internal error.
    #[must_use]
    pub fn io_kind(&self) -> std::io::ErrorKind {
        match self {
            Self::NoRoots | Self::OutsideRoots { .. } => std::io::ErrorKind::PermissionDenied,
            Self::Unresolvable { .. } | Self::NotAFile { .. } => std::io::ErrorKind::InvalidInput,
            Self::TooLarge { .. } | Self::OverBudget { .. } => std::io::ErrorKind::FileTooLarge,
            Self::StagingFull { .. } => std::io::ErrorKind::WouldBlock,
            Self::Staging { .. } => std::io::ErrorKind::Other,
        }
    }
}

impl AttachmentPolicy {
    /// Roots that resolve on this host; roots that do not exist are dropped.
    #[must_use]
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            roots: roots
                .into_iter()
                .filter(|root| !root.as_os_str().is_empty())
                .filter_map(|root| std::fs::canonicalize(root).ok())
                .collect(),
        }
    }

    /// The work root (or the working directory without one) plus the extra
    /// roots from the environment.
    #[must_use]
    pub fn from_environment() -> Self {
        let work_root = std::env::var_os(WORK_ROOT_ENV)
            .filter(|root| !root.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok());
        let extra = std::env::var_os(ATTACHMENT_ROOTS_ENV)
            .map(|roots| std::env::split_paths(&roots).collect::<Vec<_>>())
            .unwrap_or_default();
        Self::new(work_root.into_iter().chain(extra))
    }

    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Open every path and confirm that the opened handle is a regular file
    /// within the size limit whose current location lies under a root. The
    /// root check is made on the handle's own location, not on the
    /// pathname that was opened, so a path re-pointed between the two steps
    /// is caught. Returns the open files in request order; the first
    /// refusal ends the check, so nothing is authorized when any path is
    /// refused.
    ///
    /// # Errors
    /// Returns which path was refused and why, never file contents.
    pub fn authorize(&self, paths: &[PathBuf]) -> Result<Vec<AuthorizedFile>, AttachmentPolicyError> {
        if self.roots.is_empty() {
            return Err(AttachmentPolicyError::NoRoots);
        }
        paths.iter().map(|path| self.authorize_one(path)).collect()
    }

    fn authorize_one(&self, path: &Path) -> Result<AuthorizedFile, AttachmentPolicyError> {
        let display = path.display().to_string();
        let unresolvable = |error: std::io::Error| AttachmentPolicyError::Unresolvable {
            path: display.clone(),
            reason: error.kind().to_string(),
        };
        // Refuse anything but a regular file before opening it, then open
        // without blocking so a FIFO swapped in between cannot hang the
        // caller waiting for a writer; the opened handle is checked again.
        if !std::fs::metadata(path).map_err(unresolvable)?.is_file() {
            return Err(AttachmentPolicyError::NotAFile { path: display });
        }
        let file = open_without_blocking(path).map_err(unresolvable)?;
        let metadata = file.metadata().map_err(unresolvable)?;
        if !metadata.is_file() {
            return Err(AttachmentPolicyError::NotAFile { path: display });
        }
        if metadata.len() > MAX_ATTACHMENT_BYTES {
            return Err(AttachmentPolicyError::TooLarge {
                path: display,
                limit: MAX_ATTACHMENT_BYTES,
            });
        }
        let location = handle_location(&file, path).map_err(unresolvable)?;
        if self.roots.iter().any(|root| location.starts_with(root)) {
            Ok(AuthorizedFile {
                path: location,
                file,
                size: metadata.len(),
            })
        } else {
            Err(AttachmentPolicyError::OutsideRoots {
                path: display,
                roots: self
                    .roots
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            })
        }
    }
}

/// Open for reading without waiting on the target: on Unix `O_NONBLOCK`
/// makes a FIFO open return at once instead of blocking for a writer, and
/// it has no effect on reading a regular file.
#[cfg(unix)]
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_without_blocking(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// The resolved location of an open file. Linux reads it from the handle
/// itself through `/proc`; elsewhere the pathname is resolved again and
/// accepted only when the file it names now is the very file the handle
/// holds (device and inode on Unix, volume and file index on Windows), so
/// a pathname re-pointed between the two steps is refused.
#[cfg(target_os = "linux")]
fn handle_location(file: &std::fs::File, _requested: &Path) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(not(target_os = "linux"))]
fn handle_location(file: &std::fs::File, requested: &Path) -> std::io::Result<PathBuf> {
    let resolved = std::fs::canonicalize(requested)?;
    // The resolved name is reopened without blocking and checked to be a
    // regular file too, so a FIFO swapped in after resolution cannot hang
    // this second open either.
    let reopened = open_without_blocking(&resolved)?;
    if !reopened.metadata()?.is_file() {
        return Err(std::io::Error::other(
            "the attachment changed while it was being checked",
        ));
    }
    let opened = same_file::Handle::from_file(file.try_clone()?)?;
    let named = same_file::Handle::from_file(reopened)?;
    if opened == named {
        Ok(resolved)
    } else {
        Err(std::io::Error::other(
            "the attachment changed while it was being checked",
        ))
    }
}

/// Copy each authorized file into
/// `<attachments>/<panel>/<action>/<index>/<name>` from its open handle, so
/// the bytes the browser reads are the bytes that were checked. Returns the
/// staged paths in request order; any failure removes the action's staging
/// directory.
///
/// # Errors
/// Returns the first file that could not be copied.
pub fn stage_attachments(
    attachments_dir: &Path,
    panel_local_id: &str,
    action_id: &str,
    files: &[AuthorizedFile],
) -> Result<Vec<PathBuf>, AttachmentPolicyError> {
    let directory = action_directory(attachments_dir, panel_local_id, action_id);
    let staged = stage_into(&directory, files);
    if staged.is_err() {
        release_attachments(attachments_dir, panel_local_id, action_id);
    }
    staged
}

fn action_directory(attachments_dir: &Path, panel_local_id: &str, action_id: &str) -> PathBuf {
    attachments_dir
        .join(crate::paths::safe_local_id(panel_local_id))
        .join(crate::paths::safe_local_id(action_id))
}

fn stage_into(directory: &Path, files: &[AuthorizedFile]) -> Result<Vec<PathBuf>, AttachmentPolicyError> {
    create_private_dir(directory)?;
    for marker in [STAGED_STAMP, PENDING_MARKER] {
        std::fs::write(directory.join(marker), b"").map_err(|error| AttachmentPolicyError::Staging {
            path: directory.display().to_string(),
            reason: error.kind().to_string(),
        })?;
    }
    files
        .iter()
        .enumerate()
        .map(|(index, authorized)| {
            let display = authorized.path.display().to_string();
            let staging = |error: std::io::Error| AttachmentPolicyError::Staging {
                path: display.clone(),
                reason: error.kind().to_string(),
            };
            let name = authorized
                .path
                .file_name()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| AttachmentPolicyError::NotAFile { path: display.clone() })?;
            let slot = directory.join(index.to_string());
            create_private_dir(&slot)?;
            let target = slot.join(name);
            let mut copy = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(staging)?;
            // The copy starts at the file's beginning whatever was read from
            // the handle before, and is bounded by the size that was checked
            // and reserved in the panel budget: a file that grew since is
            // refused rather than staged past its reservation, and one that
            // shrank is refused as changed.
            std::io::Seek::seek(&mut &authorized.file, std::io::SeekFrom::Start(0)).map_err(staging)?;
            let mut source = std::io::Read::take(&authorized.file, authorized.size + 1);
            let copied = std::io::copy(&mut source, &mut copy).map_err(staging)?;
            if copied > authorized.size {
                return Err(AttachmentPolicyError::TooLarge {
                    path: display,
                    limit: authorized.size,
                });
            }
            if copied < authorized.size {
                return Err(AttachmentPolicyError::Staging {
                    path: display,
                    reason: "the file changed while it was being staged".to_string(),
                });
            }
            copy.flush().map_err(staging)?;
            Ok(target)
        })
        .collect()
}

fn create_private_dir(directory: &Path) -> Result<(), AttachmentPolicyError> {
    let staging = |error: std::io::Error| AttachmentPolicyError::Staging {
        path: directory.display().to_string(),
        reason: error.kind().to_string(),
    };
    std::fs::create_dir_all(directory).map_err(staging)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).map_err(staging)?;
    }
    Ok(())
}

/// The action's result was consumed: its staging is no longer pending and
/// may be pruned by retention like any other; a missing marker is fine.
pub fn settle_attachments(attachments_dir: &Path, panel_local_id: &str, action_id: &str) {
    let marker = action_directory(attachments_dir, panel_local_id, action_id).join(PENDING_MARKER);
    if let Err(error) = std::fs::remove_file(&marker)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(target: "browser", path = %marker.display(), "failed to settle staged attachments: {error}");
    }
}

/// Remove the staged copies of one action; a missing directory is fine.
pub fn release_attachments(attachments_dir: &Path, panel_local_id: &str, action_id: &str) {
    let directory = action_directory(attachments_dir, panel_local_id, action_id);
    if let Err(error) = std::fs::remove_dir_all(&directory)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(target: "browser", path = %directory.display(), "failed to remove staged attachments: {error}");
    }
}

/// The aggregate size of one request, refused when it alone would exceed
/// what a panel may keep staged.
///
/// # Errors
/// Returns `OverBudget` with the requested total and the limit.
pub fn check_panel_budget(files: &[AuthorizedFile], max_bytes: u64) -> Result<u64, AttachmentPolicyError> {
    let requested = files.iter().fold(0u64, |total, file| total.saturating_add(file.size));
    if requested > max_bytes {
        return Err(AttachmentPolicyError::OverBudget {
            requested,
            limit: max_bytes,
        });
    }
    Ok(requested)
}

/// Make room for one more attachment action on `panel_local_id`: every
/// panel's settled actions older than `retention` go (a closed panel's
/// staging ages out this way), and the panel keeps at most
/// `max_actions - 1` newer actions within `max_bytes` less the
/// `reserved_bytes` the incoming action needs, so the new one fits inside
/// the budget. Actions still pending (queued, dispatched or in flight,
/// marked until their result is consumed) are never evicted within
/// retention; when they alone leave no room, or a required eviction fails,
/// the new action is refused rather than staged over the limit. A stale
/// eviction that fails on this panel stays counted so it cannot admit a
/// newcomer past the limits.
///
/// # Errors
/// Returns `StagingFull` when the room cannot be made.
pub fn prune_attachments(
    attachments_dir: &Path,
    panel_local_id: &str,
    retention: Duration,
    max_actions: usize,
    max_bytes: u64,
    reserved_bytes: u64,
) -> Result<(), AttachmentPolicyError> {
    let stale_before = std::time::SystemTime::now()
        .checked_sub(retention)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let scan_failure = |path: &Path, error: std::io::Error| AttachmentPolicyError::Staging {
        path: path.display().to_string(),
        reason: format!("could not scan staged attachments: {}", error.kind()),
    };
    let panels = match std::fs::read_dir(attachments_dir) {
        Ok(panels) => panels,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(scan_failure(attachments_dir, error)),
    };
    let current = crate::paths::safe_local_id(panel_local_id);
    for panel in panels {
        // The current panel's staging must be fully accounted for, so any
        // scan error there refuses the newcomer; other panels are cleaned
        // on a best-effort basis.
        let panel = panel.map_err(|error| scan_failure(attachments_dir, error))?;
        let is_current = panel.file_name().to_string_lossy() == current;
        let actions = match std::fs::read_dir(panel.path()) {
            Ok(actions) => actions,
            Err(error) if is_current => return Err(scan_failure(&panel.path(), error)),
            Err(_) => continue,
        };
        let mut retained = Vec::new();
        for action in actions {
            let action = match action {
                Ok(action) => action,
                Err(error) if is_current => return Err(scan_failure(&panel.path(), error)),
                Err(_) => continue,
            };
            let modified = match std::fs::metadata(action.path().join(STAGED_STAMP))
                .or_else(|_| action.metadata())
                .and_then(|metadata| metadata.modified())
            {
                Ok(modified) => modified,
                Err(error) if is_current => return Err(scan_failure(&action.path(), error)),
                Err(_) => continue,
            };
            let pending = action.path().join(PENDING_MARKER).exists();
            if modified < stale_before {
                // Only age reaches other panels: their fresh copies may
                // still be read lazily by their own pages.
                if std::fs::remove_dir_all(action.path()).is_err() && is_current {
                    // Still on disk, so still counted below.
                    let bytes = directory_bytes(&action.path()).map_err(|error| scan_failure(&action.path(), error))?;
                    retained.push((modified, bytes, action.path(), false));
                }
            } else if is_current {
                let bytes = directory_bytes(&action.path()).map_err(|error| scan_failure(&action.path(), error))?;
                retained.push((modified, bytes, action.path(), pending));
            }
        }
        if !is_current {
            continue;
        }
        // Pending actions are kept whatever their size; the newest settled
        // ones then fill what room the budget leaves for them.
        let budget = max_bytes.saturating_sub(reserved_bytes);
        let (pending_actions, settled): (Vec<_>, Vec<_>) = retained.into_iter().partition(|entry| entry.3);
        let mut kept_actions = pending_actions.len();
        let mut kept_bytes = pending_actions
            .iter()
            .fold(0u64, |total, entry| total.saturating_add(entry.1));
        if kept_actions + 1 > max_actions || kept_bytes > budget {
            return Err(AttachmentPolicyError::StagingFull {
                limit_actions: max_actions,
                limit_bytes: max_bytes,
            });
        }
        let mut settled = settled;
        settled.sort_by_key(|(modified, _, _, _)| std::cmp::Reverse(*modified));
        for (_, bytes, path, _) in settled {
            if kept_actions + 1 < max_actions && kept_bytes.saturating_add(bytes) <= budget {
                kept_actions += 1;
                kept_bytes = kept_bytes.saturating_add(bytes);
            } else {
                std::fs::remove_dir_all(&path).map_err(|error| AttachmentPolicyError::Staging {
                    path: path.display().to_string(),
                    reason: format!("could not evict older staged attachments: {}", error.kind()),
                })?;
            }
        }
    }
    Ok(())
}

fn directory_bytes(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_copies_the_checked_bytes_and_release_removes_them() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("claim.pdf");
        std::fs::write(&source, b"%PDF").expect("write");
        let attachments = root.path().join("attachments");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let authorized = policy.authorize(&[source.clone(), source.clone()]).expect("authorized");
        let staged = stage_attachments(&attachments, "panel", "action-1", &authorized).expect("staged");
        assert_eq!(staged.len(), 2);
        assert_ne!(staged[0], staged[1], "each slot keeps its own copy of a repeated file");
        assert!(
            staged
                .iter()
                .all(|path| path.starts_with(&attachments) && path.ends_with("claim.pdf"))
        );
        assert_eq!(std::fs::read(&staged[0]).expect("read"), b"%PDF");
        // The staged copies are independent of the source from here on.
        std::fs::write(&source, b"swapped").expect("rewrite");
        assert_eq!(std::fs::read(&staged[1]).expect("read"), b"%PDF");
        let directory = action_directory(&attachments, "panel", "action-1");
        assert!(directory.is_dir());
        release_attachments(&attachments, "panel", "action-1");
        assert!(!directory.exists());
        release_attachments(&attachments, "panel", "action-1");
    }

    #[test]
    fn a_failed_staging_leaves_nothing_behind() {
        let root = tempfile::tempdir().expect("root");
        let good = root.path().join("ok.txt");
        std::fs::write(&good, b"ok").expect("write");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let authorized = policy.authorize(std::slice::from_ref(&good)).expect("authorized");
        let attachments = root.path().join("attachments");
        std::fs::write(&attachments, b"not a directory").expect("block the staging root");
        let error = stage_attachments(&attachments, "panel", "action-2", &authorized).expect_err("cannot stage");
        assert!(matches!(error, AttachmentPolicyError::Staging { .. }), "{error}");
        assert!(!attachments.is_dir(), "nothing was created in place of the blocker");
    }

    #[cfg(unix)]
    #[test]
    fn authorization_judges_the_opened_handle_not_the_pathname() {
        let root = tempfile::tempdir().expect("root");
        let inside = root.path().join("inside.txt");
        std::fs::write(&inside, b"inside").expect("write");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let authorized = policy.authorize(std::slice::from_ref(&inside)).expect("authorized");
        // Re-pointing the pathname after authorization changes nothing:
        // the staged bytes come from the handle that was checked.
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"secret").expect("write");
        std::fs::remove_file(&inside).expect("remove");
        std::os::unix::fs::symlink(&secret, &inside).expect("re-point");
        let staged = stage_attachments(&root.path().join("attachments"), "panel", "swap", &authorized).expect("staged");
        assert_eq!(std::fs::read(&staged[0]).expect("read"), b"inside");
    }

    #[test]
    fn pruning_ages_out_every_panel_and_bounds_the_current_one() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("a.txt");
        std::fs::write(&source, b"abcdef").expect("write");
        let attachments = root.path().join("attachments");
        let authorized = AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(std::slice::from_ref(&source))
            .expect("authorized");
        stage_attachments(&attachments, "other", "old", &authorized).expect("old on another panel");
        stage_attachments(&attachments, "panel", "first", &authorized).expect("first");
        std::thread::sleep(Duration::from_millis(600));
        stage_attachments(&attachments, "panel", "second", &authorized).expect("second");
        stage_attachments(&attachments, "panel", "third", &authorized).expect("third");
        for (panel, action) in [
            ("other", "old"),
            ("panel", "first"),
            ("panel", "second"),
            ("panel", "third"),
        ] {
            settle_attachments(&attachments, panel, action);
        }
        // Age: the other panel's old action and this panel's first go.
        prune_attachments(&attachments, "panel", Duration::from_millis(300), 8, u64::MAX, 0).expect("room");
        assert!(!action_directory(&attachments, "other", "old").exists());
        assert!(!action_directory(&attachments, "panel", "first").exists());
        assert!(action_directory(&attachments, "panel", "second").exists());
        assert!(action_directory(&attachments, "panel", "third").exists());
        // Count: room for one more means only the newest of the two stays.
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 2, u64::MAX, 0).expect("room");
        assert_eq!(
            std::fs::read_dir(attachments.join(crate::paths::safe_local_id("panel")))
                .expect("panel dir")
                .count(),
            1
        );
        // Bytes: a 12-byte budget with 7 reserved for the newcomer leaves
        // less than one 6-byte action, so the panel is cleared.
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 8, 12, 7).expect("room");
        assert_eq!(
            std::fs::read_dir(attachments.join(crate::paths::safe_local_id("panel")))
                .expect("panel dir")
                .count(),
            0
        );
    }

    #[test]
    fn files_under_a_root_are_authorized_as_resolved_paths() {
        let root = tempfile::tempdir().expect("root");
        let nested = root.path().join("uploads");
        std::fs::create_dir(&nested).expect("mkdir");
        let file = nested.join("doc.pdf");
        std::fs::write(&file, b"%PDF").expect("write");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let unresolved = nested.join("..").join("uploads").join("doc.pdf");
        let authorized = policy.authorize(&[unresolved]).expect("inside root");
        assert_eq!(
            authorized.iter().map(AuthorizedFile::path).collect::<Vec<_>>(),
            vec![std::fs::canonicalize(&file).expect("canonical")],
            "the handle's resolved location is what gets staged"
        );
        assert_eq!(authorized[0].size(), 4);
    }

    #[test]
    fn paths_outside_every_root_are_refused_without_partial_authorization() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let inside = root.path().join("ok.txt");
        std::fs::write(&inside, b"ok").expect("write");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"secret").expect("write");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let error = policy.authorize(&[inside, secret.clone()]).expect_err("outside root");
        assert!(matches!(error, AttachmentPolicyError::OutsideRoots { .. }), "{error}");
        assert!(error.to_string().contains("secret.txt"));
        assert!(!error.to_string().contains("secret\""));
        assert!(matches!(
            policy.authorize(&[root.path().to_path_buf()]),
            Err(AttachmentPolicyError::NotAFile { .. })
        ));
        assert!(matches!(
            policy.authorize(&[root.path().join("missing.txt")]),
            Err(AttachmentPolicyError::Unresolvable { .. })
        ));
        assert_eq!(
            AttachmentPolicyError::NoRoots.io_kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            AttachmentPolicyError::TooLarge {
                path: String::new(),
                limit: 1
            }
            .io_kind(),
            std::io::ErrorKind::FileTooLarge
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_judged_by_their_resolved_location() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"secret").expect("write");
        let link = root.path().join("looks-inside.txt");
        std::os::unix::fs::symlink(&secret, &link).expect("symlink");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        assert!(matches!(
            policy.authorize(&[link]),
            Err(AttachmentPolicyError::OutsideRoots { .. })
        ));
    }

    #[test]
    fn pending_actions_are_never_evicted_for_room_and_refuse_the_newcomer_instead() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("a.txt");
        std::fs::write(&source, b"abcdef").expect("write");
        let attachments = root.path().join("attachments");
        let authorized = AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(std::slice::from_ref(&source))
            .expect("authorized");
        stage_attachments(&attachments, "panel", "older", &authorized).expect("older");
        std::thread::sleep(Duration::from_millis(600));
        stage_attachments(&attachments, "panel", "pending", &authorized).expect("pending");
        settle_attachments(&attachments, "panel", "older");
        // Age only removes settled history; the fresh pending action stays.
        prune_attachments(&attachments, "panel", Duration::from_millis(300), 8, u64::MAX, 0).expect("room");
        assert!(!action_directory(&attachments, "panel", "older").exists());
        assert!(action_directory(&attachments, "panel", "pending").exists());
        stage_attachments(&attachments, "panel", "settled", &authorized).expect("settled");
        settle_attachments(&attachments, "panel", "settled");
        // Count: the pending action stays and the settled newer one goes.
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 2, u64::MAX, 0).expect("room");
        assert!(action_directory(&attachments, "panel", "pending").exists());
        assert!(!action_directory(&attachments, "panel", "settled").exists());
        // Bytes: the pending action alone exceeds the room, so the newcomer is refused.
        let error = prune_attachments(&attachments, "panel", Duration::from_hours(1), 8, 12, 7).expect_err("no room");
        assert!(matches!(error, AttachmentPolicyError::StagingFull { .. }), "{error}");
        assert_eq!(error.io_kind(), std::io::ErrorKind::WouldBlock);
        assert!(action_directory(&attachments, "panel", "pending").exists());
        // Settled, it is ordinary retained history again.
        settle_attachments(&attachments, "panel", "pending");
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 8, 12, 7).expect("room");
        assert!(!action_directory(&attachments, "panel", "pending").exists());
    }

    #[test]
    fn other_panels_lose_only_stale_staging() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("a.txt");
        std::fs::write(&source, b"abcdef").expect("write");
        let attachments = root.path().join("attachments");
        let authorized = AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(std::slice::from_ref(&source))
            .expect("authorized");
        stage_attachments(&attachments, "other", "fresh", &authorized).expect("fresh");
        settle_attachments(&attachments, "other", "fresh");
        for action in ["one", "two", "three"] {
            stage_attachments(&attachments, "panel", action, &authorized).expect(action);
            settle_attachments(&attachments, "panel", action);
        }
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 2, 6, 0).expect("room");
        assert!(
            action_directory(&attachments, "other", "fresh").exists(),
            "count and byte limits apply to the current panel only"
        );
        assert_eq!(
            std::fs::read_dir(attachments.join(crate::paths::safe_local_id("panel")))
                .expect("panel dir")
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let root = tempfile::tempdir().expect("root");
        let fifo = root.path().join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|status| status.success());
        if !made {
            return;
        }
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let started = std::time::Instant::now();
        assert!(matches!(
            policy.authorize(std::slice::from_ref(&fifo)),
            Err(AttachmentPolicyError::NotAFile { .. })
        ));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the FIFO open must not wait for a writer"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_current_panel_refuses_the_newcomer() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("a.txt");
        std::fs::write(&source, b"abcdef").expect("write");
        let attachments = root.path().join("attachments");
        let authorized = AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(std::slice::from_ref(&source))
            .expect("authorized");
        stage_attachments(&attachments, "panel", "hidden", &authorized).expect("hidden");
        let panel_dir = attachments.join(crate::paths::safe_local_id("panel"));
        std::fs::set_permissions(&panel_dir, std::fs::Permissions::from_mode(0o300)).expect("unreadable");
        let refused = prune_attachments(&attachments, "panel", Duration::from_hours(1), 8, u64::MAX, 0);
        std::fs::set_permissions(&panel_dir, std::fs::Permissions::from_mode(0o700)).expect("restore");
        assert!(
            matches!(refused, Err(AttachmentPolicyError::Staging { .. })),
            "{refused:?}"
        );
        prune_attachments(&attachments, "other", Duration::from_hours(1), 8, u64::MAX, 0)
            .expect("an unreadable other panel is skipped");
    }

    #[test]
    fn a_pending_action_past_retention_is_aged_out_as_crash_safety() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("a.txt");
        std::fs::write(&source, b"abcdef").expect("write");
        let attachments = root.path().join("attachments");
        let authorized = AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(std::slice::from_ref(&source))
            .expect("authorized");
        stage_attachments(&attachments, "panel", "abandoned", &authorized).expect("abandoned");
        std::thread::sleep(Duration::from_millis(600));
        prune_attachments(&attachments, "panel", Duration::from_millis(300), 8, u64::MAX, 0).expect("room");
        assert!(!action_directory(&attachments, "panel", "abandoned").exists());
    }

    #[test]
    fn a_copy_is_bounded_by_the_reserved_size() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("grow.txt");
        std::fs::write(&source, b"small").expect("write");
        let policy = AttachmentPolicy::new([root.path().to_path_buf()]);
        let grown = policy.authorize(std::slice::from_ref(&source)).expect("authorized");
        std::fs::write(&source, b"grown past the reservation").expect("grow");
        let error = stage_attachments(&root.path().join("attachments"), "panel", "grow", &grown).expect_err("grew");
        assert!(
            matches!(error, AttachmentPolicyError::TooLarge { limit: 5, .. }),
            "{error}"
        );

        let shrunk = policy.authorize(std::slice::from_ref(&source)).expect("authorized");
        std::fs::write(&source, b"x").expect("shrink");
        let error =
            stage_attachments(&root.path().join("attachments"), "panel", "shrink", &shrunk).expect_err("shrank");
        assert!(matches!(error, AttachmentPolicyError::Staging { .. }), "{error}");
        for action in ["grow", "shrink"] {
            assert!(!action_directory(&root.path().join("attachments"), "panel", action).exists());
        }
    }

    #[test]
    fn a_request_above_the_panel_budget_is_refused_before_staging() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("a.txt");
        std::fs::write(&source, b"abcdef").expect("write");
        let authorized = AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(&[source.clone(), source])
            .expect("authorized");
        assert_eq!(check_panel_budget(&authorized, 12).expect("within budget"), 12);
        let error = check_panel_budget(&authorized, 11).expect_err("over budget");
        assert!(matches!(
            error,
            AttachmentPolicyError::OverBudget {
                requested: 12,
                limit: 11
            }
        ));
        assert_eq!(error.io_kind(), std::io::ErrorKind::FileTooLarge);
    }

    #[test]
    fn missing_roots_are_dropped_and_no_roots_refuses_everything() {
        let root = tempfile::tempdir().expect("root");
        let policy = AttachmentPolicy::new([root.path().join("absent"), PathBuf::new()]);
        assert!(policy.roots().is_empty());
        assert!(matches!(
            policy.authorize(&[root.path().join("any.txt")]),
            Err(AttachmentPolicyError::NoRoots)
        ));
    }
}
