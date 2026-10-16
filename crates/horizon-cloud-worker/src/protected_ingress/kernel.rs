use super::refused;
use horizon_cloud_protocol::exposure::ingress::{Namespace, REQUIRED_UNPRIVILEGED_PORT_START, ReceiptFacts};
use rustix::fs::{PROC_SUPER_MAGIC, fstatfs};
use std::{
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
};

const LIMIT: u64 = 64 * 1024;

pub(super) struct Custody {
    pid: File,
    net: File,
    user: File,
    init_start: u64,
}
impl Custody {
    pub fn capture() -> io::Result<Self> {
        Ok(Self {
            pid: File::open("/proc/thread-self/ns/pid")?,
            net: File::open("/proc/thread-self/ns/net")?,
            user: File::open("/proc/thread-self/ns/user")?,
            init_start: start(&proc_text("/proc/1/stat")?, 1)?,
        })
    }
    pub fn validate(&self, facts: &ReceiptFacts) -> io::Result<()> {
        require_root()?;
        root_status(&proc_text("/proc/1/status")?, 1)?;
        for (original, current, init) in [
            (&self.pid, "/proc/thread-self/ns/pid", "/proc/1/ns/pid"),
            (&self.net, "/proc/thread-self/ns/net", "/proc/1/ns/net"),
            (&self.user, "/proc/thread-self/ns/user", "/proc/1/ns/user"),
        ] {
            let expected = namespace(original)?;
            if expected != namespace(&File::open(current)?)? || expected != namespace(&File::open(init)?)? {
                return Err(refused());
            }
        }
        if self.init_start != facts.container.init_start_ticks
            || start(&proc_text("/proc/1/stat")?, 1)? != self.init_start
            || namespace(&self.net)? != facts.container.netns
        {
            return Err(refused());
        }
        let actual: u16 = proc_text("/proc/sys/net/ipv4/ip_unprivileged_port_start")?
            .trim()
            .parse()
            .map_err(|_| refused())?;
        if actual < REQUIRED_UNPRIVILEGED_PORT_START || actual != facts.unprivileged_port_start {
            return Err(refused());
        }
        require_root()
    }
}

pub(super) fn require_root() -> io::Result<()> {
    if !rustix::process::getuid().is_root() || !rustix::process::geteuid().is_root() {
        return Err(refused());
    }
    let status = proc_text("/proc/thread-self/status")?;
    root_status(&status, std::process::id())?;
    identity_map(&proc_text("/proc/thread-self/uid_map")?)?;
    identity_map(&proc_text("/proc/thread-self/gid_map")?)?;
    start(&proc_text("/proc/self/stat")?, std::process::id())?;
    Ok(())
}

pub(super) fn root_status(status: &str, process: u32) -> io::Result<()> {
    let uids: Vec<_> = field(status, "Uid:")?.split_whitespace().collect();
    let caps = u64::from_str_radix(field(status, "CapEff:")?.trim(), 16).map_err(|_| refused())?;
    if uids != ["0", "0", "0", "0"]
        || caps & (1 << 10) == 0
        || field(status, "Tgid:")?.trim().parse::<u32>().map_err(|_| refused())? != process
    {
        return Err(refused());
    }
    Ok(())
}
fn field<'a>(text: &'a str, name: &str) -> io::Result<&'a str> {
    let mut matches = text.lines().filter_map(|line| line.strip_prefix(name));
    let value = matches.next().ok_or_else(refused)?;
    if matches.next().is_some() {
        return Err(refused());
    }
    Ok(value)
}
pub(super) fn identity_map(text: &str) -> io::Result<()> {
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields != ["0", "0", "4294967295"] {
        return Err(refused());
    }
    Ok(())
}
pub(super) fn start(stat: &str, pid: u32) -> io::Result<u64> {
    let (head, tail) = stat.rsplit_once(") ").ok_or_else(refused)?;
    if head
        .split_once(" (")
        .ok_or_else(refused)?
        .0
        .parse::<u32>()
        .map_err(|_| refused())?
        != pid
    {
        return Err(refused());
    }
    let mut fields = tail.split_whitespace();
    if !matches!(fields.next(), Some("R" | "S" | "D" | "I" | "T" | "t")) {
        return Err(refused());
    }
    let ticks = fields
        .nth(18)
        .ok_or_else(refused)?
        .parse::<u64>()
        .map_err(|_| refused())?;
    if ticks == 0 {
        return Err(refused());
    }
    Ok(ticks)
}
fn namespace(file: &File) -> io::Result<Namespace> {
    let m = file.metadata()?;
    Ok(Namespace {
        device: m.dev(),
        inode: m.ino(),
    })
}
fn proc_text(path: &str) -> io::Result<String> {
    let file = File::open(path)?;
    if fstatfs(&file)?.f_type != PROC_SUPER_MAGIC {
        return Err(refused());
    }
    let mut text = String::new();
    file.take(LIMIT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > LIMIT {
        return Err(refused());
    }
    Ok(text)
}
