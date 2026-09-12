//! Initial recorded-commit hydration; never delegate recursion to repository configuration.
use super::{GitPreparation, GitPreparationError as Error, git, lfs, linux::Directory};
use crate::cloud_run::GitCommitSha;
use std::{collections::BTreeMap, path::Path};

const MAX_DEPTH: usize = 4;
const MAX_REPOSITORIES: usize = 32;
const MAX_METADATA: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 128 * 1024;

#[derive(Default)]
pub(super) struct Budget {
    repositories: usize,
    metadata: usize,
    directories: Vec<Directory>,
    pub lfs: lfs::Budget,
}

impl Budget {
    pub(super) fn verify(&self) -> Result<(), Error> {
        for directory in &self.directories {
            directory.verify()?;
        }
        Ok(())
    }
    pub(super) fn charge(&mut self, bytes: usize) -> Result<(), Error> {
        self.metadata = self.metadata.checked_add(bytes).ok_or(Error::UnsupportedRepository)?;
        if bytes > MAX_RESPONSE || self.metadata > MAX_METADATA {
            return Err(Error::UnsupportedRepository);
        }
        Ok(())
    }
}

struct Entry {
    name: String,
    path: String,
    repository: String,
    commit: GitCommitSha,
}

pub(super) struct Plan {
    entries: Vec<Entry>,
    depth: usize,
    directories: Vec<Vec<Directory>>,
}

impl Plan {
    pub(super) fn prepare_paths(
        &mut self,
        directory: &Path,
        verify: &dyn Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        for entry in &self.entries {
            verify()?;
            self.directories.push(Directory::submodule(directory, &entry.path)?);
            verify()?;
        }
        Ok(())
    }
}

pub(super) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.split('/').count() <= 16
        && !path.chars().any(|c| c.is_control() || c == '\\')
        && path.split('/').all(|p| {
            !p.is_empty()
                && p.len() <= 255
                && !matches!(p, "." | "..")
                && !p.trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".git")
        })
}

fn repository(value: &str, parent: &str) -> Result<String, Error> {
    let absolute;
    let path = if let Some(path) = value.strip_prefix("https://github.com/") {
        path
    } else if value.starts_with("./") || value.starts_with("../") {
        let base = format!("{parent}.git");
        let mut parts: Vec<_> = base.split('/').collect();
        for part in value.split('/') {
            match part {
                "." => {}
                ".." => {
                    parts.pop().ok_or(Error::UnsupportedRepository)?;
                }
                "" => return Err(Error::UnsupportedRepository),
                _ => parts.push(part),
            }
        }
        absolute = parts.join("/");
        &absolute
    } else {
        return Err(Error::UnsupportedRepository);
    };
    let slug = path.strip_suffix(".git").ok_or(Error::UnsupportedRepository)?;
    let parts: Vec<_> = slug.split('/').collect();
    if parts.len() != 2
        || slug.len() > 512
        || parts.iter().any(|p| {
            p.is_empty()
                || matches!(*p, "." | "..")
                || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(Error::UnsupportedRepository);
    }
    Ok(slug.into())
}

fn records(bytes: &[u8]) -> Result<impl Iterator<Item = &str>, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::UnsupportedRepository)?;
    let text = text.strip_suffix('\0').ok_or(Error::UnsupportedRepository)?;
    Ok(text.split('\0'))
}

fn decode(tree: &[u8], config: &[u8], parent: &str) -> Result<Vec<Entry>, Error> {
    let mut links = BTreeMap::new();
    let mut modules = false;
    for record in records(tree)? {
        let (object, path) = record.split_once('\t').ok_or(Error::UnsupportedRepository)?;
        let fields: Vec<_> = object.split(' ').collect();
        if fields.len() != 3 {
            return Err(Error::UnsupportedRepository);
        }
        if path == ".gitmodules" {
            if modules || !matches!(fields[0], "100644" | "100755") || fields[1] != "blob" {
                return Err(Error::UnsupportedRepository);
            }
            modules = true;
        }
        if fields[0] == "160000" {
            let commit = GitCommitSha::parse(fields[2]).map_err(|_| Error::UnsupportedRepository)?;
            if fields[1] != "commit"
                || !valid_path(path)
                || fields[2].bytes().all(|b| b == b'0')
                || links.insert(path.to_owned(), commit).is_some()
            {
                return Err(Error::UnsupportedRepository);
            }
        }
    }
    if !modules || links.is_empty() || links.len() > MAX_REPOSITORIES {
        return Err(Error::UnsupportedRepository);
    }
    let mut sections: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    for record in records(config)? {
        let (key, value) = record.split_once('\n').ok_or(Error::UnsupportedRepository)?;
        let (name, key) = key
            .strip_prefix("submodule.")
            .and_then(|s| s.rsplit_once('.'))
            .ok_or(Error::UnsupportedRepository)?;
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
            || value.chars().any(char::is_control)
            || sections.entry(name).or_default().insert(key, value).is_some()
        {
            return Err(Error::UnsupportedRepository);
        }
        match key {
            "path" | "url" | "branch" => {}
            "update" if value == "checkout" => {}
            "ignore" if value == "none" => {}
            "shallow" if matches!(value, "true" | "false") => {}
            "fetchrecursesubmodules" if matches!(value, "true" | "false" | "on-demand") => {}
            _ => return Err(Error::UnsupportedRepository),
        }
    }
    let mut entries = Vec::new();
    for (name, fields) in sections {
        let path = *fields.get("path").ok_or(Error::UnsupportedRepository)?;
        let commit = links.remove(path).ok_or(Error::UnsupportedRepository)?;
        let repository = repository(fields.get("url").ok_or(Error::UnsupportedRepository)?, parent)?;
        entries.push(Entry {
            name: name.into(),
            path: path.into(),
            repository,
            commit,
        });
    }
    if !links.is_empty()
        || entries.iter().enumerate().any(|(i, a)| {
            entries[..i]
                .iter()
                .any(|b| a.path.starts_with(&format!("{}/", b.path)) || b.path.starts_with(&format!("{}/", a.path)))
        })
    {
        return Err(Error::UnsupportedRepository);
    }
    Ok(entries)
}

