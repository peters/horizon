//! Dependency maintenance: a worker that keeps Dependabot pull requests moving across
//! repositories, as the Dependencies panel sees it. The worker's documents are external
//! input; they are size-limited, schema-checked and never turned into commands.

pub mod portfolio;
pub mod setup;
#[cfg(test)]
mod tests;
mod transport;

use std::path::Path;

pub use transport::{Endpoint, FIXTURE_ENV, Poller};

/// The GitHub App this machine connected in Cloud settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubApp {
    pub slug: String,
    /// Where the person chooses which repositories the app reaches.
    pub installation_url: String,
}

/// The connected GitHub App, or `None` when Cloud settings have none or cannot be read.
#[must_use]
pub fn connected_github_app(cloud_root: &Path) -> Option<GitHubApp> {
    let app = crate::cloud_runtime::settings::Settings::load(&cloud_root.join("settings.json"))
        .ok()?
        .github?;
    Some(GitHubApp {
        installation_url: app.installation_url(),
        slug: app.slug,
    })
}
