//! Export only the selected committed tree and its Git history.
mod attributes;
mod material;
use super::{Error, Result, command::Runner};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
/// # Errors
/// Rejects non-commit revisions and unsafe/missing repositories.
pub fn resolve(repository: &Path, revision: &str) -> Result<String> {
    resolve_with_runner(
        repository,
        revision,
        &Runner {
            cancel: &super::Cancellation::default(),
            emit: &|_| {},
            secrets: Vec::new(),
        },
    )
}
/// # Errors
/// Rejects invalid revisions and bounds or cancels repository inspection.
pub fn resolve_with_runner(repository: &Path, revision: &str, runner: &Runner<'_>) -> Result<String> {
    if revision.is_empty() || revision.starts_with('-') || revision.contains(['\n', '\0']) {
        return Err(Error::Invalid("Select a committed Git revision"));
    }
    let output = runner.run(
        "Resolve committed revision",
        Command::new("git").arg("-C").arg(repository).args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ]),
        Duration::from_secs(30),
    )?;
    let sha = output.trim().to_owned();
    if !is_commit_id(&sha) {
        return Err(Error::Invalid("Cannot resolve the selected committed revision"));
    }
    Ok(sha)
}
pub(crate) fn is_commit_id(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64) && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}
/// # Errors
/// Reports export/extraction failures. Local uncommitted files are never copied.
pub fn snapshot(repository: &Path, revision: &str, root: &Path, runner: &Runner<'_>) -> Result<PathBuf> {
    let sha = resolve_with_runner(repository, revision, runner)?;
    let material = material::Material::collect(repository, &sha, runner)?;
    let path = root.join(format!("source-{sha}"));
    if path.exists() {
        return Err(Error::Invalid(
            "Build snapshot already exists; use a new operation directory",
        ));
    }
    std::fs::create_dir(&path)?;
    let archive = root.join("source.tar");
    runner.run(
        "git archive",
        Command::new("git")
            .args(["-c", "core.autocrlf=false"])
            .arg("-C")
            .arg(repository)
            .args(["archive", "--format=tar", "--output"])
            .arg(&archive)
            .arg(&sha),
        Duration::from_secs(120),
    )?;
    runner.run(
        "extract committed source",
        Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&path),
        Duration::from_secs(120),
    )?;
    for (index, module) in material.modules.iter().enumerate() {
        let archive = root.join(format!("module-{index}.tar"));
        let destination = path.join(&module.path);
        std::fs::create_dir_all(&destination)?;
        runner.run(
            "Export committed submodule",
            Command::new("git")
                .args(["-c", "core.autocrlf=false"])
                .arg("-C")
                .arg(&module.repository)
                .args(["archive", "--format=tar", "--output"])
                .arg(&archive)
                .arg(&module.revision),
            Duration::from_secs(120),
        )?;
        runner.run(
            "Extract committed submodule",
            Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(destination),
            Duration::from_secs(120),
        )?;
    }
    material.hydrate(&path, runner)?;
    Ok(path)
}
/// # Errors
/// Packs only objects reachable from the selected commit; no branch mutation/push.
pub fn pack(repository: &Path, revision: &str, output: &Path, runner: &Runner<'_>) -> Result<()> {
    let sha = resolve_with_runner(repository, revision, runner)?;
    let mut input = tempfile::NamedTempFile::new()?;
    writeln!(input, "{sha}")?;
    runner.to_file(
        "Git object transfer",
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["pack-objects", "--stdout", "--revs"]),
        input.path(),
        output,
        Duration::from_secs(300),
    )
}

/// # Errors
/// Requires every selected submodule commit and LFS object to be available locally.
pub fn validate_tree(repository: &Path, revision: &str, runner: &Runner<'_>) -> Result<()> {
    material::Material::collect(repository, revision, runner).map(|_| ())
}
/// # Errors
/// Packages verified LFS content and pinned submodule history without local configuration.
pub fn auxiliary(repository: &Path, revision: &str, root: &Path, runner: &Runner<'_>) -> Result<PathBuf> {
    material::archive(repository, revision, root, runner)
}

#[cfg(test)]
mod tests;
