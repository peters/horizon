use super::{
    CHECKOUT, GitPreparation, GitPreparationError as Error, GitPreparationResponse, GitPreparationState as State,
    REQUEST_LIMIT, git::Commands,
};
use crate::repository_overlay::reader::{SelectedRepositoryNode, SelectedRepositoryReader};
use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, mkdirat, openat2};
use std::{
    fs::File,
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const PRIVATE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);
const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

fn open_directory(path: &Path) -> Result<File, Error> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map(File::from)
    .map_err(|_| Error::UnsafeRoot)
}

fn record_identity(meta: &std::fs::Metadata) -> (u64, u64, u32, u64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.mode(),
        meta.len(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

/// A held inode plus a rechecked pathname. Git remains path-based: stable trusted
/// ownership/ancestry is a precondition, not protection from a malicious worker owner.
pub(super) struct Directory {
    handle: File,
    pub path: PathBuf,
    private: bool,
}

impl Directory {
    fn open_mode(path: &Path, private: bool) -> Result<Self, Error> {
        let handle = open_directory(path)?;
        let directory = Self {
            handle,
            path: path.to_owned(),
            private,
        };
        directory.verify()?;
        Ok(directory)
    }

    pub fn verify(&self) -> Result<(), Error> {
        let held = self.handle.metadata().map_err(|_| Error::UnsafeRoot)?;
        let actual = open_directory(&self.path)?.metadata().map_err(|_| Error::UnsafeRoot)?;
        if held.uid() != rustix::process::geteuid().as_raw()
            || held.mode() & 0o7022 != 0
            || self.private && held.mode() & 0o7777 != 0o700
            || held.nlink() == 0
            || (held.dev(), held.ino()) != (actual.dev(), actual.ino())
        {
            return Err(Error::UnsafeRoot);
        }
        Ok(())
    }

    fn child(&self, name: &str, create: bool) -> Result<Option<Self>, Error> {
        self.verify()?;
        if create {
            mkdirat(&self.handle, name, PRIVATE).map_err(|_| Error::Conflict)?;
        }
        let handle = match openat2(
            &self.handle,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        ) {
            Ok(handle) => File::from(handle),
            Err(rustix::io::Errno::NOENT) if !create => return Ok(None),
            Err(_) => return Err(Error::UnsafeRoot),
        };
        let child = Self {
            handle,
            path: self.path.join(name),
            private: true,
        };
        child.verify()?;
        if create {
            child
                .handle
                .sync_all()
                .and_then(|()| self.handle.sync_all())
                .map_err(|_| Error::Storage)?;
        }
        self.verify()?;
        Ok(Some(child))
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        self.verify()?;
        let mut file = File::from(
            openat2(
                &self.handle,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
                CONFINED,
            )
            .map_err(|_| Error::Conflict)?,
        );
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| self.handle.sync_all())
            .map_err(|_| Error::Storage)?;
        self.verify()?;
        if self.read(name)?.as_deref() != Some(bytes) {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, Error> {
        self.verify()?;
        let file = match openat2(
            &self.handle,
            name,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(Error::Conflict),
        };
        let meta = file.metadata().map_err(|_| Error::Conflict)?;
        if !meta.is_file()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o7777 != 0o600
            || meta.nlink() != 1
        {
            return Err(Error::Conflict);
        }
        let value = SelectedRepositoryReader::open(&self.path)
            .and_then(|reader| reader.read(name, REQUEST_LIMIT))
            .map_err(|_| Error::Conflict)?;
        let current = File::from(
            openat2(
                &self.handle,
                name,
                OFlags::PATH | OFlags::CLOEXEC,
                Mode::empty(),
                CONFINED,
            )
            .map_err(|_| Error::Conflict)?,
        )
        .metadata()
        .map_err(|_| Error::Conflict)?;
        if record_identity(&meta) != record_identity(&current) {
            return Err(Error::Conflict);
        }
        self.verify()?;
        match value {
            SelectedRepositoryNode::File { bytes, .. } => Ok(Some(bytes)),
            SelectedRepositoryNode::Symlink { .. } => Err(Error::Conflict),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    request: GitPreparation,
    device: u64,
    inode: u64,
}

pub(super) fn execute(
    parent: &Path,
    request: &GitPreparation,
    observe: bool,
    cancelled: &dyn Fn() -> bool,
    git: &mut impl Commands,
) -> GitPreparationResponse {
    let mut response = GitPreparationResponse::failure(Error::Storage);
    let result = (|| {
        let bytes = request.encode()?;
        let parent = Directory::open_mode(parent, false)?;
        let worker = Directory::open_mode(&parent.path.join(".horizon-worker"), true)?;
        let tasks = Directory::open_mode(&parent.path.join("horizon"), true)?;
        let existing = worker.child("git-workspace", false)?;
        if let Some(slot) = existing {
            response.state = State::ClaimedUnknown;
            if slot.read("claim.json")?.as_deref() != Some(&bytes) {
                return Err(Error::Conflict);
            }
            if let Some(completion) = slot.read("complete.json")? {
                let completion: Completion = serde_json::from_slice(&completion).map_err(|_| Error::Conflict)?;
                let checkout = tasks.child("repository", false)?.ok_or(Error::Conflict)?;
                let meta = checkout.handle.metadata().map_err(|_| Error::Conflict)?;
                if completion.request != *request || (completion.device, completion.inode) != (meta.dev(), meta.ino()) {
                    return Err(Error::Conflict);
                }
                response.state = State::Complete;
                response.checkout = Some(CHECKOUT);
            }
            parent.verify()?;
            worker.verify()?;
            tasks.verify()?;
            return Ok(());
        }
        if observe {
            response.state = State::Absent;
            return Ok(());
        }
        if cancelled() {
            return Err(Error::Interrupted);
        }
        // Exclusive mkdir is the irreversible replay barrier even if claim writing fails.
        response.state = State::ClaimedUnknown;
        let slot = worker.child("git-workspace", true)?.ok_or(Error::Storage)?;
        slot.write("claim.json", &bytes)?;
        let checkout = tasks.child("repository", true)?.ok_or(Error::Storage)?;
        super::git::prepare(git, &checkout.path, request, cancelled, &|| {
            if cancelled() {
                return Err(Error::Interrupted);
            }
            parent.verify()?;
            worker.verify()?;
            tasks.verify()?;
            slot.verify()?;
            checkout.verify()
        })?;
        if cancelled() {
            return Err(Error::Interrupted);
        }
        let meta = checkout.handle.metadata().map_err(|_| Error::Storage)?;
        let completion = Completion {
            request: request.clone(),
            device: meta.dev(),
            inode: meta.ino(),
        };
        slot.write(
            "complete.json",
            &serde_json::to_vec(&completion).map_err(|_| Error::Storage)?,
        )?;
        parent.verify()?;
        worker.verify()?;
        tasks.verify()?;
        checkout.verify()?;
        response.state = State::Complete;
        response.checkout = Some(CHECKOUT);
        Ok(())
    })();
    response.reason = result.err();
    response
}
