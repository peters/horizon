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
            Self::TooLarge { .. } => std::io::ErrorKind::FileTooLarge,
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
        let file = std::fs::File::open(path).map_err(unresolvable)?;
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
    let opened = same_file::Handle::from_file(file.try_clone()?)?;
    let named = same_file::Handle::from_path(&resolved)?;
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
            // the handle before. The size was checked on metadata; a file
            // that grows afterwards is cut off at the limit and refused
            // rather than staged whole.
            std::io::Seek::seek(&mut &authorized.file, std::io::SeekFrom::Start(0)).map_err(staging)?;
            let mut source = std::io::Read::take(&authorized.file, MAX_ATTACHMENT_BYTES + 1);
            let copied = std::io::copy(&mut source, &mut copy).map_err(staging)?;
            if copied > MAX_ATTACHMENT_BYTES {
                return Err(AttachmentPolicyError::TooLarge {
                    path: display,
                    limit: MAX_ATTACHMENT_BYTES,
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

/// Remove the staged copies of one action; a missing directory is fine.
pub fn release_attachments(attachments_dir: &Path, panel_local_id: &str, action_id: &str) {
    let directory = action_directory(attachments_dir, panel_local_id, action_id);
    if let Err(error) = std::fs::remove_dir_all(&directory)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(target: "browser", path = %directory.display(), "failed to remove staged attachments: {error}");
    }
}

/// Make room for one more attachment action on `panel_local_id`: every
/// panel's actions older than `retention` go (a closed panel's staging
/// ages out this way), and the panel keeps at most `max_actions - 1`
/// newer actions within `max_bytes` so the new one fits.
pub fn prune_attachments(
    attachments_dir: &Path,
    panel_local_id: &str,
    retention: Duration,
    max_actions: usize,
    max_bytes: u64,
) {
    let stale_before = std::time::SystemTime::now()
        .checked_sub(retention)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let Ok(panels) = std::fs::read_dir(attachments_dir) else {
        return;
    };
    let current = crate::paths::safe_local_id(panel_local_id);
    for panel in panels.flatten() {
        let Ok(actions) = std::fs::read_dir(panel.path()) else {
            continue;
        };
        let mut retained = Vec::new();
        for action in actions.flatten() {
            let Ok(modified) = action.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            if modified < stale_before {
                remove_action_dir(&action.path());
            } else if panel.file_name().to_string_lossy() == current {
                retained.push((modified, directory_bytes(&action.path()), action.path()));
            }
        }
        retained.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));
        let mut kept_bytes = 0u64;
        for (index, (_, bytes, path)) in retained.into_iter().enumerate() {
            kept_bytes = kept_bytes.saturating_add(bytes);
            if index + 1 >= max_actions || kept_bytes > max_bytes {
                remove_action_dir(&path);
            }
        }
    }
}

fn remove_action_dir(path: &Path) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        tracing::warn!(target: "browser", path = %path.display(), "failed to prune staged attachments: {error}");
    }
}

fn directory_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
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
        // Age: the other panel's old action and this panel's first go.
        prune_attachments(&attachments, "panel", Duration::from_millis(300), 8, u64::MAX);
        assert!(!action_directory(&attachments, "other", "old").exists());
        assert!(!action_directory(&attachments, "panel", "first").exists());
        assert!(action_directory(&attachments, "panel", "second").exists());
        assert!(action_directory(&attachments, "panel", "third").exists());
        // Count: room for one more means only the newest of the two stays.
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 2, u64::MAX);
        assert_eq!(
            std::fs::read_dir(attachments.join(crate::paths::safe_local_id("panel")))
                .expect("panel dir")
                .count(),
            1
        );
        // Bytes: a budget below one action's size clears the panel.
        prune_attachments(&attachments, "panel", Duration::from_hours(1), 8, 5);
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
