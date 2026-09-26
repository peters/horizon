use super::{
    super::{
        membership,
        recovery::{BOOTSTRAP, Bootstrap, ROOT, decode},
        runtime::Runtime,
        sessions,
        store::{Store, invalid, open_directory, same},
    },
    policy, process,
    records::Record,
};
use horizon_cloud_protocol::{
    OperationId, ProjectIdentity,
    membership::{Manifest, SessionId},
    session_runtime::Status,
};
use rustix::fs::{Mode, OFlags, mkdirat, openat};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{fs::MetadataExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Permit {
    project: ProjectIdentity,
    session: SessionId,
    nonce: OperationId,
}

pub(super) fn spawn(store: &Store, manifest: &Manifest, record: &mut Record) -> io::Result<()> {
    sessions::published(store, manifest, &record.launch.identity, record.session)?;
    let expected = serde_json::to_vec(record)?;
    let (mut parent, child) = UnixStream::pair()?;
    parent.set_write_timeout(Some(Duration::from_secs(5)))?;
    let child_fd: OwnedFd = child.into();
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("supervise-project-session")
        .env_clear()
        .stdin(Stdio::from(child_fd))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn()?;
    record.supervisor = Some(process::Identity::capture(
        i32::try_from(child.id()).map_err(|_| invalid())?,
    )?);
    record.save(store, Some(&expected))?;
    // The supervisor cannot acquire the allocation lock until this request
    // releases it. A terminal stop that wins next is checked before spawning.
    serde_json::to_writer(
        &mut parent,
        &Permit {
            project: record.launch.identity.clone(),
            session: record.session,
            nonce: record.nonce,
        },
    )?;
    parent.shutdown(std::net::Shutdown::Write)
}

pub(super) fn run() -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    rustix::process::setsid()?;
    rustix::process::umask(Mode::RWXG | Mode::RWXO);
    require_child_signals()?;
    rustix::process::set_child_subreaper(rustix::process::Pid::from_raw(1))?;
    let stream = UnixStream::from(io::stdin().as_fd().try_clone_to_owned()?);
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut bytes = Vec::new();
    stream.take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(invalid());
    }
    let permit: Permit = decode(&bytes)?;
    let own = process::Identity::current()?;
    let guard = Children;
    let result = supervise(&permit, &own);
    drop(guard);
    result
}
struct Children;
impl Drop for Children {
    fn drop(&mut self) {
        let _ = process::stop(Duration::from_secs(5));
    }
}

