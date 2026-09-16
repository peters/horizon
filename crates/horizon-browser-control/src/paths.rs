//! Browser coordination paths with Horizon-compatible defaults.

use std::path::{Path, PathBuf};

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserRuntimePaths {
    root: PathBuf,
}

impl BrowserRuntimePaths {
    #[must_use]
    pub fn resolve() -> Self {
        let root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map_or_else(|| PathBuf::from(".horizon"), |home| home.join(".horizon"));
        Self { root }
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

    #[must_use]
    pub fn browser_audit_dir(&self) -> PathBuf {
        self.root.join("audit").join("browsers")
    }
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
