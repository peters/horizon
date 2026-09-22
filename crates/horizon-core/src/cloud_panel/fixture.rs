use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::{CLOUDS, CloudGroups};
use crate::{Error, Result, RuntimeState};

#[derive(Deserialize, Serialize)]
pub struct PrototypeSnapshot {
    pub version: u32,
    pub groups: CloudGroups,
    pub runtime: RuntimeState,
}

/// # Errors
/// Returns an error for unreadable, malformed or unsupported private state.
pub fn load(root: &Path) -> Result<Option<PrototypeSnapshot>> {
    let path = root.join("cloud-panels.json");
    if !path.exists() {
        return Ok(None);
    }
    let snapshot: PrototypeSnapshot =
        serde_json::from_slice(&std::fs::read(path)?).map_err(|e| Error::Config(e.to_string()))?;
    if snapshot.version != 1 {
        return Err(Error::Config("Unsupported cloud prototype snapshot".into()));
    }
    Ok(Some(snapshot))
}

/// # Errors
/// Returns an error if the private snapshot cannot be atomically persisted.
pub fn save(root: &Path, snapshot: &PrototypeSnapshot) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut file, snapshot).map_err(|e| Error::Config(e.to_string()))?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(root.join("cloud-panels.json"))
        .map_err(|e| Error::Io(e.error))?;
    Ok(())
}

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(Error::Config(format!(
            "Prototype repository setup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

/// Prepare only this explicitly selected prototype directory. Existing worktrees
/// are reused without resetting or cleaning the user's ongoing changes.
///
/// # Errors
/// Returns an error when Git or filesystem setup fails.
pub fn prepare_repository(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.is_absolute() {
        return Err(Error::Config("Prototype directory must be absolute".into()));
    }
    std::fs::create_dir_all(root)?;
    let repo = root.join("sample-project");
    if !repo.exists() {
        std::fs::create_dir(&repo)?;
        git(&repo, &["init", "-b", "main"])?;
        std::fs::write(
            repo.join("README.md"),
            "# Project desk\n\nDisposable cloud-panel prototype project.\nRun `python3 -m http.server 8080 --bind 127.0.0.1` to preview index.html.\n",
        )?;
        std::fs::write(
            repo.join("AGENTS.md"),
            "Work only in this cloud worktree. This is a disposable UI prototype. Make requested changes locally. Do not push, create PRs, or access other repositories. Wait for the user's instructions.\n",
        )?;
        std::fs::write(
            repo.join("index.html"),
            r#"<!doctype html><html lang="en"><meta charset="utf-8"><title>Project desk</title><style>body{background:#1e1e2e;color:#cdd6f4;font:18px system-ui;margin:64px}article{padding:24px;border:1px solid #45475a;border-radius:12px;margin-top:24px}button,input{font:inherit;padding:10px}h1{color:#89b4fa}</style><h1>Project desk</h1><p>A small project for trying real changes in a cloud session.</p><article><h2>Your projects</h2><p>No projects yet.</p><button>Create project</button></article></html>"#,
        )?;
        std::fs::create_dir(repo.join(".horizon"))?;
        std::fs::write(repo.join(".horizon/cloud.yml"), horizon_cloud::DESIGN_EXAMPLE)?;
        git(&repo, &["add", "."])?;
        git(
            &repo,
            &[
                "-c",
                "user.name=Prototype",
                "-c",
                "user.email=prototype@example.invalid",
                "commit",
                "-m",
                "Initialize prototype fixture",
            ],
        )?;
    }
    let mut worktrees = Vec::new();
    for (issue, title) in CLOUDS {
        let path = root.join(format!("issue-{issue}"));
        if !path.exists() {
            let branch = format!("issue-{issue}-prototype");
            let path_text = path
                .to_str()
                .ok_or_else(|| Error::Config("Prototype path must be UTF-8".into()))?;
            git(&repo, &["worktree", "add", "-b", &branch, path_text, "main"])?;
            std::fs::write(
                path.join("CLOUD.md"),
                format!(
                    "# {title}\n\nA disposable cloud workspace. Wait for instructions before editing. File edits and agent conversations are real.\n"
                ),
            )?;
        }
        worktrees.push(path);
    }
    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_worktrees_are_isolated_and_reopening_preserves_edits() {
        let temp = tempfile::tempdir().unwrap();
        let paths = prepare_repository(temp.path()).unwrap();
        std::fs::write(paths[0].join("index.html"), "changed").unwrap();
        assert_ne!(
            std::fs::read(paths[0].join("index.html")).unwrap(),
            std::fs::read(paths[1].join("index.html")).unwrap()
        );
        assert_eq!(paths, prepare_repository(temp.path()).unwrap());
        assert_eq!(std::fs::read_to_string(paths[0].join("index.html")).unwrap(), "changed");
    }
}