fn require_child_signals() -> io::Result<()> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    for name in ["SigIgn:", "SigCgt:"] {
        let value = status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .ok_or_else(invalid)?;
        if u64::from_str_radix(value.trim(), 16).map_err(|_| invalid())? & (1 << 16) != 0 {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(super) fn open(deadline: Instant) -> io::Result<Store> {
    loop {
        match Store::open(Path::new(ROOT)) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {}
            result => return result,
        }
    }
}
fn bound(store: &Store, permit: &Permit, own: &process::Identity) -> io::Result<(Manifest, Record, Vec<u8>)> {
    let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
    bootstrap.validate(store, &Runtime::captured()?)?;
    let (_, manifest) = membership::load(store, &bootstrap)?;
    let (record, bytes) = Record::load(store, &manifest, &permit.project, permit.session)?.ok_or_else(invalid)?;
    if record.nonce != permit.nonce || record.supervisor.as_ref() != Some(own) {
        return Err(invalid());
    }
    sessions::published(store, &manifest, &permit.project, permit.session)?;
    Ok((manifest, record, bytes))
}
fn stopping(manifest: &Manifest, permit: &Permit) -> io::Result<bool> {
    Ok(manifest
        .members
        .iter()
        .find(|m| m.identity == permit.project)
        .ok_or_else(invalid)?
        .stops
        .contains(&permit.session))
}

fn supervise(permit: &Permit, own: &process::Identity) -> io::Result<()> {
    let store = open(Instant::now() + Duration::from_secs(30))?;
    let (manifest, mut record, bytes) = bound(&store, permit, own)?;
    if record.status != Status::Launching || record.agent.is_some() {
        return Err(invalid());
    }
    if stopping(&manifest, permit)? {
        process::stop(Duration::from_secs(5))?;
        record.status = Status::Stopped;
        return record.save(&store, Some(&bytes));
    }
    policy::qualify(&store)?;
    let roots = sessions::published(&store, &manifest, &permit.project, permit.session)?;
    let runtime = Terminal::start(&store, permit, roots)?;
    let (agent, status) = runtime.observe()?;
    record.agent = agent;
    record.status = status;
    record.save(&store, Some(&bytes))?;
    drop(store);
    loop {
        thread::sleep(Duration::from_secs(1));
        let store = match Store::open(Path::new(ROOT)) {
            Ok(store) => store,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        };
        let (manifest, mut record, bytes) = bound(&store, permit, own)?;
        if stopping(&manifest, permit)? {
            record.status = Status::Stopping;
            record.save(&store, Some(&bytes))?;
            let bytes = serde_json::to_vec(&record)?;
            process::stop(Duration::from_secs(5))?;
            record.status = Status::Stopped;
            return record.save(&store, Some(&bytes));
        }
        let (agent, status) = runtime.observe()?;
        if record.agent != agent {
            return Err(invalid());
        }
        if status != record.status {
            record.status = status;
            record.save(&store, Some(&bytes))?;
        }
        process::reap()?;
    }
}

struct Terminal {
    directory: File,
    parent: File,
    name: String,
    socket: PathBuf,
    session: String,
    roots: Vec<File>,
    server: process::Identity,
    socket_inode: u64,
    socket_device: u64,
    agent: Option<process::Identity>,
    pane: i32,
}
fn descriptor(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()))
}
impl Terminal {
    fn start(store: &Store, permit: &Permit, roots: Vec<File>) -> io::Result<Self> {
        let parent = open_directory(store.path())?;
        let name = format!("r-{}", permit.nonce);
        mkdirat(&parent, name.as_str(), Mode::RWXU)?;
        let directory = File::from(openat(
            &parent,
            name.as_str(),
            OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let mut config = File::from(openat(
            &directory,
            "tmux.conf",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        config.write_all(b"set-option -g exit-empty off\nset-option -g remain-on-exit on\nset-option -g history-limit 2000\nset-option -g default-shell /bin/sh\nset-option -g update-environment ''\n")?;
        config.sync_all()?;
        directory.sync_all()?;
        parent.sync_all()?;
        store.verify()?;
        let socket = descriptor(&directory).join("s");
        let session = format!("p-{}-s-{}", permit.project.project_id(), permit.session);
        let mut command = Command::new(policy::TMUX);
        command
            .arg("-f")
            .arg(descriptor(&directory).join("tmux.conf"))
            .arg("-S")
            .arg(&socket)
            .args(["new-session", "-d", "-s", &session, "-c"])
            .arg(descriptor(roots.first().ok_or_else(invalid)?))
            .arg(policy::AGENT)
            .args(policy::ARGUMENTS);
        policy::environment(&mut command, &descriptor(roots.get(1).ok_or_else(invalid)?));
        policy::execute(&mut command, Duration::from_secs(5))?;
        let metadata = std::fs::symlink_metadata(&socket)?;
        let output = Self::query(&socket, &session)?;
        let (server, agent, status) = parse(&output)?;
        let parent_pid = std::fs::read_to_string(format!("/proc/{server}/status"))?;
        let expected = std::process::id().to_string();
        if parent_pid.lines().find_map(|l| l.strip_prefix("PPid:")).map(str::trim) != Some(expected.as_str()) {
            return Err(invalid());
        }
        let identity = capture_agent(server, agent, &status, process::Identity::capture, || {
            parse(&Self::query(&socket, &session)?)
        })?;
        Ok(Self {
            directory,
            parent,
            name,
            socket,
            session,
            roots,
            server: process::Identity::capture(server)?,
            socket_inode: metadata.ino(),
            socket_device: metadata.dev(),
            agent: identity,
            pane: agent,
        })
    }
    fn query(socket: &Path, session: &str) -> io::Result<Vec<u8>> {
        policy::execute(
            Command::new(policy::TMUX)
                .arg("-S")
                .arg(socket)
                .args([
                    "display-message",
                    "-p",
                    "-t",
                    &format!("={session}:0.0"),
                    "#{pid} #{pane_pid} #{pane_dead} #{pane_dead_status}",
                ])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", "/nonexistent"),
            Duration::from_secs(2),
        )
    }
    fn verify(&self) -> io::Result<()> {
        let current = File::from(openat(
            &self.parent,
            self.name.as_str(),
            OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        same(&self.directory, &current)?;
        let meta = std::fs::symlink_metadata(&self.socket)?;
        if meta.ino() != self.socket_inode
            || meta.dev() != self.socket_device
            || !self.server.alive()
            || self.roots.len() != 5
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn observe(&self) -> io::Result<(Option<process::Identity>, Status)> {
        self.verify()?;
        let pane = parse(&Self::query(&self.socket, &self.session)?)?;
        self.verify()?;
        let status = observe_pane(
            self.server.pid,
            self.pane,
            pane,
            || self.agent.as_ref().is_some_and(process::Identity::alive),
            || parse(&Self::query(&self.socket, &self.session)?),
        )?;
        self.verify()?;
        Ok((self.agent.clone(), status))
    }
}
fn capture_agent(
    server: i32,
    pane: i32,
    status: &Status,
    capture: impl FnOnce(i32) -> io::Result<process::Identity>,
    query: impl FnOnce() -> io::Result<(i32, i32, Status)>,
) -> io::Result<Option<process::Identity>> {
    if *status != Status::Running {
        return Ok(None);
    }
    if let Ok(identity) = capture(pane) {
        Ok(Some(identity))
    } else {
        exited_after_race(server, pane, query()?)?;
        Ok(None)
    }
}
fn observe_pane(
    server: i32,
    pane: i32,
    observation: (i32, i32, Status),
    alive: impl FnOnce() -> bool,
    query: impl FnOnce() -> io::Result<(i32, i32, Status)>,
) -> io::Result<Status> {
    let (observed_server, observed_pane, status) = observation;
    if observed_server != server || observed_pane != pane {
        return Err(invalid());
    }
    if status == Status::Running && !alive() {
        return exited_after_race(server, pane, query()?);
    }
    Ok(status)
}
fn exited_after_race(server: i32, pane: i32, observation: (i32, i32, Status)) -> io::Result<Status> {
    let (observed_server, observed_pane, status) = observation;
    if observed_server != server || observed_pane != pane || !matches!(status, Status::Exited { .. }) {
        return Err(invalid());
    }
    Ok(status)
}

fn parse(bytes: &[u8]) -> io::Result<(i32, i32, Status)> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let parts: Vec<_> = text.split_whitespace().collect();
    if !(3..=4).contains(&parts.len()) {
        return Err(invalid());
    }
    let pid = |index: usize| parts[index].parse::<i32>().ok().filter(|p| *p > 0).ok_or_else(invalid);
    let status = match parts[2] {
        "0" if parts.len() == 3 => Status::Running,
        "1" => Status::Exited {
            code: parts.get(3).map(|v| v.parse()).transpose().map_err(|_| invalid())?,
        },
        _ => return Err(invalid()),
    };
    Ok((pid(0)?, pid(1)?, status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_between_initial_query_and_identity_capture_retains_dead_pane() {
        assert!(
            capture_agent(
                10,
                11,
                &Status::Running,
                |_| Err(invalid()),
                || Ok((10, 11, Status::Exited { code: Some(17) }))
            )
            .unwrap()
            .is_none()
        );
        for pane in [
            (12, 11, Status::Exited { code: Some(17) }),
            (10, 12, Status::Exited { code: Some(17) }),
            (10, 11, Status::Running),
        ] {
            assert!(capture_agent(10, 11, &Status::Running, |_| Err(invalid()), || Ok(pane)).is_err());
        }
    }

    #[test]
    fn exit_between_live_query_and_liveness_check_retains_exit_status() {
        assert_eq!(
            observe_pane(
                10,
                11,
                (10, 11, Status::Running),
                || false,
                || Ok((10, 11, Status::Exited { code: Some(23) }))
            )
            .unwrap(),
            Status::Exited { code: Some(23) }
        );
        for pane in [
            (12, 11, Status::Exited { code: Some(23) }),
            (10, 12, Status::Exited { code: Some(23) }),
            (10, 11, Status::Running),
        ] {
            assert!(observe_pane(10, 11, (10, 11, Status::Running), || false, || Ok(pane)).is_err());
        }
    }
}
