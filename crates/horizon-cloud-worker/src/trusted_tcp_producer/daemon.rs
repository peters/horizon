use super::{Error, live, procfs, whois};
use rustix::{
    event::{PollFd, PollFlags, Timespec},
    process::{PidfdFlags, pidfd_open},
};
use std::{
    fs::{self, File},
    net::SocketAddr,
    os::{
        fd::{AsFd, OwnedFd},
        unix::fs::{FileTypeExt, MetadataExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::UnixStream,
    sync::Semaphore,
    time::{Instant, timeout_at},
};

const CLI: &str = "/usr/local/bin/tailscale";
const EXECUTABLE: &str = "/usr/local/bin/tailscaled";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    device: u64,
    inode: u64,
}
impl Key {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    start: u64,
    executable: Key,
    network: Key,
    pid_namespace: Key,
}

pub(super) struct Daemon {
    unix: UnixStream,
    pid: rustix::process::Pid,
    pidfd: OwnedFd,
    original: Stamp,
    socket: PathBuf,
    socket_key: Key,
    executable: File,
    cli: File,
    children: Arc<Semaphore>,
}

impl Daemon {
    pub async fn pin(path: &str, until: Instant) -> Result<Self, Error> {
        if rustix::fs::statfs("/proc")?.f_type != rustix::fs::PROC_SUPER_MAGIC {
            return Err(Error::Custody);
        }
        let executable = trusted_file(Path::new(EXECUTABLE))?;
        let cli = trusted_file(Path::new(CLI))?;
        let socket_key = socket(path)?;
        let unix = timeout_at(until, UnixStream::connect(path))
            .await
            .map_err(|_| Error::Deadline)??;
        let peer = rustix::net::sockopt::socket_peercred(&unix)?;
        if !peer.uid.is_root() || peer.gid.as_raw() != 0 {
            return Err(Error::Custody);
        }
        let original = stamp(peer.pid)?;
        let pidfd = pidfd_open(peer.pid, PidfdFlags::empty())?;
        if original.executable != Key::of(&executable.metadata()?) {
            return Err(Error::Custody);
        }
        let result = Self {
            unix,
            pid: peer.pid,
            pidfd,
            original,
            socket: path.into(),
            socket_key,
            executable,
            cli,
            children: Arc::new(Semaphore::new(super::MAX_PENDING)),
        };
        result.verify(until)?;
        Ok(result)
    }

    pub fn verify(&self, until: Instant) -> Result<(), Error> {
        self.verify_process(until)?;
        if socket(&self.socket)? != self.socket_key {
            return Err(Error::Custody);
        }
        live(until)
    }

    fn verify_process(&self, until: Instant) -> Result<(), Error> {
        live(until)?;
        let mut events = [PollFd::new(&self.pidfd, PollFlags::IN)];
        if rustix::event::poll(&mut events, Some(&Timespec { tv_sec: 0, tv_nsec: 0 }))? != 0 {
            return Err(Error::Custody);
        }
        let peer = rustix::net::sockopt::socket_peercred(&self.unix)?;
        if peer.pid != self.pid
            || !peer.uid.is_root()
            || peer.gid.as_raw() != 0
            || stamp(self.pid)? != self.original
            || Key::of(&trusted_file(Path::new(EXECUTABLE))?.metadata()?) != Key::of(&self.executable.metadata()?)
            || Key::of(&trusted_file(Path::new(CLI))?.metadata()?) != Key::of(&self.cli.metadata()?)
        {
            return Err(Error::Custody);
        }
        live(until)
    }

    pub fn prove(&self, stream: &impl AsFd, until: Instant) -> Result<(), Error> {
        self.verify(until)?;
        verify_peer(stream, self.pid)?;
        self.verify(until)
    }

    #[cfg(test)]
    pub async fn prove_test_proxy(&self, stream: &tokio::net::TcpStream, until: Instant) -> Result<(), Error> {
        self.verify(until)?;
        super::tests::prove_proxy(stream, self.pid, until).await?;
        self.verify(until)
    }

    async fn command(&self, args: &[String], until: Instant) -> Result<Vec<u8>, Error> {
        use std::os::fd::AsRawFd;
        self.verify(until)?;
        let mut command = tokio::process::Command::new(format!("/proc/self/fd/{}", self.cli.as_raw_fd()));
        command
            .arg(format!("--socket={}", self.socket.display()))
            .args(args)
            .env_clear();
        let result = whois::run(command, until, Arc::clone(&self.children)).await?;
        self.verify(until)?;
        Ok(result)
    }

    pub async fn generation(&self, until: Instant) -> Result<whois::Generation, Error> {
        let args = ["status".into(), "--json".into(), "--peers=false".into()];
        whois::generation(&self.command(&args, until).await?)
    }

    pub async fn retire(self, until: Instant) -> Result<(), Error> {
        let result = drain_children(Arc::clone(&self.children), until).await;
        drop(self);
        result
    }

    pub async fn whois(&self, source: SocketAddr, until: Instant) -> Result<whois::Identity, Error> {
        let args = [
            "whois".into(),
            "--json".into(),
            "--proto=tcp".into(),
            source.to_string(),
        ];
        whois::identity(&self.command(&args, until).await?, source.ip())
    }
}

pub(super) fn verify_peer(stream: &impl AsFd, pid: rustix::process::Pid) -> Result<(), Error> {
    let peer = rustix::net::sockopt::socket_peercred(stream)?;
    if peer.pid != pid || !peer.uid.is_root() || peer.gid.as_raw() != 0 {
        return Err(Error::Custody);
    }
    Ok(())
}

/// A supervisor returns its permit only after kill/wait, including caller cancellation.
pub(super) async fn drain_children(children: Arc<Semaphore>, until: Instant) -> Result<(), Error> {
    let count = u32::try_from(super::MAX_PENDING).map_err(|_| Error::Encoding)?;
    let permits = timeout_at(until, children.acquire_many_owned(count))
        .await
        .map_err(|_| Error::Deadline)?
        .map_err(|_| Error::Custody)?;
    drop(permits);
    Ok(())
}

fn stamp(pid: rustix::process::Pid) -> Result<Stamp, Error> {
    let root = PathBuf::from(format!("/proc/{}", pid.as_raw_nonzero()));
    let status = procfs::bounded(root.join("status"), 16 * 1024)?;
    let status = std::str::from_utf8(&status).map_err(|_| Error::Encoding)?;
    let ids: Vec<_> = status
        .lines()
        .find_map(|s| s.strip_prefix("Uid:"))
        .ok_or(Error::Custody)?
        .split_whitespace()
        .collect();
    if ids != ["0", "0", "0", "0"] {
        return Err(Error::Custody);
    }
    let bytes = procfs::bounded(root.join("stat"), 4096)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| Error::Encoding)?;
    let fields: Vec<_> = text
        .rsplit_once(") ")
        .ok_or(Error::Encoding)?
        .1
        .split_whitespace()
        .collect();
    if fields.first().is_none_or(|s| matches!(*s, "Z" | "X")) {
        return Err(Error::Custody);
    }
    let network = Key::of(&fs::metadata(root.join("ns/net"))?);
    let pid_namespace = Key::of(&fs::metadata(root.join("ns/pid"))?);
    if network != Key::of(&fs::metadata("/proc/self/ns/net")?)
        || pid_namespace != Key::of(&fs::metadata("/proc/self/ns/pid")?)
    {
        return Err(Error::Custody);
    }
    Ok(Stamp {
        start: fields
            .get(19)
            .ok_or(Error::Encoding)?
            .parse()
            .map_err(|_| Error::Encoding)?,
        executable: Key::of(&fs::metadata(root.join("exe"))?),
        network,
        pid_namespace,
    })
}

pub(super) fn ancestors(path: &Path) -> Result<(), Error> {
    if !path.is_absolute() {
        return Err(Error::Custody);
    }
    for directory in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(Error::Custody);
        }
    }
    Ok(())
}
fn trusted_file(path: &Path) -> Result<File, Error> {
    ancestors(path)?;
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 || metadata.mode() & 0o111 == 0 {
        return Err(Error::Custody);
    }
    Ok(file)
}
fn socket(path: impl AsRef<Path>) -> Result<Key, Error> {
    let path = path.as_ref();
    ancestors(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != 0 {
        return Err(Error::Custody);
    }
    if path.starts_with("/run/horizon-tailnet") && fs::metadata("/run/horizon-tailnet")?.mode() & 0o077 != 0 {
        return Err(Error::Custody);
    }
    Ok(Key::of(&metadata))
}
