use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const MAX_CONTRACT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    Phone,
    Tablet,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct App {
    pub build: Vec<String>,
    pub artifact: PathBuf,
    pub bundle_id: Option<String>,
    pub package: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MatrixEntry {
    pub platform: Platform,
    pub form: Form,
    #[serde(default = "latest")]
    pub os: String,
    pub device: Option<String>,
}

fn latest() -> String {
    "latest".into()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tunnel {
    pub ports: BTreeMap<String, u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(default = "enabled")]
    pub video: bool,
    #[serde(default = "enabled")]
    pub screenshots: bool,
    #[serde(default = "enabled")]
    pub logs_on_failure: bool,
}

const fn enabled() -> bool {
    true
}

impl Default for Evidence {
    fn default() -> Self {
        Self {
            video: true,
            screenshots: true,
            logs_on_failure: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    pub version: u32,
    pub provider: String,
    pub apps: BTreeMap<Platform, App>,
    #[serde(default)]
    pub launch_arguments: BTreeMap<String, String>,
    #[serde(default)]
    pub tunnel: Tunnel,
    pub matrix: Vec<MatrixEntry>,
    /// Exact project-relative recipe files. Markdown recipes contain a device-recipe YAML fence.
    pub recipes: Vec<PathBuf>,
    #[serde(default)]
    pub evidence: Evidence,
    #[serde(default)]
    pub reset: Option<Vec<String>>,
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
}

const fn default_parallel() -> usize {
    16
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(rename = "remote-device-testing")]
    contract: Contract,
}

#[must_use]
pub fn schema() -> schemars::Schema {
    schemars::schema_for!(Envelope)
}

impl Contract {
    /// # Errors
    /// Rejects missing, repeated, malformed or secret-bearing contracts without echoing YAML.
    pub fn from_agents(markdown: &str) -> Result<Self> {
        if markdown.len() > MAX_CONTRACT_BYTES {
            return Err(Error::ContractInvalid);
        }
        let blocks = yaml_blocks(markdown, "remote-device-testing:")?;
        let [block] = blocks.as_slice() else {
            return Err(if blocks.is_empty() {
                Error::ContractMissing
            } else {
                Error::ContractInvalid
            });
        };
        let envelope: Envelope = serde_yaml::from_str(block).map_err(|_| Error::ContractInvalid)?;
        envelope.contract.validate()?;
        Ok(envelope.contract)
    }

    /// # Errors
    /// Returns only typed failures; invalid values and parser diagnostics are never logged.
    pub fn validate(&self) -> Result<()> {
        let invalid = || Error::ContractInvalid;
        if self.version != 1
            || !identifier(&self.provider)
            || self.apps.is_empty()
            || self.matrix.is_empty()
            || self.matrix.len() > 32
            || self.recipes.is_empty()
            || self.recipes.len() > 128
            || !(1..=16).contains(&self.max_parallel)
            || self.launch_arguments.len() > 32
            || self.tunnel.ports.len() > 16
        {
            return Err(invalid());
        }
        for (platform, app) in &self.apps {
            validate_command(&app.build)?;
            relative_path(&app.artifact)?;
            let (extension, id) = match platform {
                Platform::Ios => ("ipa", app.bundle_id.as_deref()),
                Platform::Android => ("apk", app.package.as_deref()),
            };
            if app.artifact.extension().and_then(|s| s.to_str()) != Some(extension)
                || !id.is_some_and(|id| application_id(*platform, id))
                || (*platform == Platform::Ios && app.package.is_some())
                || (*platform == Platform::Android && app.bundle_id.is_some())
            {
                return Err(invalid());
            }
        }
        if let Some(reset) = &self.reset {
            validate_command(reset)?;
        }
        for entry in &self.matrix {
            if !self.apps.contains_key(&entry.platform)
                || !valid_os(&entry.os)
                || entry.device.as_deref().is_some_and(|v| !printable(v, 128))
            {
                return Err(invalid());
            }
        }
        let mut ports = BTreeSet::new();
        for (name, port) in &self.tunnel.ports {
            if !identifier(name) || *port == 0 || !ports.insert(port) {
                return Err(invalid());
            }
        }
        for (name, value) in &self.launch_arguments {
            if !identifier(name) || secret_name(name) || !printable(value, 2048) {
                return Err(invalid());
            }
            self.resolve_value(value)?;
        }
        let mut recipes = BTreeSet::new();
        for recipe in &self.recipes {
            relative_path(recipe)?;
            if !recipes.insert(recipe) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// # Errors
    /// Templates resolve only declared ports, never arbitrary variables or environment secrets.
    pub fn resolve_value(&self, value: &str) -> Result<String> {
        let mut resolved = value.to_owned();
        for (name, port) in &self.tunnel.ports {
            resolved = resolved.replace(&format!("{{tunnel.port.{name}}}"), &port.to_string());
        }
        if resolved.contains(['{', '}']) {
            return Err(Error::ContractInvalid);
        }
        let parsed_url = url::Url::parse(&resolved);
        let folded = resolved.trim().to_ascii_lowercase();
        let authority = folded.split(['/', '?', '#']).next().unwrap_or("");
        if folded
            .as_bytes()
            .get(..2)
            .is_some_and(|prefix| prefix.iter().all(|byte| b"/\\".contains(byte)))
            || authority.rsplit_once(':').is_some_and(|(host, port)| {
                !host.is_empty() && !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            // Endpoints require an explicit scheme so host/port policy cannot depend on app parsing.
            return Err(Error::ContractInvalid);
        }
        if parsed_url.is_ok()
            || resolved.contains("://")
            || ["http:", "https:", "ws:", "wss:", "ftp:"]
                .iter()
                .any(|scheme| folded.starts_with(scheme))
        {
            let url = parsed_url.map_err(|_| Error::ContractInvalid)?;
            let port = url.port_or_known_default().ok_or(Error::ContractInvalid)?;
            if !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
                || !url.username().is_empty()
                || url.password().is_some()
                || !self.tunnel.ports.values().any(|declared| *declared == port)
            {
                return Err(Error::ContractInvalid);
            }
        }
        Ok(resolved)
    }
}

pub(crate) fn yaml_blocks<'a>(markdown: &'a str, marker: &str) -> Result<Vec<&'a str>> {
    let mut blocks = Vec::new();
    let mut start = None;
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let fence = line.trim();
        if fence == "```yaml" || fence == "```yml" {
            if start.is_some() {
                return Err(Error::ContractInvalid);
            }
            start = Some(offset + line.len());
        } else if fence == "```"
            && let Some(begin) = start.take()
        {
            let block = &markdown[begin..offset];
            if block.lines().any(|line| line.starts_with(marker)) {
                blocks.push(block);
            }
        }
        offset += line.len();
    }
    if start.is_some() {
        return Err(Error::ContractInvalid);
    }
    Ok(blocks)
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn application_id(platform: Platform, value: &str) -> bool {
    value.len() <= 255
        && value.contains('.')
        && value.split('.').all(|segment| {
            identifier(segment)
                && match platform {
                    Platform::Ios => segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                    Platform::Android => {
                        segment.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                            && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    }
                }
        })
}

pub(crate) fn printable(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn secret_name(value: &str) -> bool {
    let folded: String = value
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    if ["auth", "bearer", "cookie", "pwd", "jwt", "sshkey", "oauthcode"].contains(&folded.as_str()) {
        return true;
    }
    [
        "secret",
        "token",
        "password",
        "passphrase",
        "signingkey",
        "sessionkey",
        "encryptionkey",
        "clientkey",
        "sshkey",
        "jwt",
        "oauthcode",
        "credential",
        "privatekey",
        "apikey",
        "accesskey",
        "authorization",
    ]
    .iter()
    .any(|part| folded.contains(part))
}

fn validate_command(command: &[String]) -> Result<()> {
    if command.is_empty() || command.len() > 64 || command.iter().any(|v| !printable(v, 4096)) {
        return Err(Error::ContractInvalid);
    }
    Ok(())
}

pub(crate) fn valid_os(value: &str) -> bool {
    value == "latest"
        || value
            .strip_prefix("latest-")
            .is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n <= 10))
        || version(value).is_some()
}

pub(crate) fn version(value: &str) -> Option<Vec<u32>> {
    if value.len() > 32 {
        return None;
    }
    let parts: Option<Vec<_>> = value
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect();
    parts.filter(|v| !v.is_empty() && v.len() <= 4 && v[0] > 0)
}

pub(crate) fn relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.to_str().is_none_or(|s| {
            s.len() > 4096 || s.contains(['\\', ':', '*', '?', '[', ']']) || s.chars().any(char::is_control)
        })
        || path.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::PathRejected);
    }
    Ok(())
}

/// Check existing ancestors as well as the final artifact, so build-time paths cannot escape via a symlink.
/// # Errors
/// Rejects lexical traversal, symlink escapes and unavailable repository roots.
pub fn project_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    relative_path(relative)?;
    let root = root.canonicalize().map_err(|_| Error::FileUnavailable)?;
    let path = root.join(relative);
    let mut ancestor = path.as_path();
    loop {
        match ancestor.symlink_metadata() {
            Ok(_) => {
                if !ancestor
                    .canonicalize()
                    .map_err(|_| Error::PathRejected)?
                    .starts_with(&root)
                {
                    return Err(Error::PathRejected);
                }
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor.parent().ok_or(Error::PathRejected)?;
            }
            Err(_) => return Err(Error::FileUnavailable),
        }
    }
}
