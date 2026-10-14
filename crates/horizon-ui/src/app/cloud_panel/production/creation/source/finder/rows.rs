//! The results for what is typed: checkouts and the account's repositories ranked for a
//! search, and the folders of a typed path.
use super::{Kind, LISTED, RECENT, Row, fuzzy, github_name};
use horizon_core::dir_search::{self, Checkout};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Checkouts and repositories that match `query`, best first. Before anything is typed the
/// most recently changed checkouts lead, then every repository not cloned yet.
pub(super) fn search_rows(
    query: &str,
    checkouts: &[Checkout],
    labels: &[(String, String)],
    repositories: &[String],
) -> Vec<Row> {
    let listed: HashSet<&str> = repositories.iter().map(String::as_str).collect();
    let mut cloned = HashSet::new();
    // Each candidate with its score and its place in the order it was found, so equal
    // scores keep recent checkouts and the account's order.
    let mut scored: Vec<(i32, usize, Candidate)> = Vec::new();
    // A clone that stopped halfway is no checkout to use. Its folder can still be typed as
    // a path, and a clone into the same place picks it up.
    let usable = checkouts.iter().zip(labels).filter(|(checkout, _)| !checkout.stopped);
    for (recency, (checkout, (title, detail))) in usable.enumerate() {
        let name = checkout.origin.as_deref().and_then(github_name);
        let github = name.as_deref().is_some_and(|name| listed.contains(name));
        // Only a finished checkout stands in for the repository's row, a worktree too; one
        // with no commit yet is listed beside it.
        if github
            && checkout.complete
            && let Some(name) = &name
        {
            cloned.insert(name.clone());
        }
        let (score, marks) = if query.is_empty() {
            if recency >= RECENT {
                continue;
            }
            (-i32::try_from(recency).unwrap_or(i32::MAX), Vec::new())
        } else if let Some(found) = fuzzy::score(query, title) {
            // Being on this computer already is worth a little.
            (found.score + 8, found.positions)
        } else if dir_search::abbreviate_home(&checkout.path)
            .to_lowercase()
            .contains(&query.to_lowercase())
        {
            // Typed letters that only appear scattered over a path are noise; a run of them
            // in the path, such as `projects/tool`, is not.
            (-40, Vec::new())
        } else {
            continue;
        };
        let kind = Kind::Local {
            path: checkout.path.clone(),
            github,
        };
        let row = Row {
            title: title.clone(),
            detail: detail.clone(),
            marks,
            kind,
            repo: name,
        };
        scored.push((score, scored.len(), Candidate::Row(row)));
    }
    for name in repositories.iter().filter(|name| !cloned.contains(name.as_str())) {
        let (score, marks) = if query.is_empty() {
            (-1000, Vec::new())
        } else if let Some(found) = fuzzy::score(query, name) {
            (found.score, found.positions)
        } else {
            continue;
        };
        scored.push((score, scored.len(), Candidate::Repository(name, marks)));
    }
    // Only the best are kept before sorting, so a very long list costs no full sort and no
    // rows for repositories never shown.
    let best = |(score, found, _): &(i32, usize, Candidate)| (std::cmp::Reverse(*score), *found);
    if scored.len() > LISTED {
        scored.select_nth_unstable_by_key(LISTED - 1, best);
        scored.truncate(LISTED);
    }
    scored.sort_unstable_by_key(best);
    scored
        .into_iter()
        .map(|(_, _, candidate)| match candidate {
            Candidate::Row(row) => row,
            Candidate::Repository(name, marks) => Row {
                title: name.clone(),
                detail: "github.com  ·  not on this computer yet".into(),
                marks,
                kind: Kind::GitHub { name: name.clone() },
                repo: Some(name.clone()),
            },
        })
        .collect()
}

/// A result being ranked: a checkout's row, or a repository of the account whose row is made
/// only if it is listed.
enum Candidate<'a> {
    Row(Row),
    Repository(&'a String, Vec<usize>),
}

/// The folders of a typed path: the folder named as typed first, then those whose name
/// starts as typed, then repositories, then by name. Hidden folders show only once a dot
/// is typed.
pub(super) fn folder_rows(typed: &str, folders: &[(PathBuf, bool)]) -> Vec<Row> {
    let dotted = typed
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|last| last.starts_with('.'));
    let name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let mut shown: Vec<(bool, &PathBuf)> = folders
        .iter()
        .filter(|(path, _)| dotted || !name(path).starts_with('.'))
        .map(|(path, checkout)| (*checkout, path))
        .collect();
    // The folder named as typed first, then those that start so, then repositories.
    let typed_name = typed.rsplit(['/', '\\']).next().unwrap_or_default().to_lowercase();
    let rank = |checkout: bool, path: &Path| {
        let name = name(path);
        (
            name == typed_name,
            !typed_name.is_empty() && name.starts_with(&typed_name),
            checkout,
        )
    };
    shown.sort_by(|a, b| {
        rank(b.0, b.1)
            .cmp(&rank(a.0, a.1))
            .then_with(|| name(a.1).cmp(&name(b.1)))
    });
    shown
        .into_iter()
        .take(LISTED)
        .map(|(checkout, path)| Row {
            title: path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            detail: dir_search::abbreviate_home(path),
            marks: Vec::new(),
            kind: Kind::Folder {
                path: path.clone(),
                checkout,
            },
            repo: None,
        })
        .collect()
}
