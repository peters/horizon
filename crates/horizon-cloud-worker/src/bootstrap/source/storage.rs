use super::super::store::{Store, invalid, private, regular, same};
use horizon_cloud_protocol::membership::{Artifact, Receipt, Source};
use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    process::Command,
    time::{Duration, Instant},
};

#[cfg(test)]
thread_local! { static CONTENT_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(in crate::bootstrap) fn content_checks() -> usize {
    CONTENT_CHECKS.with(std::cell::Cell::get)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::bootstrap) enum Boundary {
    Anchored,
    Opened,
    Received,
    Imported,
    Published,
    Synced,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
}
impl Identity {
    fn of(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        private(&meta)?;
        Ok(Self {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    fn require(&self, file: &File) -> io::Result<()> {
        if self != &Self::of(file)? {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    receipt: Receipt,
    descriptor: Source,
    parent: Identity,
    tree: Identity,
    digest: Option<String>,
    published: bool,
}
pub(super) struct Tree<'a> {
    store: &'a Store,
    parent: &'a File,
    allocation: File,
    file: File,
    name: String,
    staging: String,
    bytes: Vec<u8>,
    record: Record,
    destination: bool,
    deadline: Instant,
}
fn directory(parent: &File, name: &str) -> io::Result<File> {
    let file = File::from(openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    private(&file.metadata()?)?;
    Ok(file)
}
fn absent(parent: &File, name: &str) -> io::Result<bool> {
    match rustix::fs::statat(parent, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error.into()),
    }
}
impl<'a> Tree<'a> {
    pub fn open(
        store: &'a Store,
        parent: &'a File,
        receipt: &Receipt,
        descriptor: &Source,
        create: bool,
        deadline: Instant,
    ) -> io::Result<Option<Self>> {
        super::remaining(deadline)?;
        descriptor.validate().map_err(|_| invalid())?;
        let (_, allocation) = store.namespace_anchors()?;
        let name = format!("source-{}.json", receipt.identity.project_id());
        let staging = format!(".source-{}.next", receipt.operation);
        let destination = !absent(parent, "source")?;
        let (bytes, record, file) = if let Some(bytes) = store.read(&name)? {
            let record: Record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if record.version != 1
                || &record.receipt != receipt
                || &record.descriptor != descriptor
                || (record.published && !destination)
                || (destination && !absent(&allocation, &staging)?)
            {
                return Err(invalid());
            }
            let file = if destination {
                directory(parent, "source")?
            } else {
                directory(&allocation, &staging)?
            };
            (bytes, record, file)
        } else {
            if destination || !absent(&allocation, &staging)? {
                return Err(invalid());
            }
            if !create {
                return Ok(None);
            }
            mkdirat(&allocation, staging.as_str(), Mode::RWXU)?;
            let file = directory(&allocation, &staging)?;
            file.sync_all()?;
            allocation.sync_all()?;
            // As with namespace creation, an interruption before this durable
            // inode anchor is deliberately fenced; an existing directory is not adopted.
            let record = Record {
                version: 1,
                receipt: receipt.clone(),
                descriptor: descriptor.clone(),
                parent: Identity::of(parent)?,
                tree: Identity::of(&file)?,
                digest: None,
                published: false,
            };
            let bytes = serde_json::to_vec(&record)?;
            store.write(&name, None, &bytes)?;
            (bytes, record, file)
        };
        let tree = Self {
            store,
            parent,
            allocation,
            file,
            name,
            staging,
            bytes,
            record,
            destination,
            deadline,
        };
        tree.verify()?;
        Ok(Some(tree))
    }
    pub fn record(&self) -> &[u8] {
        &self.bytes
    }
    pub fn published_anchor(&self) -> io::Result<File> {
        self.verify()?;
        if !self.destination || !self.record.published {
            return Err(invalid());
        }
        self.file.try_clone()
    }
    fn verify(&self) -> io::Result<()> {
        super::remaining(self.deadline)?;
        self.store.verify()?;
        self.record.parent.require(self.parent)?;
        self.record.tree.require(&self.file)?;
        let current = if self.destination {
            directory(self.parent, "source")?
        } else {
            directory(&self.allocation, &self.staging)?
        };
        same(&self.file, &current)?;
        if self.store.read(&self.name)?.as_deref() != Some(&self.bytes) {
            return Err(invalid());
        }
        Ok(())
    }
    fn save(&mut self) -> io::Result<()> {
        self.verify()?;
        let bytes = serde_json::to_vec(&self.record)?;
        self.store.write(&self.name, Some(&self.bytes), &bytes)?;
        self.bytes = bytes;
        self.verify()
    }
    fn helper(&self, build: bool) -> io::Result<String> {
        #[cfg(test)]
        if !build {
            CONTENT_CHECKS.with(|count| count.set(count.get() + 1));
        }
        self.verify()?;
        let bytes = super::super::inspection::execute_leased(
            Command::new("/usr/bin/python3")
                .args([
                    "-I",
                    "-c",
                    include_str!("import.py"),
                    if build { "build" } else { "verify" },
                    &self.record.descriptor.revision,
                ])
                .current_dir(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", "/nonexistent")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .env("GIT_TERMINAL_PROMPT", "0"),
            super::remaining(self.deadline)?.min(Duration::from_secs(120)),
            self.store.lease()?,
        )?;
        self.verify()?;
        let digest = String::from_utf8(bytes).map_err(|_| invalid())?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        Ok(digest)
    }
    pub fn validate(&self, settled: bool) -> io::Result<()> {
        self.verify()?;
        if settled && !self.record.published {
            return Err(invalid());
        }
        if (self.destination || self.record.digest.is_some())
            && self.record.digest.as_deref() != Some(&self.helper(false)?)
        {
            return Err(invalid());
        }
        self.verify()
    }
    pub fn receive(
        &self,
        input: &mut impl Read,
        checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
        verify: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        checkpoint(Boundary::Anchored)?;
        verify()?;
        self.verify()?;
        for (name, artifact) in [
            ("pack", &self.record.descriptor.pack),
            ("material.tar", &self.record.descriptor.material),
        ] {
            let mut file = if absent(&self.file, name)? {
                if self.destination || self.record.digest.is_some() {
                    return Err(invalid());
                }
                File::from(openat(
                    &self.file,
                    name,
                    OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )?)
            } else {
                regular(&self.file, name)?
            };
            let old_length = file.metadata()?.len();
            if old_length > artifact.length {
                return Err(invalid());
            }
            checkpoint(Boundary::Opened)?;
            verify()?;
            let writable = !self.destination && self.record.digest.is_none();
            let mut writer = if writable {
                let writer = File::from(openat(
                    &self.file,
                    name,
                    OFlags::WRONLY | OFlags::APPEND | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?);
                same(&file, &writer)?;
                Some(writer)
            } else {
                None
            };
            transfer(input, &mut file, writer.as_mut(), artifact, old_length)?;
            file.sync_all()?;
            same(&file, &regular(&self.file, name)?)?;
            self.verify()?;
            verify()?;
        }
        if input.read(&mut [0])? != 0 {
            return Err(invalid());
        }
        self.file.sync_all()?;
        checkpoint(Boundary::Received)?;
        verify()
    }
    pub fn publish(
        &mut self,
        checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
        verify: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        verify()?;
        if self.record.digest.is_none() {
            if self.destination {
                return Err(invalid());
            }
            self.record.digest = Some(self.helper(true)?);
            verify()?;
            self.save()?;
        }
        checkpoint(Boundary::Imported)?;
        self.validate(false)?;
        verify()?;
        if !self.destination {
            renameat_with(
                &self.allocation,
                self.staging.as_str(),
                self.parent,
                "source",
                RenameFlags::NOREPLACE,
            )?;
            self.destination = true;
            checkpoint(Boundary::Published)?;
        }
        self.verify()?;
        verify()?;
        self.parent.sync_all()?;
        self.allocation.sync_all()?;
        checkpoint(Boundary::Synced)?;
        verify()?;
        if !self.record.published {
            self.record.published = true;
            self.save()?;
        }
        self.store.sync(&self.name)?;
        self.validate(true)?;
        verify()
    }
}
fn transfer(
    input: &mut impl Read,
    existing: &mut File,
    mut writer: Option<&mut File>,
    artifact: &Artifact,
    old_length: u64,
) -> io::Result<()> {
    existing.rewind()?;
    let mut position = 0_u64;
    let mut digest = Sha256::new();
    let mut buffer = [0; 16384];
    let mut old = [0; 16384];
    while position < artifact.length {
        let length = usize::try_from((artifact.length - position).min(buffer.len() as u64)).map_err(|_| invalid())?;
        input.read_exact(&mut buffer[..length])?;
        let overlap = usize::try_from(old_length.saturating_sub(position).min(length as u64)).map_err(|_| invalid())?;
        existing.read_exact(&mut old[..overlap])?;
        if old[..overlap] != buffer[..overlap] {
            return Err(invalid());
        }
        if overlap < length {
            writer
                .as_mut()
                .ok_or_else(invalid)?
                .write_all(&buffer[overlap..length])?;
        }
        digest.update(&buffer[..length]);
        position += length as u64;
    }
    if <[u8; 32]>::from(digest.finalize()) != artifact.sha256 {
        return Err(invalid());
    }
    Ok(())
}
