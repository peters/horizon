//! Host-path policy for browser file attachments.
//!
//! A `set_files` action makes the browser read files from this host, so the
//! paths an agent may name are confined to explicit roots: the agent's work
//! root (Horizon exports it as `HORIZON_WORK_ROOT`), the process working
//! directory when no work root is set, and any extra roots listed in
//! `HORIZON_BROWSER_ATTACHMENT_ROOTS`. Every path is resolved through its
//! symlinks before the root check, so a link inside a root cannot reach out
//! of it, and the resolved path is what the engine receives.

use std::path::{Path, PathBuf};

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
}

/// The resolved roots an attachment path must fall under.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttachmentPolicy {
    roots: Vec<PathBuf>,
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

    /// Resolve every path and confirm it is a regular file under a root.
    /// Returns the resolved paths in request order; the first refusal ends
    /// the check, so nothing is authorized when any path is refused.
    ///
    /// # Errors
    /// Returns which path was refused and why, never file contents.
    pub fn authorize(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>, AttachmentPolicyError> {
        if self.roots.is_empty() {
            return Err(AttachmentPolicyError::NoRoots);
        }
        paths.iter().map(|path| self.authorize_one(path)).collect()
    }

    fn authorize_one(&self, path: &Path) -> Result<PathBuf, AttachmentPolicyError> {
        let display = path.display().to_string();
        let resolved = std::fs::canonicalize(path).map_err(|error| AttachmentPolicyError::Unresolvable {
            path: display.clone(),
            reason: error.kind().to_string(),
        })?;
        let metadata = std::fs::metadata(&resolved).map_err(|error| AttachmentPolicyError::Unresolvable {
            path: display.clone(),
            reason: error.kind().to_string(),
        })?;
        if !metadata.is_file() {
            return Err(AttachmentPolicyError::NotAFile { path: display });
        }
        if self.roots.iter().any(|root| resolved.starts_with(root)) {
            Ok(resolved)
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

#[cfg(test)]
mod tests {
    use super::*;

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
            authorized,
            vec![std::fs::canonicalize(&file).expect("canonical")],
            "the engine receives the resolved path"
        );
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
        assert_eq!(
            policy.authorize(&[root.path().to_path_buf()]),
            Err(AttachmentPolicyError::NotAFile {
                path: root.path().display().to_string()
            })
        );
        assert!(matches!(
            policy.authorize(&[root.path().join("missing.txt")]),
            Err(AttachmentPolicyError::Unresolvable { .. })
        ));
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
        assert_eq!(
            policy.authorize(&[root.path().join("any.txt")]),
            Err(AttachmentPolicyError::NoRoots)
        );
    }
}
