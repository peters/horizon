//! The source material archive workers import, streamed from verified sources.
use super::super::{Event, command::TIMED_OUT, progress::Progress};
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

/// Writes `./`, `manifest.json`, `lfs/`, one `lfs/<oid>` per distinct object and each
/// `module-<index>.pack` from `packs`. LFS content is read from the local store and
/// verified as it is written, so it is never staged as a second copy. Every write
/// stops on cancellation or once `deadline` passes; the caller discards the output.
pub(super) fn write(
    material: &Material,
    manifest: &[u8],
    packs: &Path,
    output: impl Write,
    runner: &Runner<'_>,
    deadline: Instant,
) -> Result<()> {
    (runner.emit)(Event::Progress(Progress::activity("Pack source dependencies")));
    let mut output = Checked {
        output,
        runner,
        deadline,
        failure: None,
    };
    let written = append(material, manifest, packs, &mut output, runner);
    output.failure.take().map_or(written, Err)
}

fn append(
    material: &Material,
    manifest: &[u8],
    packs: &Path,
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
    archive.into_inner()?.flush()?;
    Ok(())
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
