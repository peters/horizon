//! Local Network Bridge on the worker. `hold` keeps one bridge session that the owner's
//! Horizon opened; `status`, `discover`, `forward`, `unforward` and `mcp` let agents use it.
//!
//! The worker never decides what is reachable: every connection is a SOCKS5 CONNECT that the
//! owner's Horizon checks against the bridged subnet, and its refusal is what agents see.
mod forward;
mod hold;
mod mcp;
mod owner;
#[cfg(test)]
mod tests;

use horizon_cloud_protocol::local_network::{DIRECTORY, Nonce, PREPARED, discovery::Discovery};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, PermissionsExt},
        net::UnixStream,
    },
    path::PathBuf,
    time::Duration,
};

const OFF: &str = "Local Network Bridge is off. Only the owner can turn it on, from this cloud's card in Horizon.";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);
/// Room for a discovery answer on the control socket.
const MAX_MESSAGE: u64 = 512 * 1024;

/// Where one worker keeps its bridge sockets.
#[derive(Clone, Debug)]
pub(crate) struct Paths {
    directory: PathBuf,
}

impl Paths {
    fn system() -> Self {
        Self {
            directory: PathBuf::from(DIRECTORY),
        }
    }

    fn bridge(&self, nonce: &Nonce) -> PathBuf {
        self.directory.join(format!("{nonce}.sock"))
    }

    fn control(&self) -> PathBuf {
        self.directory.join("control.sock")
    }

    fn lock(&self) -> PathBuf {
        self.directory.join("lock")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Status,
    Discover,
    Forward {
        host: String,
        port: u16,
    },
    Unforward {
        worker_port: u16,
    },
    /// From a newer session's helper, asking this one to make way if its session was lost.
    Retire,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Status {
    /// Whether the owner's Horizon is bridging its local network to this worker now.
    active: bool,
    /// The bridged IPv4 subnet on the owner's network.
    #[serde(skip_serializing_if = "Option::is_none")]
    subnet: Option<String>,
    /// The SOCKS5 proxy on this worker's loopback, for tools and browsers that accept one.
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy: Option<String>,
    forwards: Vec<Forward>,
    /// Whether `local_network_discover` works, and with what, on the owner's computer.
    #[serde(skip_serializing_if = "Option::is_none")]
    discovery: Option<Availability>,
    note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Availability {
    /// Whether the owner's Horizon answers discovery requests.
    available: bool,
    /// What the owner's computer finds devices with: `mdns`, `ssdp` and `neighbors`.
    sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

impl Status {
    fn off() -> Self {
        Self {
            active: false,
            subnet: None,
            proxy: None,
            forwards: Vec::new(),
            discovery: None,
            note: OFF.into(),
        }
    }
}

/// A pinned forward: `127.0.0.1:worker_port` on this worker reaches `host:port` on the owner's network.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Forward {
    worker_port: u16,
    host: String,
    port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Answer {
    Status(Status),
    Discovery(Discovery),
    Forward(Forward),
    Retired,
    Error(String),
}

pub(crate) fn run() -> io::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(2).collect();
    let arguments: Vec<_> = arguments.iter().map(String::as_str).collect();
    let paths = Paths::system();
    match arguments.as_slice() {
        ["prepare"] => {
            prepare(&paths)?;
            println!("{PREPARED}");
            Ok(())
        }
        ["hold", nonce, subnet] => hold::run(&paths, nonce, subnet, io::stdin(), io::stdout()),
        ["status"] => print(&status(&paths)?),
        ["discover"] => print(&discover(&paths)?),
        ["forward", host, port] => {
            let port = port.parse().map_err(|_| io::Error::other("Invalid port"))?;
            print(&forward(&paths, host, port)?)
        }
        ["unforward", worker_port] => {
            let worker_port = worker_port
                .parse()
                .map_err(|_| io::Error::other("Invalid worker port"))?;
            print(&unforward(&paths, worker_port)?)
        }
        ["mcp"] => mcp::run(paths),
        _ => Err(io::Error::other(
            "Usage: horizon-cloud-worker local-network status|discover|forward <host> <port>|unforward <worker-port>|mcp",
        )),
    }
}

fn print(value: &impl serde::Serialize) -> io::Result<()> {
    serde_json::to_writer_pretty(io::stdout().lock(), value)?;
    io::stdout().write_all(b"\n")
}

/// Creates the private socket directory before sshd binds the session's bridge socket there.
fn prepare(paths: &Paths) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(&paths.directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(&paths.directory)?;
    if !metadata.is_dir() {
        return Err(io::Error::other("The local network directory is not a directory"));
    }
    std::fs::set_permissions(&paths.directory, std::fs::Permissions::from_mode(0o700))
}

/// Asks the running helper; with no helper listening, the bridge is off.
fn exchange(paths: &Paths, request: &Request) -> io::Result<Answer> {
    let Ok(mut stream) = UnixStream::connect(paths.control()) else {
        return Ok(Answer::Status(Status::off()));
    };
    stream.set_read_timeout(Some(CONTROL_TIMEOUT))?;
    stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let line = read_line(&mut BufReader::new(stream))?.ok_or_else(|| io::Error::other(OFF))?;
    serde_json::from_str(&line).map_err(|_| io::Error::other("Invalid local network answer"))
}

fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let count = reader.take(MAX_MESSAGE + 1).read_line(&mut line)?;
    if count as u64 > MAX_MESSAGE {
        return Err(io::Error::other("Local network message too large"));
    }
    Ok((count > 0).then_some(line))
}

pub(crate) fn status(paths: &Paths) -> io::Result<Status> {
    match exchange(paths, &Request::Status)? {
        Answer::Status(status) => Ok(status),
        Answer::Error(error) => Err(io::Error::other(error)),
        Answer::Discovery(_) | Answer::Forward(_) | Answer::Retired => {
            Err(io::Error::other("Invalid local network answer"))
        }
    }
}

pub(crate) fn discover(paths: &Paths) -> io::Result<Discovery> {
    match exchange(paths, &Request::Discover)? {
        Answer::Discovery(found) => Ok(found),
        Answer::Error(error) => Err(io::Error::other(error)),
        Answer::Status(_) | Answer::Forward(_) | Answer::Retired => Err(io::Error::other(OFF)),
    }
}

pub(crate) fn forward(paths: &Paths, host: &str, port: u16) -> io::Result<Forward> {
    match exchange(
        paths,
        &Request::Forward {
            host: host.to_owned(),
            port,
        },
    )? {
        Answer::Forward(forward) => Ok(forward),
        Answer::Error(error) => Err(io::Error::other(error)),
        Answer::Status(_) | Answer::Discovery(_) | Answer::Retired => Err(io::Error::other(OFF)),
    }
}

pub(crate) fn unforward(paths: &Paths, worker_port: u16) -> io::Result<Status> {
    match exchange(paths, &Request::Unforward { worker_port })? {
        Answer::Status(status) if status.active => Ok(status),
        Answer::Error(error) => Err(io::Error::other(error)),
        _ => Err(io::Error::other(OFF)),
    }
}