pub(super) fn read_plan(
    run: &mut impl FnMut(&[&str], &[u8], bool) -> Result<Vec<u8>, Error>,
    request: &GitPreparation,
    has_children: bool,
    depth: usize,
    budget: &mut Budget,
) -> Result<Plan, Error> {
    if !has_children {
        return Ok(Plan {
            entries: Vec::new(),
            depth,
            directories: Vec::new(),
        });
    }
    if depth >= MAX_DEPTH {
        return Err(Error::UnsupportedRepository);
    }
    let commit = request.source.commit.as_str();
    let tree = run(&["ls-tree", "-rz", commit], &[], false)?;
    budget.charge(tree.len())?;
    let blob = format!("{commit}:.gitmodules");
    let size = run(&["cat-file", "-s", &blob], &[], false)?;
    let size: usize = std::str::from_utf8(&size)
        .ok()
        .and_then(|s| s.trim_end().parse().ok())
        .filter(|size| *size <= MAX_RESPONSE)
        .ok_or(Error::UnsupportedRepository)?;
    budget.charge(size)?;
    let config = run(
        &["config", "--no-includes", &format!("--blob={blob}"), "--null", "--list"],
        &[],
        false,
    )?;
    budget.charge(config.len())?;
    let entries = decode(&tree, &config, &request.source.repository)?;
    budget.repositories = budget
        .repositories
        .checked_add(entries.len())
        .ok_or(Error::UnsupportedRepository)?;
    if budget.repositories > MAX_REPOSITORIES {
        return Err(Error::UnsupportedRepository);
    }
    Ok(Plan {
        entries,
        depth,
        directories: Vec::new(),
    })
}

pub(super) fn hydrate(
    git: &mut impl git::Commands,
    directory: &Path,
    request: &GitPreparation,
    plan: Plan,
    cancelled: &dyn Fn() -> bool,
    verify: &dyn Fn() -> Result<(), Error>,
    budget: &mut Budget,
) -> Result<(), Error> {
    if plan.entries.is_empty() {
        return verify();
    }
    if plan.entries.len() != plan.directories.len() {
        return Err(Error::UnsafeRoot);
    }
    for (entry, handles) in plan.entries.iter().zip(plan.directories) {
        verify()?;
        for handle in &handles {
            handle.verify()?;
        }
        let child = handles.last().ok_or(Error::UnsafeRoot)?;
        let git_directory = child.fresh_git_directory()?;
        let verify_child = || {
            verify()?;
            for handle in &handles {
                handle.verify()?;
            }
            git_directory.verify()
        };
        let mut child_request = request.clone();
        child_request.source.repository.clone_from(&entry.repository);
        child_request.source.commit = entry.commit.clone();
        child_request.source.branch = None;
        git::prepare_repository(
            git,
            &child.path,
            &child_request,
            cancelled,
            &verify_child,
            budget,
            plan.depth + 1,
        )?;
        verify_child()?;
        // Only admitted metadata is registered; no update strategy or URL is copied blindly.
        for (key, value) in [
            ("url", format!("https://github.com/{}.git", entry.repository)),
            ("active", "true".into()),
        ] {
            verify_child()?;
            git.run(
                directory,
                &["config", "--local", &format!("submodule.{}.{key}", entry.name), &value],
                &[],
                false,
                cancelled,
            )?;
            verify_child()?;
        }
        budget.directories.extend(handles);
        budget.directories.push(git_directory);
    }
    budget.verify()?;
    verify()?;
    Ok(())
}

#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
