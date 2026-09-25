use super::super::store::{Store, invalid, private, regular, same};
use super::{Boundary, Owner};
use rustix::fs::{Mode, OFlags, RenameFlags, mkdirat, openat, renameat_with};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    os::unix::fs::MetadataExt,
};

const HEADER: &str = ".namespace-owner.json";
const FLAGS: OFlags = OFlags::RDONLY.union(OFlags::NOFOLLOW).union(OFlags::CLOEXEC);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        if *self != Self::of(file)? {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    owner: Owner,
    identity: Identity,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    anchor: Anchor,
    published: bool,
}

pub(super) struct Location {
    pub record: String,
    pub staging: String,
    pub destination: String,
}

pub(super) fn directory(parent: &File, name: &str) -> io::Result<File> {
    let file = File::from(openat(parent, name, FLAGS | OFlags::DIRECTORY, Mode::empty())?);
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
fn create(parent: &File, name: &str) -> io::Result<File> {
    mkdirat(parent, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)?;
    directory(parent, name)
}
fn names(directory: &File, allowed: &[&str]) -> io::Result<()> {
    for entry in rustix::fs::Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." && !allowed.iter().any(|allowed| allowed.as_bytes() == name) {
            return Err(invalid());
        }
    }
    Ok(())
}

struct Tree<'a> {
    store: &'a Store,
    allocation: &'a File,
    parent: &'a File,
    location: &'a Location,
    record: Record,
    bytes: Vec<u8>,
    file: File,
    at_destination: bool,
}
impl Tree<'_> {
    fn verify(&self) -> io::Result<()> {
        self.store.verify()?;
        self.record.anchor.identity.require(&self.file)?;
        let (parent, name) = if self.at_destination {
            (self.parent, &self.location.destination)
        } else {
            (self.allocation, &self.location.staging)
        };
        same(&self.file, &directory(parent, name)?)?;
        if self.store.read(&self.location.record)?.as_deref() != Some(&self.bytes) {
            return Err(invalid());
        }
        Ok(())
    }
    fn header(&self, creating: bool) -> io::Result<()> {
        let expected = serde_json::to_vec(&self.record.anchor)?;
        let mut file = if creating && absent(&self.file, HEADER)? {
            File::from(openat(
                &self.file,
                HEADER,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?)
        } else {
            regular(&self.file, HEADER)?
        };
        let mut bytes = Vec::new();
        (&mut file).take(expected.len() as u64 + 1).read_to_end(&mut bytes)?;
        if bytes != expected {
            if !creating || !expected.starts_with(&bytes) {
                return Err(invalid());
            }
            // Only an anchored, unpublished staging tree can complete a partial header.
            let mut writable = File::from(openat(
                &self.file,
                HEADER,
                OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            same(&file, &writable)?;
            self.verify()?;
            writable.seek(io::SeekFrom::Start(bytes.len() as u64))?;
            writable.write_all(&expected[bytes.len()..])?;
            writable.sync_all()?;
            same(&writable, &regular(&self.file, HEADER)?)?;
        }
        file.sync_all()?;
        same(&file, &regular(&self.file, HEADER)?)?;
        self.verify()
    }
    fn layout(
        &self,
        children: &[&str],
        creating: bool,
        checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
    ) -> io::Result<()> {
        self.header(creating)?;
        if creating {
            let mut allowed = children.to_vec();
            allowed.push(HEADER);
            names(&self.file, &allowed)?;
        }
        for name in children {
            let child = if creating && absent(&self.file, name)? {
                create(&self.file, name)?
            } else {
                directory(&self.file, name)?
            };
            if creating {
                names(&child, &[])?;
            }
            child.sync_all()?;
            checkpoint(Boundary::Child)?;
            self.verify()?;
            same(&child, &directory(&self.file, name)?)?;
        }
        self.file.sync_all()?;
        self.verify()
    }
}

fn load<'a>(
    store: &'a Store,
    allocation: &'a File,
    parent: &'a File,
    location: &'a Location,
    owner: &Owner,
) -> io::Result<Option<Tree<'a>>> {
    let Some(bytes) = store.read(&location.record)? else {
        if !absent(allocation, &location.staging)? || !absent(parent, &location.destination)? {
            return Err(invalid());
        }
        return Ok(None);
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if record.anchor.owner != *owner {
        return Err(invalid());
    }
    let at_destination = !absent(parent, &location.destination)?;
    if (record.published && !at_destination) || (at_destination && !absent(allocation, &location.staging)?) {
        return Err(invalid());
    }
    let file = if at_destination {
        directory(parent, &location.destination)?
    } else {
        directory(allocation, &location.staging)?
    };
    let tree = Tree {
        store,
        allocation,
        parent,
        location,
        record,
        bytes,
        file,
        at_destination,
    };
    tree.verify()?;
    Ok(Some(tree))
}

pub(super) struct Directories<'a> {
    pub store: &'a Store,
    pub allocation: &'a File,
    pub parent: &'a File,
}
impl Directories<'_> {
    pub fn validate(
        &self,
        location: &Location,
        owner: &Owner,
        children: &[&str],
        settled: bool,
    ) -> io::Result<Option<File>> {
        let Some(tree) = load(self.store, self.allocation, self.parent, location, owner)? else {
            return if settled { Err(invalid()) } else { Ok(None) };
        };
        if settled && !tree.record.published {
            return Err(invalid());
        }
        if tree.at_destination {
            tree.layout(children, false, &mut |_| Ok(()))?;
        }
        Ok(Some(tree.file))
    }
    pub fn ensure(
        &self,
        location: &Location,
        owner: &Owner,
        children: &[&str],
        checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
    ) -> io::Result<File> {
        let mut tree = if let Some(tree) = load(self.store, self.allocation, self.parent, location, owner)? {
            tree
        } else {
            let file = create(self.allocation, &location.staging)?;
            checkpoint(Boundary::Created)?;
            self.store.verify()?;
            same(&file, &directory(self.allocation, &location.staging)?)?;
            file.sync_all()?;
            self.allocation.sync_all()?;
            let record = Record {
                anchor: Anchor {
                    owner: owner.clone(),
                    identity: Identity::of(&file)?,
                },
                published: false,
            };
            let bytes = serde_json::to_vec(&record)?;
            self.store.write(&location.record, None, &bytes)?;
            Tree {
                store: self.store,
                allocation: self.allocation,
                parent: self.parent,
                location,
                record,
                bytes,
                file,
                at_destination: false,
            }
        };
        checkpoint(Boundary::Anchored)?;
        tree.verify()?;
        tree.layout(children, !tree.at_destination, checkpoint)?;
        checkpoint(Boundary::Populated)?;
        tree.layout(children, false, &mut |_| Ok(()))?;
        tree.verify()?;
        if !tree.at_destination {
            renameat_with(
                self.allocation,
                location.staging.as_str(),
                self.parent,
                location.destination.as_str(),
                RenameFlags::NOREPLACE,
            )?;
            tree.at_destination = true;
            checkpoint(Boundary::Published)?;
        }
        tree.verify()?;
        self.parent.sync_all()?;
        self.allocation.sync_all()?;
        checkpoint(Boundary::Synced)?;
        tree.verify()?;
        if !tree.record.published {
            tree.record.published = true;
            let bytes = serde_json::to_vec(&tree.record)?;
            self.store.write(&location.record, Some(&tree.bytes), &bytes)?;
            tree.bytes = bytes;
        }
        self.store.sync(&location.record)?;
        tree.layout(children, false, &mut |_| Ok(()))?;
        tree.verify()?;
        Ok(tree.file)
    }
}
