use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// Directories that are never useful to enter.
const SKIP_NAMES: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".cache",
    ".npm",
    ".cargo",
    ".rustup",
    ".local",
    ".vscode-server",
    "vendor",
    "dist",
    "build",
    ".Trash",
    "Library",
];

/// Maximum depth for recursive fuzzy search.
const MAX_DEPTH: usize = 5;

/// Maximum results before we stop searching.
const MAX_RESULTS: usize = 200;

/// Spawn a background thread that searches for directories matching `query`.
/// Returns a receiver that will eventually yield the results.
///
/// When the query changes the caller simply drops the old receiver and calls
/// this again — the orphaned thread finishes harmlessly.
#[must_use]
pub fn spawn_lookup(query: String) -> mpsc::Receiver<Vec<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let results = search_directories(&query);
        let _ = tx.send(results);
    });
    rx
}

fn home_dir() -> PathBuf {
    std::env::var("HOME").map_or_else(|_| PathBuf::from("/"), PathBuf::from)
}

fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix('~') {
        let home = home_dir();
        if rest.is_empty() {
            home
        } else {
            home.join(rest.strip_prefix('/').unwrap_or(rest))
        }
    } else {
        PathBuf::from(input)
    }
}

fn search_directories(query: &str) -> Vec<PathBuf> {
    let trimmed = query.trim();

    if trimmed.is_empty() {
        return list_home_children();
    }

    // Path-based completion: query contains a separator or starts with ~ /
    if trimmed.starts_with('/') || trimmed.starts_with('~') {
        return complete_path(trimmed);
    }
    if trimmed.contains('/') {
        // The UI shows ~ as a prefix, so bare relative paths like "github/foo"
        // should resolve against $HOME, not the current working directory.
        return complete_path(&format!("~/{trimmed}"));
    }

    // Fuzzy name search from $HOME
    fuzzy_search(trimmed)
}

/// List immediate subdirectories of $HOME, sorted with project-like dirs first.
fn list_home_children() -> Vec<PathBuf> {
    let home = home_dir();
    let mut dirs = list_child_dirs(&home);

    // Partition: non-hidden first, hidden last
    dirs.sort_by(|a, b| {
        let a_hidden = is_hidden(a);
        let b_hidden = is_hidden(b);
        a_hidden
            .cmp(&b_hidden)
            .then_with(|| has_project_marker(b).cmp(&has_project_marker(a)))
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    dirs
}

/// Tab-complete a partial path.
fn complete_path(input: &str) -> Vec<PathBuf> {
    let expanded = expand_tilde(input);

    // If the expanded path is an existing directory and the input ends with /,
    // list its children.
    if expanded.is_dir() && input.ends_with('/') {
        return list_child_dirs(&expanded);
    }

    // Otherwise complete in the parent directory.
    let parent = expanded.parent().unwrap_or(Path::new("/"));
    let prefix = expanded
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if !parent.is_dir() {
        return Vec::new();
    }

    let mut matches: Vec<PathBuf> = list_child_dirs(parent)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().to_lowercase().contains(&prefix))
        })
        .collect();

    matches.sort_by(|a, b| {
        let a_name = a.file_name().map(|n| n.to_string_lossy().to_lowercase());
        let b_name = b.file_name().map(|n| n.to_string_lossy().to_lowercase());
        let a_starts = a_name.as_deref().is_some_and(|n| n.starts_with(&prefix));
        let b_starts = b_name.as_deref().is_some_and(|n| n.starts_with(&prefix));
        b_starts.cmp(&a_starts).then_with(|| a.file_name().cmp(&b.file_name()))
    });

    matches
}

/// BFS fuzzy search from $HOME for directories whose name matches `query`.
fn fuzzy_search(query: &str) -> Vec<PathBuf> {
    let home = home_dir();
    let query_lower = query.to_lowercase();
    let query_parts: Vec<&str> = query_lower.split_whitespace().collect();

    let mut results: Vec<(PathBuf, i32)> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((home.clone(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if depth > MAX_DEPTH || results.len() >= MAX_RESULTS {
            break;
        }

        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = match path.file_name() {
                Some(name) => name.to_string_lossy(),
                None => continue,
            };

            if name.starts_with('.') {
                // Allow .config but skip most hidden dirs
                if name != ".config" {
                    continue;
                }
            }

            if SKIP_NAMES.iter().any(|skip| name == *skip) {
                continue;
            }

            let name_lower = name.to_lowercase();
            let full_path_str = path.display().to_string().to_lowercase();

            // Check if all query parts match either the name or full path
            let all_match = query_parts
                .iter()
                .all(|part| name_lower.contains(part) || full_path_str.contains(part));

            if all_match {
                let score = score_match(&name_lower, &query_parts, depth);
                results.push((path.clone(), score));
            }

            // Always recurse (unless at max depth)
            if depth < MAX_DEPTH {
                queue.push_back((path, depth + 1));
            }
        }
    }

    results.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    results.into_iter().map(|(path, _)| path).collect()
}

fn score_match(name: &str, query_parts: &[&str], depth: usize) -> i32 {
    let mut score: i32 = 1000;

    // Prefer shallower directories
    let depth_i32 = i32::try_from(depth).unwrap_or(i32::MAX);
    score -= depth_i32 * 80;

    for part in query_parts {
        if name == *part {
            score += 500; // exact match
        } else if name.starts_with(part) {
            score += 200; // prefix match
        }
    }

    score
}

fn list_child_dirs(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect()
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

fn has_project_marker(path: &Path) -> bool {
    path.join(".git").exists()
        || path.join("Cargo.toml").exists()
        || path.join("package.json").exists()
        || path.join("go.mod").exists()
        || path.join("pyproject.toml").exists()
}

/// Abbreviate a path by replacing $HOME with ~.
#[must_use]
pub fn abbreviate_home(path: &Path) -> String {
    let home = home_dir();
    match path.strip_prefix(&home) {
        Ok(relative) => {
            if relative.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", relative.display())
            }
        }
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion_works() {
        let home = home_dir();
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/foo"), home.join("foo"));
        assert_eq!(expand_tilde("/etc"), PathBuf::from("/etc"));
    }

    #[test]
    fn abbreviate_home_replaces_prefix() {
        let home = home_dir();
        assert_eq!(abbreviate_home(&home), "~");
        assert_eq!(abbreviate_home(&home.join("projects")), "~/projects");
    }

    #[test]
    fn empty_query_returns_home_children() {
        let results = search_directories("");
        // Should return something (home always has directories)
        // Just verify it doesn't panic
        assert!(results.iter().all(|p| p.is_dir()));
    }
}
