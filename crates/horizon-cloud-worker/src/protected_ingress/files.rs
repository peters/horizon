use super::refused;
use horizon_cloud_protocol::exposure::ingress::MAX_RECEIPT_BYTES;
use rustix::fs::{FileType, Mode, OFlags, StatVfsMountFlags, fstatvfs, openat};
use std::{
    fs::File,
    io,
    os::unix::fs::{FileExt, MetadataExt},
};

const FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Stamp {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u64,
    pub size: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
}
impl Stamp {
    fn read(file: &File) -> io::Result<Self> {
        let m = file.metadata()?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            links: m.nlink(),
            size: m.size(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        })
    }
    pub(super) fn validate(self, directory: bool) -> io::Result<()> {
        let (kind, permissions) = if directory {
            (FileType::Directory, 0o700)
        } else {
            (FileType::RegularFile, 0o400)
        };
        if self.uid != 0
            || FileType::from_raw_mode(self.mode) != kind
            || self.mode & 0o7777 != permissions
            || (!directory && (self.links != 1 || self.size == 0 || self.size > MAX_RECEIPT_BYTES as u64))
        {
            return Err(refused());
        }
        Ok(())
    }
}

pub(super) struct ReceiptFiles {
    directory: File,
    receipt: File,
    directory_stamp: Stamp,
    receipt_stamp: Stamp,
}
impl ReceiptFiles {
    pub fn open() -> io::Result<Self> {
        let directory = directory()?;
        let receipt = receipt(&directory)?;
        let directory_stamp = Stamp::read(&directory)?;
        let receipt_stamp = Stamp::read(&receipt)?;
        directory_stamp.validate(true)?;
        receipt_stamp.validate(false)?;
        Ok(Self {
            directory,
            receipt,
            directory_stamp,
            receipt_stamp,
        })
    }
    pub fn read(&self) -> io::Result<Vec<u8>> {
        self.verify()?;
        let mut bytes = vec![0; MAX_RECEIPT_BYTES + 1];
        let mut length = 0;
        while length < bytes.len() {
            let n = self.receipt.read_at(&mut bytes[length..], length as u64)?;
            if n == 0 {
                break;
            }
            length += n;
        }
        if length as u64 != self.receipt_stamp.size {
            return Err(refused());
        }
        bytes.truncate(length);
        self.verify()?;
        Ok(bytes)
    }
    fn verify(&self) -> io::Result<()> {
        let current_directory = directory()?;
        let current_receipt = receipt(&current_directory)?;
        for (file, original, is_directory) in [
            (&self.directory, self.directory_stamp, true),
            (&current_directory, self.directory_stamp, true),
            (&self.receipt, self.receipt_stamp, false),
            (&current_receipt, self.receipt_stamp, false),
        ] {
            let stamp = Stamp::read(file)?;
            stamp.validate(is_directory)?;
            readonly(fstatvfs(file)?.f_flag)?;
            if stamp != original {
                return Err(refused());
            }
        }
        Ok(())
    }
}

fn directory() -> io::Result<File> {
    let mut file = File::open("/")?;
    for name in ["run", "horizon-host-ingress"] {
        let m = file.metadata()?;
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err(refused());
        }
        file = File::from(openat(&file, name, FLAGS | OFlags::DIRECTORY, Mode::empty())?);
    }
    Ok(file)
}
fn receipt(directory: &File) -> io::Result<File> {
    Ok(File::from(openat(directory, "receipt.json", FLAGS, Mode::empty())?))
}

pub(super) fn readonly(flags: StatVfsMountFlags) -> io::Result<()> {
    if !flags.contains(StatVfsMountFlags::RDONLY) {
        return Err(refused());
    }
    Ok(())
}
