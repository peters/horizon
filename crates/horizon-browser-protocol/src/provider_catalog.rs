//! Safe, credential-free provider discovery shared by every browser interface.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct CatalogDevice {
    pub target: String,
    pub provider: String,
    pub os: String,
    pub os_version: String,
    pub browser: String,
    pub browser_version: Option<String>,
    pub device: Option<String>,
    pub real_mobile: bool,
}

impl CatalogDevice {
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "{} · {} {} · {} {}",
            self.device.as_deref().unwrap_or("Desktop"),
            self.os,
            self.os_version,
            self.browser,
            self.browser_version.as_deref().unwrap_or_default()
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogQuery {
    pub provider: String,
    /// Case-insensitive words, all of which must occur in the device description.
    pub search: String,
    pub offset: usize,
}

impl CatalogQuery {
    #[must_use]
    pub fn valid(&self) -> bool {
        valid_provider(&self.provider)
            && self.search.len() <= 128
            && !self.search.chars().any(char::is_control)
            && self.offset <= 50_000
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct CatalogPage {
    pub devices: Vec<CatalogDevice>,
    pub total: usize,
    pub next_offset: Option<usize>,
}

impl CatalogPage {
    #[must_use]
    pub fn select(devices: &[CatalogDevice], query: &CatalogQuery) -> Self {
        let words: Vec<_> = query.search.split_whitespace().map(str::to_ascii_lowercase).collect();
        let mut matched = devices.iter().filter(|device| {
            device.provider == query.provider && {
                let label = device.label().to_ascii_lowercase();
                words.iter().all(|word| label.contains(word))
            }
        });
        let total = matched.clone().count();
        let devices: Vec<_> = matched.by_ref().skip(query.offset).take(50).cloned().collect();
        let next = query.offset.saturating_add(devices.len());
        Self {
            devices,
            total,
            next_offset: (next < total).then_some(next),
        }
    }
}

/// Catalog references cannot collide with configured target identifiers (maximum 64 bytes).
#[must_use]
pub fn target_provider(target: &str) -> Option<&str> {
    let (provider, digest) = target.strip_prefix("catalog.")?.rsplit_once('.')?;
    (valid_provider(provider) && digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(provider)
}

fn valid_provider(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
