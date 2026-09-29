//! Export only the selected committed tree and its Git history.
mod archive;
mod attributes;
pub mod launch;
mod material;
pub mod source;
use super::{Error, Result, command::Runner};
use horizon_cloud::Source;
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
/// A full commit ID as Git prints it and the worker accepts it: lowercase hexadecimal.
pub(crate) fn is_commit_id(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64)
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
        archive_command(repository).arg(&archive).arg(&sha),
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
            archive_command(&module.repository).arg(&archive).arg(&module.revision),
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
/// `git archive` converts line endings the way a checkout on this host would.
/// The Linux worker builds the snapshot, so pin the conversion to a Linux
/// checkout's: Windows defaults (`core.autocrlf=true`, a CRLF `core.eol`) would
/// otherwise turn committed LF text into CRLF. Explicit `eol` attributes still
/// apply. The caller appends the output path and revision.
fn archive_command(repository: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.autocrlf=false", "-c", "core.eol=lf", "-C"])
        .arg(repository)
        .args(["archive", "--format=tar", "--output"]);
    command
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

/// Packs only the selected commit and its tree, with no ancestors; the worker records
/// the commit as shallow.
fn pack_pinned(repository: &Path, revision: &str, output: &Path, runner: &Runner<'_>) -> Result<()> {
    let sha = resolve_with_runner(repository, revision, runner)?;
    let scratch = tempfile::tempdir()?;
    let (empty, objects) = (scratch.path().join("empty"), scratch.path().join("objects"));
    std::fs::File::create_new(&empty)?;
    runner.to_file(
        "List pinned submodule objects",
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["rev-list", "--objects", "--no-walk", &sha]),
        &empty,
        &objects,
        Duration::from_secs(300),
    )?;
    runner.to_file(
        "Git object transfer",
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["pack-objects", "--stdout"]),
        &objects,
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
/// As [`validate_tree`], except that LFS objects `lfs` leaves out need not be local.
pub fn validate_selected(
    repository: &Path,
    revision: &str,
    lfs: &horizon_cloud::Lfs,
    runner: &Runner<'_>,
) -> Result<()> {
    material::Material::collect_selecting(repository, revision, lfs, runner).map(|_| ())
}
/// # Errors
/// Packages verified LFS content and submodule history, as `source` selects, without
/// local configuration.
pub fn auxiliary(
    repository: &Path,
    revision: &str,
    root: &Path,
    source: &Source,
    runner: &Runner<'_>,
) -> Result<PathBuf> {
    material::archive(repository, revision, root, source, runner)
}

/// Reserves the transfer frame while counting retained output and disposable material.
#[cfg(target_os = "linux")]
pub(super) fn bounded_source(
    repository: &Path,
    revision: &str,
    retained: &Path,
    scratch: &Path,
    runner: &Runner<'_>,
    limit: u64,
) -> Result<()> {
    let selected = material::Material::collect_bounded(repository, revision, runner, limit)?;
    let mut budget = ExportBudget { remaining: limit };
    budget.charge(horizon_cloud_protocol::membership::Source::MAX_REQUEST_BYTES as u64 + 4)?;
    bounded_pack(repository, revision, &retained.join("pack"), runner, &mut budget, 2)?;
    let directory = scratch.join("material");
    std::fs::create_dir(&directory)?;
    for (index, module) in selected.modules.iter().enumerate() {
        bounded_pack(
            &module.repository,
            &module.revision,
            &directory.join(format!("module-{index}.pack")),
            runner,
            &mut budget,
            1,
        )?;
    }
    let manifest = serde_json::to_vec(&selected).map_err(|_| Error::Json)?;
    if manifest.len() > 1024 * 1024 {
        return Err(Error::Invalid("Source material manifest exceeds its limit"));
    }
    // The archive is retained and later framed, so it is charged twice, as a pack is.
    let path = retained.join("source-material.tar");
    let mut output = Bounded {
        file: std::io::BufWriter::new(std::fs::File::create_new(&path)?),
        remaining: budget.remaining / 2,
        written: 0,
        exceeded: false,
    };
    let result = archive::write(&selected, &manifest, &directory, &mut output, runner, archive::TIMEOUT);
    let (exceeded, written) = (output.exceeded, output.written);
    drop(output);
    if exceeded || result.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    if exceeded {
        return Err(Error::Invalid("Source export exceeds its aggregate byte budget"));
    }
    result?;
    budget.charge(written * 2)
}

/// Refuses any write past its share of the export budget.
#[cfg(target_os = "linux")]
struct Bounded {
    file: std::io::BufWriter<std::fs::File>,
    remaining: u64,
    written: u64,
    exceeded: bool,
}
#[cfg(target_os = "linux")]
impl Write for Bounded {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let length = buffer.len() as u64;
        if length > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::other("Source export exceeds its aggregate byte budget"));
        }
        let written = self.file.write(buffer)?;
        self.remaining -= written as u64;
        self.written += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(target_os = "linux")]
struct ExportBudget {
    remaining: u64,
}
#[cfg(target_os = "linux")]
impl ExportBudget {
    fn charge(&mut self, length: u64) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(length)
            .ok_or(Error::Invalid("Source export exceeds its aggregate byte budget"))?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn bounded_pack(
    repository: &Path,
    revision: &str,
    output: &Path,
    runner: &Runner<'_>,
    budget: &mut ExportBudget,
    copies: u64,
) -> Result<()> {
    let sha = resolve_with_runner(repository, revision, runner)?;
    let mut input = tempfile::tempfile()?;
    writeln!(input, "{sha}")?;
    std::io::Seek::rewind(&mut input)?;
    bounded_output(
        Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(["pack-objects", "--stdout", "--revs"])
            .stdin(input),
        output,
        runner,
        budget,
        copies,
    )
}

#[cfg(target_os = "linux")]
fn bounded_output(
    command: &mut Command,
    output: &Path,
    runner: &Runner<'_>,
    budget: &mut ExportBudget,
    copies: u64,
) -> Result<()> {
    let mut file = std::fs::File::create_new(output)?;
    let length = runner.bounded_file(command, &mut file, budget.remaining / copies, Duration::from_secs(300))?;
    budget.charge(length * copies)
}

#[cfg(test)]
mod tests;
