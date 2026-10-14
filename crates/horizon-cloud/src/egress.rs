//! The outbound allowlist of a worker. A profile with `egress` lets its worker reach only
//! GitHub, the package registries, the model APIs of the agents and the names it adds. The
//! host enforces it outside the container (see `host`), so no process in the container can
//! change it. Only Hetzner workers support it.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The names every allowlisted worker reaches. Each name is exact: a wildcard would also allow
/// hosting where anyone can publish a site, such as the storage accounts of a cloud.
pub const DEFAULT_ALLOW: &[&str] = &[
    // GitHub: the site, the API, Git, archives, raw files, release assets, packages and the
    // hosts that the logs and artifacts of Actions are served from.
    "github.com",
    "api.github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "raw.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-cloud.githubusercontent.com",
    "media.githubusercontent.com",
    "pkg-containers.githubusercontent.com",
    "results-receiver.actions.githubusercontent.com",
    "ghcr.io",
    "npm.pkg.github.com",
    "nuget.pkg.github.com",
    // Rust: the crates index and downloads, and the toolchains that rustup installs.
    "index.crates.io",
    "static.crates.io",
    "static.rust-lang.org",
    // npm, and the Node.js headers that native modules build against.
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "nodejs.org",
    // NuGet and the .NET SDK downloads.
    "api.nuget.org",
    "builds.dotnet.microsoft.com",
    "ci.dot.net",
    // PyPI.
    "pypi.org",
    "files.pythonhosted.org",
    // The model APIs: Codex with an API key or a ChatGPT sign-in, and Claude.
    "api.openai.com",
    "auth.openai.com",
    "chatgpt.com",
    "api.anthropic.com",
    "console.anthropic.com",
    "platform.claude.com",
];

/// The storage accounts of GitHub that the logs of Actions jobs redirect to:
/// `productionresultssa0.blob.core.windows.net` up to this count, each an exact name.
pub const ACTIONS_RESULTS_ACCOUNTS: u8 = 20;

/// At most this many names beyond the defaults.
pub const ALLOW_LIMIT: usize = 16;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Egress {
    /// Names beyond `DEFAULT_ALLOW`: a host name such as `nuget.example.com`, or
    /// `*.example.com` for every name under `example.com`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub allow: BTreeSet<String>,
}

impl Egress {
    /// # Errors
    /// Rejects more than `ALLOW_LIMIT` names, a name that is not a lowercase host name, an
    /// address, and a wildcard over a name with fewer than two labels, such as `*.com`.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.allow.len() > ALLOW_LIMIT {
            return Err("egress.allow has more than 16 names");
        }
        if self.allow.iter().all(|entry| valid_entry(entry)) {
            Ok(())
        } else {
            Err("egress.allow takes lowercase host names, or *.name for the names under one")
        }
    }

    /// The allowlist file of the host: one name a line, the defaults first.
    #[must_use]
    pub fn allowlist(&self) -> String {
        let results = (0..ACTIONS_RESULTS_ACCOUNTS).map(|n| format!("productionresultssa{n}.blob.core.windows.net"));
        DEFAULT_ALLOW
            .iter()
            .map(|name| (*name).to_owned())
            .chain(results)
            .chain(self.allow.iter().cloned())
            .fold(String::new(), |mut file, name| {
                file.push_str(&name);
                file.push('\n');
                file
            })
    }
}

fn valid_entry(entry: &str) -> bool {
    let name = entry.strip_prefix("*.").unwrap_or(entry);
    let labels: Vec<&str> = name.split('.').collect();
    let label = |label: &&str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    // A last label of digits is an address, not a name.
    name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(label)
        && !labels
            .last()
            .is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests;
