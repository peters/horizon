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

    fn smudge(
        &mut self,
        directory: &Path,
        request: &GitPreparation,
        pointer: &super::lfs::Pointer,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        let _ = (directory, request, pointer, cancelled);
        Err(Error::Git)
    }
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

    #[cfg(test)]
    pub(super) fn with_timeout(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
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
        input: &[u8],
        allow_missing: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.exchange(command(directory, args), input, LIMIT, allow_missing, cancelled)
    }

    fn smudge(
        &mut self,
        directory: &Path,
        request: &GitPreparation,
        pointer: &super::lfs::Pointer,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.smudge_endpoint(
            directory,
            pointer,
            cancelled,
            &format!("https://github.com/{}.git/info/lfs", request.source.repository),
        )
    }
}

impl Git {
    pub(super) fn smudge_endpoint(
        &mut self,
        directory: &Path,
        pointer: &super::lfs::Pointer,
        cancelled: &dyn Fn() -> bool,
        endpoint: &str,
    ) -> Result<Vec<u8>, Error> {
        let mut child = command(directory, &[]);
        child.env_remove("GIT_LFS_SKIP_SMUDGE");
        for option in [
            format!("lfs.url={endpoint}"),
            "lfs.basictransfersonly=true".into(),
            "lfs.concurrenttransfers=1".into(),
            "lfs.transfer.maxretries=1".into(),
            "lfs.fetchinclude=".into(),
            "lfs.fetchexclude=".into(),
            "lfs.skipdownloaderrors=false".into(),
            "lfs.remote.autodetect=false".into(),
            "lfs.remote.searchall=false".into(),
            "lfs.dialtimeout=10".into(),
            "lfs.tlstimeout=10".into(),
            "lfs.activitytimeout=10".into(),
        ] {
            child.args(["-c", &option]);
        }
        child.args(["lfs", "smudge", "--", &pointer.path]);
        // A downloader may write before verifying the advertised object size.
        // Apply a kernel ceiling to every child file, including incomplete cache files.
        // Small fixed Git metadata writes need at most the additional 4 KiB floor.
        let mut bounded = Command::new("/usr/bin/prlimit");
        bounded
            .arg(format!("--fsize={0}:{0}", pointer.size.max(4096)))
            .args(["--core=0:0", "--", "/usr/bin/git"])
            .args(child.get_args())
            .env_clear()
            .envs(
                child
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .current_dir(directory)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        self.exchange(bounded, &pointer.encoded, pointer.size, false, cancelled)
    }
    fn exchange(
        &mut self,
        mut command: Command,
        mut input: &[u8],
        limit: usize,
        allow_missing: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        if cancelled() || Instant::now() >= self.deadline {
            return Err(Error::Interrupted);
        }
        let mut child = Owned(command.spawn().map_err(|_| Error::Git)?);
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
                Ok(count) if output.len() + count <= limit => output.extend_from_slice(&buffer[..count]),
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
    let mut budget = super::submodules::Budget::default();
    prepare_repository(git, directory, request, cancelled, verify, &mut budget, 0)?;
    budget.verify()?;
    verify()
}

pub(super) fn prepare_repository(
    git: &mut impl Commands,
    directory: &Path,
    request: &GitPreparation,
    cancelled: &dyn Fn() -> bool,
    verify: &dyn Fn() -> Result<(), Error>,
    budget: &mut super::submodules::Budget,
    depth: usize,
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
    budget.charge(modes.len())?;
    if !run(&["ls-tree", "--name-only", commit, "--", ".lfsconfig"], &[], false)?.is_empty() {
        return Err(Error::UnsupportedRepository);
    }
    let has_children = modes.split(|byte| *byte == b'\n').any(|mode| mode == b"160000");
    let mut plan = super::submodules::read_plan(&mut run, request, has_children, depth, budget)?;
    // Git inherits caller umask. Admit and create only planned empty directories
    // privately before checkout, rather than chmod/adopt paths after materialization.
    plan.prepare_paths(directory, verify)?;
    if depth == 0 {
        run(&["checkout", "-b", &request.work_branch, commit], &[], false)?;
    } else {
        run(&["checkout", "--detach", commit], &[], false)?;
    }
    let paths = run(&["ls-files", "-z"], &[], false)?;
    let attributes = run(&["check-attr", "--cached", "-z", "--stdin", "filter"], &paths, false)?;
    let pointers = run(
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
    )?;
    super::lfs::hydrate(
        git,
        directory,
        request,
        super::lfs::Selection {
            attributes: &attributes,
            matches: &pointers,
        },
        &mut budget.lfs,
        cancelled,
        verify,
    )?;
    super::submodules::hydrate(git, directory, request, plan, cancelled, verify, budget)?;
    if depth > 0 || has_children {
        verify_checkout(git, directory, commit, cancelled, verify)?;
    }
    Ok(())
}

fn verify_checkout(
    git: &mut impl Commands,
    directory: &Path,
    commit: &str,
    cancelled: &dyn Fn() -> bool,
    verify: &dyn Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    let mut run = |args: &[&str]| {
        verify()?;
        let output = git.run(directory, args, &[], false, cancelled)?;
        verify()?;
        Ok::<_, Error>(output)
    };
    if run(&["rev-parse", "--verify", "HEAD^{commit}"])? != format!("{commit}\n").as_bytes() {
        return Err(Error::Git);
    }
    run(&["diff-index", "--cached", "--quiet", commit, "--"])?;
    // Match admitted LFS clean semantics, including Git's child status processes.
    // Setup otherwise disables process filters while checking out pointers.
    let status = run(&[
        "-c",
        "filter.lfs.process=/usr/bin/git-lfs filter-process",
        "-c",
        "filter.lfs.required=true",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
    ])?;
    if !status.is_empty() {
        return Err(Error::Git);
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn environment(directory: &Path) -> Command {
    command(directory, &["status"])
}
