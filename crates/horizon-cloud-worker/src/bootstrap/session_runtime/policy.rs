//! One qualified image/CLI policy; no inherited credentials or project commands.
use super::super::{
    inspection::execute_leased,
    store::{Store, invalid},
};
use std::{
    fs::{self, File},
    io::{self, Read, Seek},
    os::unix::fs::MetadataExt,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(super) const AGENT: &str = "/usr/local/bin/claude";
pub(super) const TMUX: &str = "/usr/bin/tmux";
pub(super) const ARGUMENTS: &[&str] = &[
    "--safe-mode",
    "--strict-mcp-config",
    "--mcp-config",
    "{\"mcpServers\":{}}",
    "--setting-sources",
    "",
    "--disable-slash-commands",
    "--no-chrome",
];
pub(super) fn qualify(store: &Store) -> io::Result<()> {
    match fs::symlink_metadata("/etc/claude-code") {
        Ok(meta) if meta.is_dir() && fs::read_dir("/etc/claude-code")?.next().is_none() => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(invalid()),
    }
    for path in [AGENT, TMUX] {
        let meta = fs::metadata(path)?;
        if !meta.is_file() || meta.mode() & 0o022 != 0 || meta.mode() & 0o111 == 0 {
            return Err(invalid());
        }
    }
    let home = tempfile::tempdir()?;
    let version = execute_leased(
        Command::new(AGENT)
            .arg("--version")
            .env_clear()
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("CLAUDE_CONFIG_DIR", home.path())
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("DISABLE_AUTOUPDATER", "1")
            .current_dir(home.path()),
        Duration::from_secs(10),
        store.lease()?,
    )?;
    if version != b"2.1.283 (Claude Code)\n" {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn environment(command: &mut Command, home: &Path) {
    command
        .env_clear()
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("TERM", "xterm-256color")
        .env("LANG", "C.UTF-8")
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
/// Control commands have bounded output and lifetime. Daemon descendants belong
/// to the calling subreaper, not a process group shared with another session.
pub(super) fn execute(command: &mut Command, timeout: Duration) -> io::Result<Vec<u8>> {
    let mut output = File::from(rustix::fs::memfd_create(
        "session-control",
        rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
    )?);
    output.set_len(65537)?;
    rustix::fs::fcntl_add_seals(&output, rustix::fs::SealFlags::GROW | rustix::fs::SealFlags::SHRINK)?;
    let mut child = ChildGuard(
        command
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + timeout;
    loop {
        if output.stream_position()? > 65536 || Instant::now() >= deadline {
            return Err(invalid());
        }
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err(invalid());
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let length = output.stream_position()?;
    if length > 65536 {
        return Err(invalid());
    }
    output.rewind()?;
    let mut bytes = Vec::new();
    output.take(length).read_to_end(&mut bytes)?;
    Ok(bytes)
}
