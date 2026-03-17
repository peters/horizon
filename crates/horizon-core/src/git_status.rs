use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use git2::{Delta, DiffOptions, Repository, StatusOptions};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub status: FileStatus,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Clone, Debug)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    pub new_lineno: Option<u32>,
    pub old_lineno: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Clone, Debug)]
pub struct GitStatus {
    pub repo_root: PathBuf,
    pub branch: Option<String>,
    pub changes: Vec<FileChange>,
    pub diffs: HashMap<String, FileDiff>,
    pub total_insertions: usize,
    pub total_deletions: usize,
    pub timestamp: Instant,
}

impl GitStatus {
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.changes.len()
    }
}

/// Compute the current git status for the repository at `repo_path`.
///
/// # Errors
///
/// Returns an error if the repository cannot be opened or the diff computation fails.
pub fn compute_status(repo_path: &Path) -> Result<GitStatus> {
    let repo = Repository::discover(repo_path).map_err(|e| Error::Git(e.message().to_string()))?;
    let repo_root = repo.workdir().unwrap_or_else(|| repo.path()).to_path_buf();

    let branch = resolve_branch(&repo);
    let (changes, diffs, total_insertions, total_deletions) = compute_changes(&repo)?;

    Ok(GitStatus {
        repo_root,
        branch,
        changes,
        diffs,
        total_insertions,
        total_deletions,
        timestamp: Instant::now(),
    })
}

fn resolve_branch(repo: &Repository) -> Option<String> {
    repo.head().ok().and_then(|head| head.shorthand().map(String::from))
}

type ChangesResult = (Vec<FileChange>, HashMap<String, FileDiff>, usize, usize);

fn compute_changes(repo: &Repository) -> Result<ChangesResult> {
    // First pass: collect file statuses via git2 status API for speed.
    let mut status_opts = StatusOptions::new();
    status_opts.include_untracked(true);
    status_opts.recurse_untracked_dirs(true);
    let statuses = repo
        .statuses(Some(&mut status_opts))
        .map_err(|e| Error::Git(e.message().to_string()))?;

    if statuses.is_empty() {
        return Ok((Vec::new(), HashMap::new(), 0, 0));
    }

    // Second pass: compute unified diff (HEAD to workdir including index).
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());

    let mut diff_opts = DiffOptions::new();
    diff_opts.include_untracked(true);
    diff_opts.recurse_untracked_dirs(true);

    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut diff_opts))
        .map_err(|e| Error::Git(e.message().to_string()))?;

    let stats = diff.stats().map_err(|e| Error::Git(e.message().to_string()))?;
    let total_insertions = stats.insertions();
    let total_deletions = stats.deletions();

    let mut changes: Vec<FileChange> = Vec::new();
    let mut diffs: HashMap<String, FileDiff> = HashMap::new();

    // Collect per-file stats.
    let num_deltas = diff.deltas().len();
    for i in 0..num_deltas {
        let Some(delta) = diff.get_delta(i) else {
            continue;
        };
        let file_status = match delta.status() {
            Delta::Added | Delta::Untracked => FileStatus::Added,
            Delta::Deleted => FileStatus::Deleted,
            Delta::Modified => FileStatus::Modified,
            Delta::Renamed | Delta::Copied => FileStatus::Renamed,
            _ => continue,
        };

        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p: &std::path::Path| p.to_string_lossy().to_string())
            .unwrap_or_default();

        if path.is_empty() {
            continue;
        }

        // Per-file insertions/deletions via patch.
        let (ins, del) = if let Ok(Some(ref patch)) = git2::Patch::from_diff(&diff, i) {
            let (_, a, d) = patch.line_stats().unwrap_or((0, 0, 0));
            (a, d)
        } else {
            (0, 0)
        };

        changes.push(FileChange {
            path: path.clone(),
            status: file_status,
            insertions: ins,
            deletions: del,
        });

        // Build hunk-level diff for each file.
        if let Ok(Some(patch)) = git2::Patch::from_diff(&diff, i) {
            let file_diff = build_file_diff(&path, &patch);
            diffs.insert(path, file_diff);
        }
    }

    // Sort: modified first, then added, then deleted. Within groups, alphabetical.
    changes.sort_by(|a, b| {
        status_order(a.status)
            .cmp(&status_order(b.status))
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok((changes, diffs, total_insertions, total_deletions))
}

fn build_file_diff(file_path: &str, patch: &git2::Patch<'_>) -> FileDiff {
    let num_hunks = patch.num_hunks();
    let mut hunks = Vec::with_capacity(num_hunks);

    for hunk_idx in 0..num_hunks {
        let Ok((hunk, num_lines)) = patch.hunk(hunk_idx) else {
            continue;
        };

        let header = String::from_utf8_lossy(hunk.header()).trim().to_string();
        let mut lines = Vec::with_capacity(num_lines);

        for line_idx in 0..num_lines {
            let Ok(line) = patch.line_in_hunk(hunk_idx, line_idx) else {
                continue;
            };

            let kind = match line.origin() {
                '+' => DiffLineKind::Add,
                '-' => DiffLineKind::Delete,
                _ => DiffLineKind::Context,
            };

            lines.push(DiffLine {
                kind,
                content: String::from_utf8_lossy(line.content()).to_string(),
                new_lineno: line.new_lineno(),
                old_lineno: line.old_lineno(),
            });
        }

        hunks.push(DiffHunk { header, lines });
    }

    FileDiff {
        path: file_path.to_string(),
        hunks,
    }
}

const fn status_order(status: FileStatus) -> u8 {
    match status {
        FileStatus::Modified => 0,
        FileStatus::Added => 1,
        FileStatus::Renamed => 2,
        FileStatus::Deleted => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_ordering_is_stable() {
        assert!(status_order(FileStatus::Modified) < status_order(FileStatus::Added));
        assert!(status_order(FileStatus::Added) < status_order(FileStatus::Deleted));
    }
}
