use super::{GitPreparation, GitPreparationError as Error};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const LIMIT: usize = 128 * 1024;
pub(super) trait Commands {
    fn run(
        &mut self,
        directory: &Path,
        args: &[&str],
        input: &[u8],
        allow_missing: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error>;
}

pub(super) struct Git {
    deadline: Instant,
}
impl Git {
    pub fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(300),
        }
    }
}

fn command(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/git");
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(directory)
        .process_group(0);
    for option in [
        "credential.helper=",
        "credential.helper=/usr/local/bin/horizon-github-credential",
        "credential.interactive=false",
        "core.askPass=/bin/false",
        "core.hooksPath=/dev/null",
        "protocol.allow=never",
        "protocol.https.allow=always",
        "http.followRedirects=false",
        "filter.lfs.process=",
        "filter.lfs.smudge=",
        "filter.lfs.required=false",
    ] {
        command.args(["-c", option]);
    }
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(pid) = i32::try_from(self.0.id()).ok().and_then(rustix::process::Pid::from_raw) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Commands for Git {
    fn run(
        &mut self,
        directory: &Path,
        args: &[&str],
        mut input: &[u8],
        allow_missing: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        if cancelled() || Instant::now() >= self.deadline {
            return Err(Error::Interrupted);
        }
        let mut child = Owned(command(directory, args).spawn().map_err(|_| Error::Git)?);
        let mut stdin = child.0.stdin.take();
        let mut stdout = child.0.stdout.take().ok_or(Error::Git)?;
        let input_fd = stdin.as_ref().ok_or(Error::Git)?;
        fcntl_setfl(
            input_fd,
            fcntl_getfl(input_fd).map_err(|_| Error::Git)? | OFlags::NONBLOCK,
        )
        .map_err(|_| Error::Git)?;
        fcntl_setfl(
            &stdout,
            fcntl_getfl(&stdout).map_err(|_| Error::Git)? | OFlags::NONBLOCK,
        )
        .map_err(|_| Error::Git)?;
        let mut output = Vec::new();
        let mut eof = false;
        loop {
            if cancelled() || Instant::now() >= self.deadline {
                return Err(Error::Interrupted);
            }
            if !input.is_empty() {
                match stdin.as_mut().ok_or(Error::Git)?.write(input) {
                    Ok(0) => return Err(Error::Git),
                    Ok(count) => input = &input[count..],
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err(Error::Git),
                }
            }
            if input.is_empty() {
                stdin.take();
            }
            let mut buffer = [0; 8192];
            match stdout.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(count) if output.len() + count <= LIMIT => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Ok(_) | Err(_) => return Err(Error::Git),
            }
            let pid = i32::try_from(child.0.id())
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .ok_or(Error::Git)?;
            // Keep the leader unreaped until Drop kills the owned group. This
            // prevents PID reuse from ever turning cleanup into an unrelated signal.
            if eof
                && let Some(status) = rustix::process::waitid(
                    rustix::process::WaitId::Pid(pid),
                    rustix::process::WaitIdOptions::EXITED
                        | rustix::process::WaitIdOptions::NOHANG
                        | rustix::process::WaitIdOptions::NOWAIT,
                )
                .map_err(|_| Error::Git)?
            {
                if status.exit_status() == Some(0) || allow_missing && status.exit_status() == Some(1) {
                    return Ok(output);
                }
                return Err(Error::Git);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(super) fn prepare(
    git: &mut impl Commands,
    directory: &Path,
    request: &GitPreparation,
    cancelled: &dyn Fn() -> bool,
    verify: &dyn Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let mut run = |args: &[&str], input: &[u8], missing| {
        verify()?;
        let output = git.run(directory, args, input, missing, cancelled)?;
        verify()?;
        Ok::<_, Error>(output)
    };
    let commit = request.source.commit.as_str();
    let origin = format!("https://github.com/{}.git", request.source.repository);
    run(
        &["init", "--template=", "--initial-branch=horizon-unprepared", "."],
        &[],
        false,
    )?;
    run(&["remote", "add", "origin", &origin], &[], false)?;
    run(
        &[
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--depth=1",
            "origin",
            commit,
        ],
        &[],
        false,
    )?;
    if run(&["rev-parse", "--verify", "FETCH_HEAD^{commit}"], &[], false)? != format!("{commit}\n").as_bytes() {
        return Err(Error::Git);
    }
    let modes = run(&["ls-tree", "-r", "--format=%(objectmode)", commit], &[], false)?;
    if modes.split(|byte| *byte == b'\n').any(|mode| mode == b"160000")
        || !run(&["ls-tree", "--name-only", commit, "--", ".lfsconfig"], &[], false)?.is_empty()
    {
        return Err(Error::UnsupportedRepository);
    }
    run(&["checkout", "-b", &request.work_branch, commit], &[], false)?;
    let paths = run(&["ls-files", "-z"], &[], false)?;
    let attributes = run(&["check-attr", "--cached", "-z", "--stdin", "filter"], &paths, false)?;
    if attributes
        .split(|byte| *byte == 0)
        .skip(2)
        .step_by(3)
        .any(|value| value != b"unspecified" && value != b"unset")
        || !run(
            &[
                "grep",
                "-I",
                "-l",
                "-z",
                "-e",
                "^version https://git-lfs.github.com/spec/v1$",
                commit,
                "--",
            ],
            &[],
            true,
        )?
        .is_empty()
    {
        return Err(Error::UnsupportedRepository);
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn environment(directory: &Path) -> Command {
    command(directory, &["status"])
}
