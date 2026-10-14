//! The checkout of an Upkeep repository on this computer, in `~/github/<owner>/<name>`,
//! cloned with the GitHub sign-in when it is not there yet.
use std::path::{Path, PathBuf};

use crate::cloud_runtime::{self, Cancellation, repository, settings::Settings};

/// The checkout of `name`, cloned with the GitHub sign-in of `settings` when it is not there
/// yet. Blocks on Git and the disk: call it off the UI thread.
/// # Errors
/// A name that is not a GitHub repository, or a clone that failed.
pub fn checkout(root: &Path, settings: &Settings, name: &str) -> Result<PathBuf, String> {
    let remote = repository::source::parse(name).ok_or("The repository is not a GitHub repository.")?;
    let parent = repository::source::default_parent(&crate::dir_search::expand_tilde("~"));
    let found = repository::source::existing(&parent, &remote).or_else(|| {
        let spelled = repository::source::parse(&spelled(&parent, name)?)?;
        repository::source::existing(&parent, &spelled)
    });
    if let Some(path) = found {
        return Ok(path);
    }
    let token = match &settings.github {
        Some(github) => cloud_runtime::github::host::current(root, github)
            .map_err(|error| format!("{error}."))?
            .and_then(|(_, secret)| repository::source::Token::new(&remote, secret.expose())),
        None => None,
    };
    let destination = repository::source::destination(&parent, &remote);
    repository::source::clone(
        &remote,
        &destination,
        token.as_ref(),
        &Cancellation::default(),
        &repository::source::Progress::default(),
    )
    .map_err(|failure| format!("Cloning {name} failed: {failure}"))?;
    Ok(destination)
}

/// `name` as the folders under `parent` spell it when they differ only in case: GitHub
/// names come in lowercase, and an earlier clone keeps the spelling of its owner.
fn spelled(parent: &Path, name: &str) -> Option<String> {
    let (owner, repository) = name.split_once('/')?;
    let find = |folder: &Path, wanted: &str| {
        std::fs::read_dir(folder)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|found| found.eq_ignore_ascii_case(wanted))
    };
    let owner = find(parent, owner)?;
    let repository = find(&parent.join(&owner), repository)?;
    Some(format!("{owner}/{repository}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_earlier_clone_is_found_whatever_the_case_of_its_folders() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Acme/Web")).unwrap();
        assert_eq!(super::spelled(temp.path(), "acme/web").as_deref(), Some("Acme/Web"));
        assert_eq!(super::spelled(temp.path(), "acme/api"), None);
        assert_eq!(super::spelled(temp.path(), "other/web"), None);
    }
}
