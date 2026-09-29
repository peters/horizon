//! The source material archive workers import, streamed from verified sources.
use super::super::{Event, command::TIMED_OUT, package_restore::Restored, progress::Progress};
use super::material::{Material, Verified};
use super::{Error, Result, Runner};
use std::{
    collections::BTreeSet,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

/// The bound the `tar` process this replaces ran under.
pub(super) const TIMEOUT: Duration = Duration::from_secs(300);

/// Writes `./`, `manifest.json`, `lfs/`, one `lfs/<oid>` per distinct object, each
/// `module-<index>.pack` from `packs` and, when a package folder was restored, `packages/`
/// with its folders and files. LFS content is read from the local store and
/// verified as it is written, so it is never staged as a second copy. Every write
/// stops on cancellation or once `timeout`, counted from here, has passed; the caller
/// discards the output.
pub(super) fn write(
    material: &Material,
    manifest: &[u8],
    packs: &Path,
    packages: Option<&Restored>,
    output: impl Write,
    runner: &Runner<'_>,
    timeout: Duration,
) -> Result<()> {
    (runner.emit)(Event::Progress(Progress::activity("Pack source dependencies")));
    let mut output = Checked {
        output,
        runner,
        deadline: Instant::now() + timeout,
        failure: None,
    };
    let written = append(material, manifest, packs, packages, &mut output, runner);
    output.failure.take().map_or(written, Err)
}

fn append(
    material: &Material,
    manifest: &[u8],
    packs: &Path,
    packages: Option<&Restored>,
    output: &mut Checked<'_, '_, impl Write>,
    runner: &Runner<'_>,
) -> Result<()> {
    let mut archive = tar::Builder::new(output);
    directory(&mut archive, "./")?;
    archive.append_data(&mut file(manifest.len() as u64), "manifest.json", manifest)?;
    directory(&mut archive, "lfs/")?;
    let mut written = BTreeSet::new();
    for asset in &material.assets {
        if !written.insert(asset.oid.as_str()) {
            continue;
        }
        let mut object = Verified::open(&asset.source, &asset.oid, asset.size, runner)?;
        let appended = archive.append_data(&mut file(asset.size), format!("lfs/{}", asset.oid), &mut object);
        object.finish(appended)?;
    }
    for index in 0..material.modules.len() {
        runner.cancel.check()?;
        let name = format!("module-{index}.pack");
        let pack = std::fs::File::open(packs.join(&name))?;
        let length = pack.metadata()?.len();
        archive.append_data(&mut file(length), name, std::io::Read::take(pack, length))?;
    }
    if let Some(packages) = packages {
        directory(&mut archive, "packages/")?;
        for entry in &packages.entries {
            runner.cancel.check()?;
            let name = format!("packages/{}", entry.name);
            let Some(size) = entry.size else {
                directory(&mut archive, &format!("{name}/"))?;
                continue;
            };
            let mut header = file(size);
            if entry.executable {
                header.set_mode(0o700);
            }
            let content = Exact {
                content: std::fs::File::open(&entry.path)?,
                remaining: size,
            };
            archive.append_data(&mut header, name, content)?;
        }
    }
    archive.into_inner()?.flush()?;
    Ok(())
}

/// Exactly `remaining` bytes of a file listed with that size: one that changed since
/// is refused rather than archived short or long.
struct Exact {
    content: std::fs::File,
    remaining: u64,
}
impl std::io::Read for Exact {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let changed = || std::io::Error::other("A restored package file changed while it was archived");
        if self.remaining == 0 {
            return if self.content.read(&mut [0])? == 0 {
                Ok(0)
            } else {
                Err(changed())
            };
        }
        let limit = buffer.len().min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let read = self.content.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(changed());
        }
        self.remaining -= read as u64;
        Ok(read)
    }
}

/// Checks cancellation and the deadline before each write; a failure is kept so the
/// caller reports it rather than the I/O error the archive writer saw.
struct Checked<'a, 'r, W> {
    output: W,
    runner: &'a Runner<'r>,
    deadline: Instant,
    failure: Option<Error>,
}
impl<W: Write> Checked<'_, '_, W> {
    fn check(&mut self) -> std::io::Result<()> {
        let failure = match self.runner.cancel.check() {
            Err(error) => error.into(),
            Ok(()) if Instant::now() >= self.deadline => Error::Invalid(TIMED_OUT),
            Ok(()) => return Ok(()),
        };
        let error = std::io::Error::other(failure.to_string());
        self.failure = Some(failure);
        Err(error)
    }
}
impl<W: Write> Write for Checked<'_, '_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.check()?;
        self.output.write(buffer)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.output.flush()
    }
}

fn directory(archive: &mut tar::Builder<impl Write>, name: &str) -> Result<()> {
    let mut header = header(tar::EntryType::Directory, 0o700);
    header.set_size(0);
    archive.append_data(&mut header, name, std::io::empty())?;
    Ok(())
}

fn file(size: u64) -> tar::Header {
    let mut header = header(tar::EntryType::Regular, 0o600);
    header.set_size(size);
    header
}

fn header(kind: tar::EntryType, mode: u32) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_mtime(0);
    header
}
