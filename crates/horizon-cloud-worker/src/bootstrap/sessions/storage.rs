use super::super::{
    inspection, source,
    store::{Store, invalid, private, same},
};
use horizon_cloud_protocol::membership::Receipt;
use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    process::Command,
    time::Instant,
};

const CHILDREN: &[&str] = &["checkout", "home", "runtime", "logs", "tools"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::bootstrap) enum Boundary {
    Anchored,
    Started,
    Built,
    Ready,
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
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Empty,
    Building,
    Ready,
    Published,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    receipt: Receipt,
    session: horizon_cloud_protocol::membership::SessionId,
    parent: Identity,
    tree: Identity,
    phase: Phase,
    children: Vec<Identity>,
    digest: Option<String>,
}
pub(super) struct Tree<'a> {
    store: &'a Store,
    parent: &'a File,
    allocation: File,
    file: File,
    name: String,
    staging: String,
    destination: String,
    bytes: Vec<u8>,
    record: Record,
    exposed: bool,
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
        session: horizon_cloud_protocol::membership::SessionId,
        create: bool,
        deadline: Instant,
    ) -> io::Result<Option<Self>> {
        source::remaining(deadline)?;
        let (_, allocation) = store.namespace_anchors()?;
        let name = format!("session-{session}.json");
        let staging = format!(".session-{session}.next");
        let destination = session.to_string();
        let exposed = !absent(parent, &destination)?;
        let (bytes, record, file) = if let Some(bytes) = store.read(&name)? {
            let record: Record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if record.version != 1
                || &record.receipt != receipt
                || record.session != session
                || (exposed
                    && (!matches!(record.phase, Phase::Ready | Phase::Published) || !absent(&allocation, &staging)?))
                || (!exposed && record.phase == Phase::Published)
            {
                return Err(invalid());
            }
            let file = if exposed {
                directory(parent, &destination)?
            } else {
                directory(&allocation, &staging)?
            };
            (bytes, record, file)
        } else {
            if exposed || !absent(&allocation, &staging)? {
                return Err(invalid());
            }
            if !create {
                return Ok(None);
            }
            mkdirat(&allocation, staging.as_str(), Mode::RWXU)?;
            let file = directory(&allocation, &staging)?;
            file.sync_all()?;
            allocation.sync_all()?;
            let record = Record {
                version: 1,
                receipt: receipt.clone(),
                session,
                parent: Identity::of(parent)?,
                tree: Identity::of(&file)?,
                phase: Phase::Empty,
                children: Vec::new(),
                digest: None,
            };
            let bytes = serde_json::to_vec(&record)?;
            store.write(&name, None, &bytes)?;
            (bytes, record, file)
        };
        if matches!(record.phase, Phase::Ready | Phase::Published)
            && (record.children.len() != CHILDREN.len()
                || record.digest.as_ref().is_none_or(|digest| digest.len() != 64))
        {
            return Err(invalid());
        }
        let tree = Self {
            store,
            parent,
            allocation,
            file,
            name,
            staging,
            destination,
            bytes,
            record,
            exposed,
            deadline,
        };
        tree.verify()?;
        Ok(Some(tree))
    }
    fn verify(&self) -> io::Result<()> {
        source::remaining(self.deadline)?;
        self.store.verify()?;
        self.record.parent.require(self.parent)?;
        self.record.tree.require(&self.file)?;
        for (name, identity) in CHILDREN.iter().zip(&self.record.children) {
            identity.require(&directory(&self.file, name)?)?;
        }
        let (_, allocation) = self.store.namespace_anchors()?;
        same(&self.allocation, &allocation)?;
        let current = if self.exposed {
            directory(self.parent, &self.destination)?
        } else {
            directory(&self.allocation, &self.staging)?
        };
        same(&self.file, &current)?;
        if self.store.read(&self.name)?.as_deref() != Some(&self.bytes) {
            return Err(invalid());
        }
        Ok(())
    }
    fn save(&mut self, phase: Phase) -> io::Result<()> {
        self.verify()?;
        self.record.phase = phase;
        let bytes = serde_json::to_vec(&self.record)?;
        self.store.write(&self.name, Some(&self.bytes), &bytes)?;
        self.bytes = bytes;
        self.verify()
    }
    pub fn validate(&self, settled: bool) -> io::Result<()> {
        self.verify()?;
        if settled && (!self.exposed || self.record.phase != Phase::Published) {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn published_children(&self) -> io::Result<Vec<File>> {
        self.validate(true)?;
        CHILDREN.iter().map(|name| directory(&self.file, name)).collect()
    }
    fn helper(&self, source: &File, revision: &str, build: bool) -> io::Result<String> {
        let destination = format!("/proc/{}/fd/{}", std::process::id(), self.file.as_raw_fd());
        let output = inspection::execute_leased(
            Command::new("/usr/bin/python3")
                .args([
                    "-I",
                    "-c",
                    include_str!("checkout.py"),
                    &destination,
                    if build { "build" } else { "verify" },
                    revision,
                    &self.record.session.to_string(),
                    &self.record.receipt.identity.project_id().to_string(),
                ])
                .current_dir(format!("/proc/self/fd/{}", source.as_raw_fd()))
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", "/nonexistent")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_NO_REPLACE_OBJECTS", "1")
                .env("GIT_TERMINAL_PROMPT", "0"),
            source::remaining(self.deadline)?,
            self.store.lease()?,
        )?;
        let digest = String::from_utf8(output).map_err(|_| invalid())?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid());
        }
        Ok(digest)
    }

    pub fn prepare(
        &mut self,
        source: &File,
        revision: &str,
        checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
        verify: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        self.verify()?;
        verify()?;
        checkpoint(Boundary::Anchored)?;
        if self.record.phase == Phase::Empty {
            self.save(Phase::Building)?;
            checkpoint(Boundary::Started)?;
            self.record.digest = Some(self.helper(source, revision, true)?);
            self.record.children = CHILDREN
                .iter()
                .map(|name| Identity::of(&directory(&self.file, name)?))
                .collect::<io::Result<_>>()?;
            checkpoint(Boundary::Built)?;
            verify()?;
            self.save(Phase::Ready)?;
        }
        if self.record.phase == Phase::Building {
            return Err(invalid());
        }
        checkpoint(Boundary::Ready)?;
        self.verify()?;
        verify()?;
        if !self.exposed {
            if self.record.digest.as_deref() != Some(&self.helper(source, revision, false)?) {
                return Err(invalid());
            }
            self.store.sync(&self.name)?;
            self.verify()?;
            verify()?;
            renameat_with(
                &self.allocation,
                self.staging.as_str(),
                self.parent,
                self.destination.as_str(),
                RenameFlags::NOREPLACE,
            )?;
            self.exposed = true;
            checkpoint(Boundary::Published)?;
        }
        self.verify()?;
        verify()?;
        self.parent.sync_all()?;
        self.allocation.sync_all()?;
        checkpoint(Boundary::Synced)?;
        verify()?;
        self.save(Phase::Published)?;
        self.store.sync(&self.name)?;
        self.validate(true)
    }
}
