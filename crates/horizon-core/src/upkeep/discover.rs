//! The repositories a new Upkeep can watch: every repository that this computer's GitHub
//! sign-in reaches through the app, with the open pull requests the chosen bots have there.
//!
//! The names come from GitHub through the signed-in account, never from a worker. Each one
//! is a checked `owner/name` in lowercase, and a repository the app cannot reach is never
//! offered, so New Upkeep can only choose repositories the cloud can be given.
use std::collections::BTreeMap;
use std::path::Path;

use horizon_cloud::github::Client;

use crate::cloud_runtime::{github::host, settings::Settings};

/// What New Upkeep offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    /// Most open pull requests first, then by name.
    pub repositories: Vec<CatalogRepository>,
    /// Why the pull requests could not be counted. The repositories are still listed, each
    /// with no open pull requests.
    pub search_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRepository {
    /// `owner/name`, in lowercase.
    pub name: String,
    /// Open pull requests from the chosen bots.
    pub open: u32,
}

impl CatalogRepository {
    #[must_use]
    pub fn owner(&self) -> &str {
        self.name.split_once('/').map_or(self.name.as_str(), |(owner, _)| owner)
    }

    #[must_use]
    pub fn short_name(&self) -> &str {
        self.name.split_once('/').map_or(self.name.as_str(), |(_, name)| name)
    }
}

impl Catalog {
    /// Owners in the order they first appear.
    #[must_use]
    pub fn owners(&self) -> Vec<&str> {
        let mut owners: Vec<&str> = Vec::new();
        for repository in &self.repositories {
            if !owners.contains(&repository.owner()) {
                owners.push(repository.owner());
            }
        }
        owners
    }

    /// Open pull requests in the repositories named in `chosen`.
    #[must_use]
    pub fn open_in<'a>(&self, chosen: impl IntoIterator<Item = &'a String>) -> u32 {
        chosen
            .into_iter()
            .filter_map(|name| self.repositories.iter().find(|repository| repository.name == *name))
            .map(|repository| repository.open)
            .sum()
    }
}

/// The catalog for this computer's GitHub sign-in in the cloud root `root`, with the open
/// pull requests of `authors`, such as `app/dependabot`. Blocks on GitHub: call it off the
/// UI thread.
/// # Errors
/// No GitHub sign-in, GitHub out of reach, or a refused token.
pub fn discover(root: &Path, authors: &[String]) -> Result<Catalog, String> {
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    let github = settings
        .github
        .as_ref()
        .ok_or("Connect GitHub in Cloud settings first.")?;
    let (_, token) = host::current(root, github)
        .map_err(|error| error.to_string())?
        .ok_or("Sign in to GitHub in Cloud settings first.")?;
    let reachable =
        host::repositories(&token).map_err(|error| format!("Listing the app's repositories failed: {error}"))?;
    let owners: Vec<String> = reachable
        .iter()
        .filter_map(|name| name.split_once('/').map(|(owner, _)| owner.to_owned()))
        .collect();
    let client = Client::new();
    let mut open = BTreeMap::<String, u32>::new();
    let mut search_error = None;
    for author in authors {
        match client.open_pull_requests_by(&token, author, &owners, &reachable) {
            Ok(counts) => {
                for (name, count) in counts {
                    *open.entry(name).or_insert(0) += count;
                }
            }
            Err(error) => {
                // Counts of only some bots would put the list in a wrong order.
                search_error = Some(format!("searching the pull requests of {author} failed: {error}"));
                open.clear();
                break;
            }
        }
    }
    let mut catalog = catalog(&reachable, &open);
    catalog.search_error = search_error;
    Ok(catalog)
}

/// The reachable repositories, most open pull requests first, then by name.
fn catalog(reachable: &[String], open: &BTreeMap<String, u32>) -> Catalog {
    let mut repositories: Vec<CatalogRepository> = reachable
        .iter()
        .map(|name| CatalogRepository {
            name: name.clone(),
            open: open.get(name).copied().unwrap_or(0),
        })
        .collect();
    repositories.sort_by(|a, b| b.open.cmp(&a.open).then_with(|| a.name.cmp(&b.name)));
    Catalog {
        repositories,
        search_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(catalog: &Catalog) -> Vec<(&str, u32)> {
        catalog
            .repositories
            .iter()
            .map(|repository| (repository.name.as_str(), repository.open))
            .collect()
    }

    #[test]
    fn repositories_with_open_pull_requests_come_first() {
        let reachable = ["acme/api", "acme/docs", "acme/web", "beta/app"].map(String::from);
        let open = [("acme/web".to_string(), 2), ("acme/api".to_string(), 5)].into();
        let catalog = catalog(&reachable, &open);
        assert_eq!(
            names(&catalog),
            [("acme/api", 5), ("acme/web", 2), ("acme/docs", 0), ("beta/app", 0)]
        );
        assert_eq!(catalog.owners(), ["acme", "beta"]);
        assert_eq!(catalog.repositories[0].short_name(), "api");
    }

    #[test]
    fn counts_only_name_repositories_the_catalog_holds() {
        let reachable = ["acme/api", "acme/web"].map(String::from);
        let open = [("acme/web".to_string(), 2), ("acme/api".to_string(), 5)].into();
        let catalog = catalog(&reachable, &open);
        let chosen = ["acme/web".to_string(), "other/repo".to_string()];
        assert_eq!(catalog.open_in(&chosen), 2);
    }
}
