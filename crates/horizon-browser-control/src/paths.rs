//! Browser coordination paths with Horizon-compatible defaults.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const RUNTIME_ROOT_ENV: &str = "HORIZON_BROWSER_ROOT";
static RUNTIME_PATHS: OnceLock<Option<BrowserRuntimePaths>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum RuntimePathError {
    #[error("browser runtime root must not be empty")]
    EmptyRoot,
    #[error("could not resolve browser runtime root: {0}")]
    Resolve(#[from] std::io::Error),
    #[error("browser runtime paths were already used with defaults or configured with a different root")]
    AlreadyInUse,
}

/// The application home is independent of an explicitly configured browser root.
#[must_use]
pub fn default_horizon_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map_or_else(|| PathBuf::from(".horizon"), |home| home.join(".horizon"))
}

/// Freeze an absolute coordination root before using any default-path helpers.
/// Repeating the same root is allowed; changing it after use is rejected.
/// # Errors
/// Rejects empty paths, unavailable current directories and late reconfiguration.
pub fn configure_runtime_root(root: impl AsRef<Path>) -> Result<(), RuntimePathError> {
    let root = root.as_ref();
    if root.as_os_str().is_empty() {
        return Err(RuntimePathError::EmptyRoot);
    }
    let paths = BrowserRuntimePaths::from_root(std::path::absolute(root)?);
    match RUNTIME_PATHS.set(Some(paths.clone())) {
        Ok(()) => Ok(()),
        Err(_) if RUNTIME_PATHS.get() == Some(&Some(paths)) => Ok(()),
        Err(_) => Err(RuntimePathError::AlreadyInUse),
    }
}

/// Initialize standalone clients before discovery, pruning or serving requests.
/// # Errors
/// Returns configuration errors without falling back to another root.
pub fn initialize_from_environment() -> Result<(), RuntimePathError> {
    if let Some(root) = std::env::var_os(RUNTIME_ROOT_ENV) {
        configure_runtime_root(PathBuf::from(root))
    } else {
        Ok(())
    }
}

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserRuntimePaths {
    root: PathBuf,
}

impl BrowserRuntimePaths {
    #[must_use]
    pub fn resolve() -> Self {
        RUNTIME_PATHS
            .get_or_init(|| None)
            .clone()
            .unwrap_or_else(|| Self::from_root(default_horizon_root()))
    }

    #[must_use]
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn browsers_manifest_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browsers")
    }

    #[must_use]
    pub fn browser_results_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browser-results")
    }

    /// Private copies of files an agent attaches through `set_files`, one
    /// directory per panel and action. They stay while the panel's page may
    /// still read them lazily and are pruned by age, count and size when the
    /// panel's next attachment is staged. The browser must be able to read
    /// them: on Linux a Snap-confined
    /// browser cannot open hidden directories directly beneath the home
    /// directory, so a runtime root such as `~/.horizon` stages under the
    /// visible `~/Horizon` directory the Snap profile root already uses.
    /// That visible directory is namespaced by the runtime root, so two
    /// hidden roots under one home (say `~/.horizon` and `~/.horizon-dev`)
    /// never share staging or sweep each other's. The path is absolute even
    /// when the runtime root is the relative fallback, because queued
    /// attachment paths must be absolute.
    #[must_use]
    pub fn browser_attachments_dir(&self) -> PathBuf {
        let root = std::path::absolute(&self.root).unwrap_or_else(|_| self.root.clone());
        let home = std::env::var_os("HOME").map(PathBuf::from);
        browser_visible_attachments_dir(&root, home.as_deref())
            .unwrap_or_else(|| root.join("runtime").join("browser-attachments"))
    }

    #[must_use]
    pub fn browser_audit_dir(&self) -> PathBuf {
        self.root.join("audit").join("browsers")
    }
}

#[cfg(any(target_os = "linux", test))]
fn browser_visible_attachments_dir(root: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let home = home?;
    let hidden_beneath_home = root
        .strip_prefix(home)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str().to_string_lossy().starts_with('.'));
    hidden_beneath_home.then(|| {
        home.join("Horizon")
            .join("browser-attachments")
            .join(encode_path_bytes(root))
    })
}

/// A directory name derived from a path's exact platform bytes, so two
/// distinct roots never share it however they are spelled.
#[cfg(any(target_os = "linux", test))]
fn encode_path_bytes(path: &Path) -> String {
    #[cfg(unix)]
    let bytes: Vec<u8> = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes: Vec<u8> = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect()
    };
    let mut encoded = String::with_capacity(1 + bytes.len() * 2);
    encoded.push('%');
    for byte in bytes {
        encoded.push(LOWER_HEX[(byte >> 4) as usize] as char);
        encoded.push(LOWER_HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(not(any(target_os = "linux", test)))]
fn browser_visible_attachments_dir(_root: &Path, _home: Option<&Path>) -> Option<PathBuf> {
    None
}

#[must_use]
pub fn safe_local_id(local_id: &str) -> String {
    // Always encode the exact UTF-8 bytes. Keeping an apparently safe ID
    // verbatim would make identifiers that differ only by case collide on
    // default macOS and Windows filesystems. The lowercase hexadecimal
    // alphabet itself has no case variants, and the '%' prefix keeps the
    // empty identifier distinct.
    let mut encoded = String::with_capacity(1 + local_id.len() * 2);
    encoded.push('%');
    for byte in local_id.bytes() {
        encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    #[test]
    fn hidden_runtime_roots_under_one_home_stage_in_separate_visible_directories() {
        let home = std::path::Path::new("/home/someone");
        let first = super::browser_visible_attachments_dir(&home.join(".horizon"), Some(home)).expect("hidden");
        let second = super::browser_visible_attachments_dir(&home.join(".horizon-dev"), Some(home)).expect("hidden");
        assert!(first.starts_with(home.join("Horizon").join("browser-attachments")));
        assert_ne!(first, second, "two hidden roots never share staging");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let odd = |bytes: &[u8]| home.join(std::ffi::OsStr::from_bytes(bytes));
            let left = super::browser_visible_attachments_dir(&odd(b".horizon-\x80"), Some(home)).expect("hidden");
            let right = super::browser_visible_attachments_dir(&odd(b".horizon-\x81"), Some(home)).expect("hidden");
            assert_ne!(left, right, "non-UTF-8 roots are told apart by their exact bytes");
        }
        assert!(super::browser_visible_attachments_dir(&home.join("Horizon"), Some(home)).is_none());
        assert!(super::browser_visible_attachments_dir(&home.join(".horizon"), None).is_none());
    }
}
