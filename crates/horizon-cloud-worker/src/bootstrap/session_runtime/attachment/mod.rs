//! Signed transport for one supervisor-owned endpoint; never starts a server.
mod bridge;
mod terminal;
use super::super::{
    membership,
    recovery::{BOOTSTRAP, Bootstrap, decode},
    runtime::Runtime,
    sessions,
    store::{Store, invalid, open_directory},
};
use super::{
    process::Identity,
    records::{Endpoint, Record},
    supervisor,
};
use horizon_cloud_protocol::{
    bootstrap::RecoveryRequest,
    membership::State,
    session_attachment::{self, Request},
    session_runtime::Status,
    signed::{Action, SignedIntent, Target},
};
use rustix::{
    event::{PollFd, PollFlags, Timespec},
    fs::{Mode, OFlags, openat},
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType},
};
use std::{
    fs::{self, File},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(in crate::bootstrap) fn run(child: bool) -> io::Result<()> {
    if child {
        return terminal::child();
    }
    let mut arguments = std::env::args().skip(2);
    let encoded = arguments.next().ok_or_else(invalid)?;
    if arguments.next().is_some() {
        return Err(invalid());
    }
    let request = session_attachment::decode(&encoded).map_err(|_| invalid())?;
    // Require a real terminal without reading any user bytes during handoff.
    rustix::termios::tcgetattr(io::stdin())?;
    rustix::termios::tcgetattr(io::stdout())?;
    attach(&request)
}
fn eligible(store: &Store, request: &RecoveryRequest) -> io::Result<Record> {
    eligible_with(store, &Runtime::captured()?, request)
}
fn eligible_with(store: &Store, runtime: &Runtime, request: &RecoveryRequest) -> io::Result<Record> {
    let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
    bootstrap.validate(store, runtime)?;
    let (_, manifest) = membership::load(store, &bootstrap)?;
    let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| invalid())?;
    let intent = signed
        .verify(&bootstrap.startup.controller, request.payload.as_bytes())
        .map_err(|_| invalid())?;
    let Target::Project { identity } = intent.target() else {
        return Err(invalid());
    };
    let query: Request = decode(request.payload.as_bytes())?;
    if intent.action() != Action::AttachProjectSession
        || intent.expected_revision() != manifest.revision
        || query.startup != bootstrap.startup
        || query.worker_id != bootstrap.worker_id
        || query.session_id.is_nil()
    {
        return Err(invalid());
    }
    let member = manifest
        .members
        .iter()
        .find(|m| &m.identity == identity)
        .ok_or_else(invalid)?;
    if member.state != State::Importing
        || !member.preparations.contains(&query.session_id)
        || !member.launches.contains(&query.session_id)
        || member.stops.contains(&query.session_id)
        || member.capabilities.desktop
        || member.capabilities.browser_tools()
        || !member.ports.is_empty()
        || !member
            .sessions
            .iter()
            .any(|s| s.id == query.session_id && s.agent == horizon_cloud::Agent::Claude)
    {
        return Err(invalid());
    }
    let (record, _) = Record::load(store, &manifest, identity, query.session_id)?.ok_or_else(invalid)?;
    if record.launch.operation != query.launch
        || record.endpoint.is_none()
        || !matches!(record.observed(), Status::Running | Status::Exited { .. })
    {
        return Err(invalid());
    }
    sessions::published(store, &manifest, identity, query.session_id)?;
    Ok(record)
}
fn attach(request: &RecoveryRequest) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let store = supervisor::open(deadline)?;
    let record = eligible(&store, request)?;
    let endpoint = record.endpoint.as_ref().ok_or_else(invalid)?;
    let directory = endpoint_directory(&store, &record)?;
    let upstream = connect(&descriptor(&directory).join("s"))?;
    require_peer(&upstream, &endpoint.server)?;
    endpoint_directory(&store, &record)?;
    drop(store);
    let proxy = Proxy::new()?;
    let mut terminal = terminal::Terminal::spawn(&proxy.socket(), &endpoint.pane_id)?;
    let client = proxy.accept(deadline, &mut terminal.child)?;
    require_peer(
        &client,
        &Identity::capture(i32::try_from(terminal.child.id()).map_err(|_| invalid())?)?,
    )?;
    let store = supervisor::open(deadline)?;
    let current = eligible(&store, request)?;
    if current.nonce != record.nonce || current.endpoint != record.endpoint || current.supervisor != record.supervisor {
        return Err(invalid());
    }
    endpoint_directory(&store, &current)?;
    require_peer(&upstream, &endpoint.server)?;
    if Instant::now() >= deadline {
        return Err(invalid());
    }
    // This is the gate-open decision, ordered with terminal stop by the same
    // allocation lock. No real terminal bytes have been read or forwarded yet.
    drop(store);
    terminal.relay(&client, &upstream)
}
fn descriptor(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()))
}
fn endpoint_directory(store: &Store, record: &Record) -> io::Result<File> {
    let Endpoint {
        directory_device,
        directory_inode,
        socket_device,
        socket_inode,
        server,
        pane_id,
        ..
    } = record.endpoint.as_ref().ok_or_else(invalid)?;
    if !server.alive() || !valid_pane(pane_id) {
        return Err(invalid());
    }
    let parent = open_directory(store.path())?;
    let directory = File::from(openat(
        &parent,
        format!("r-{}", record.nonce),
        OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let meta = directory.metadata()?;
    let socket = fs::symlink_metadata(descriptor(&directory).join("s"))?;
    if meta.dev() != *directory_device
        || meta.ino() != *directory_inode
        || meta.mode() & 0o077 != 0
        || socket.dev() != *socket_device
        || socket.ino() != *socket_inode
        || !socket.file_type().is_socket()
    {
        return Err(invalid());
    }
    store.verify()?;
    Ok(directory)
}
pub(super) fn valid_pane(value: &str) -> bool {
    value.len() > 1
        && value.len() <= 20
        && value.starts_with('%')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_digit)
}
fn require_peer(stream: &UnixStream, identity: &Identity) -> io::Result<()> {
    let peer = rustix::net::sockopt::socket_peercred(stream)?;
    if peer.pid.as_raw_nonzero().get() != identity.pid || peer.uid != rustix::process::getuid() || !identity.alive() {
        return Err(invalid());
    }
    Ok(())
}
fn connect(path: &Path) -> io::Result<UnixStream> {
    let socket = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )?;
    // A full Unix listener backlog returns EAGAIN. Fail boundedly rather than
    // waiting with the allocation lock or silently retrying a replaced endpoint.
    rustix::net::connect(&socket, &SocketAddrUnix::new(path)?)?;
    Ok(UnixStream::from(socket))
}
struct Proxy {
    path: PathBuf,
    directory: File,
    listener: UnixListener,
    socket_device: u64,
    socket_inode: u64,
}
impl Proxy {
    fn new() -> io::Result<Self> {
        let temporary = tempfile::Builder::new()
            .prefix("horizon-attach-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let directory = open_directory(temporary.path())?;
        let socket = descriptor(&directory).join("s");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let meta = fs::symlink_metadata(&socket)?;
        Ok(Self {
            path: temporary.keep(),
            directory,
            listener,
            socket_device: meta.dev(),
            socket_inode: meta.ino(),
        })
    }
    fn socket(&self) -> PathBuf {
        descriptor(&self.directory).join("s")
    }
    fn accept(&self, deadline: Instant, child: &mut std::process::Child) -> io::Result<UnixStream> {
        loop {
            if Instant::now() >= deadline || child.try_wait()?.is_some() {
                return Err(invalid());
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    return Ok(stream);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            let mut polls = [PollFd::new(&self.listener, PollFlags::IN)];
            rustix::event::poll(
                &mut polls,
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 100_000_000,
                }),
            )?;
        }
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        if fs::symlink_metadata(self.socket())
            .is_ok_and(|m| m.dev() == self.socket_device && m.ino() == self.socket_inode)
        {
            let _ = rustix::fs::unlinkat(&self.directory, "s", rustix::fs::AtFlags::empty());
        }
        if let (Ok(saved), Ok(current)) = (self.directory.metadata(), fs::symlink_metadata(&self.path))
            && saved.dev() == current.dev()
            && saved.ino() == current.ino()
        {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

#[cfg(test)]
pub(in crate::bootstrap) fn validate_for_test(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
) -> io::Result<()> {
    eligible_with(store, runtime, request).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
    };

    #[test]
    fn connected_endpoint_does_not_follow_a_replaced_socket_and_rejects_wrong_peer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let mut connection = connect(&path).unwrap();
        require_peer(&connection, &Identity::current().unwrap()).unwrap();
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let foreign = Identity::capture(i32::try_from(child.id()).unwrap()).unwrap();
        assert!(require_peer(&connection, &foreign).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        fs::remove_file(&path).unwrap();
        let replacement = UnixListener::bind(&path).unwrap();
        replacement.set_nonblocking(true).unwrap();
        connection.write_all(b"original endpoint").unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        accepted.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut bytes = [0; 17];
        accepted.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"original endpoint");
        assert_eq!(replacement.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn replaced_proxy_never_gains_a_legitimate_accept_and_cleanup_preserves_it() {
        let proxy = Proxy::new().unwrap();
        let path = proxy.path.clone();
        fs::remove_file(proxy.socket()).unwrap();
        let foreign = UnixListener::bind(proxy.socket()).unwrap();
        let _connection = UnixStream::connect(proxy.socket()).unwrap();
        let mut child = Command::new("/bin/sleep")
            .arg("5")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            proxy
                .accept(Instant::now() + Duration::from_millis(30), &mut child)
                .is_err()
        );
        child.kill().unwrap();
        child.wait().unwrap();
        let inode = fs::symlink_metadata(proxy.socket()).unwrap().ino();
        drop(proxy);
        assert_eq!(fs::symlink_metadata(path.join("s")).unwrap().ino(), inode);
        drop(foreign);
        fs::remove_file(path.join("s")).unwrap();
        fs::remove_dir(path).unwrap();
    }
}
