//! Export only the selected committed tree and its Git history.
mod attributes;
pub mod launch;
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
    let objects = directory.join("lfs");
    std::fs::create_dir(&objects)?;
    let mut copied = std::collections::BTreeSet::new();
    for asset in &selected.assets {
        if copied.insert(&asset.oid) {
            copy_asset(asset, &objects.join(&asset.oid), runner, &mut budget)?;
        }
    }
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
    budget.charge(manifest.len() as u64)?;
    std::fs::File::create_new(directory.join("manifest.json"))?.write_all(&manifest)?;
    bounded_output(
        Command::new("tar")
            .args(["-cf", "-", "-C"])
            .arg(&directory)
            .arg(".")
            .stdin(std::process::Stdio::null()),
        &retained.join("source-material.tar"),
        runner,
        &mut budget,
        2,
    )
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

#[cfg(target_os = "linux")]
fn copy_asset(asset: &material::Asset, path: &Path, runner: &Runner<'_>, budget: &mut ExportBudget) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    budget.charge(asset.size)?;
    let mut source = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
        .open(&asset.source)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.len() != asset.size {
        return Err(Error::Invalid("Source asset changed during export"));
    }
    let mut output = std::fs::File::create_new(path)?;
    let mut hash = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0; 16384];
    loop {
        runner.cancel.check()?;
        let length = source.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        if length as u64 > asset.size.saturating_sub(copied) {
            return Err(Error::Invalid("Source asset changed during export"));
        }
        output.write_all(&buffer[..length])?;
        hash.update(&buffer[..length]);
        copied += length as u64;
    }
    let mut digest = String::with_capacity(64);
    for byte in hash.finalize() {
        use std::fmt::Write as _;
        write!(digest, "{byte:02x}").map_err(|_| Error::Invalid("Cannot encode source checksum"))?;
    }
    if copied != asset.size || digest != asset.oid {
        return Err(Error::Invalid("Source asset changed during export"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
