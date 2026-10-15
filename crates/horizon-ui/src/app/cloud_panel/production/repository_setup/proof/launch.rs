//! Proof containers that get this computer's files by copy, never by a bind mount: a
//! container is created, the files go into it as one archive, and then it runs attached.
//! So a proof needs nothing of this computer's file system from the Docker daemon, and
//! the same commands run on a Docker daemon of another machine.
use super::{Job, STEP_TIMEOUT};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A file or folder of this computer, and where it goes in the container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Copy {
    pub(super) from: PathBuf,
    /// An absolute path in the container.
    pub(super) to: String,
    /// A folder that `from` lies in: each part of `from` below it is opened without
    /// following a link, so a link that replaces one cannot lead out of it.
    pub(super) within: Option<PathBuf>,
}

impl Copy {
    pub(super) fn new(from: impl Into<PathBuf>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            within: None,
        }
    }

    /// As [`Copy::new`], for a `from` inside the folder `within`.
    pub(super) fn within(within: impl Into<PathBuf>, from: impl Into<PathBuf>, to: impl Into<String>) -> Self {
        Self {
            within: Some(within.into()),
            ..Self::new(from, to)
        }
    }
}

/// A file's content, exactly `left` bytes: a file that shrinks while it is read fails
/// the archive instead of shifting the entries after it.
struct Exact<R> {
    inner: R,
    left: u64,
}

impl<R: std::io::Read> std::io::Read for Exact<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let most = usize::try_from(self.left).unwrap_or(usize::MAX).min(buffer.len());
        let read = self.inner.read(&mut buffer[..most])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "a file became shorter while it was copied",
            ));
        }
        self.left -= read as u64;
        Ok(read)
    }
}

/// A container a proof starts: the `docker run` command it would have been, and the
/// files that go into it before it starts.
pub(super) struct Launch {
    pub(super) command: Command,
    pub(super) copies: Vec<Copy>,
}

impl Launch {
    pub(super) fn new(command: Command) -> Self {
        Self {
            command,
            copies: Vec::new(),
        }
    }
}

/// The `docker create` arguments of `run`, a `docker run` command line without its
/// program: the same container, kept after it ends, so its files go in before it starts
/// and Horizon removes it after. Only the `--rm` right after `run` goes, as the proof
/// writes it; one in the container's own command stays.
pub(super) fn create_args(run: &[std::ffi::OsString]) -> Result<Vec<std::ffi::OsString>, String> {
    let mut args = run.iter();
    if args.next().map(std::ffi::OsString::as_os_str) != Some(std::ffi::OsStr::new("run")) {
        return Err("a proof container starts with docker run".to_owned());
    }
    let mut create = vec![std::ffi::OsString::from("create")];
    let mut args = args.peekable();
    if args.peek().is_some_and(|arg| arg.as_os_str() == "--rm") {
        args.next();
    }
    create.extend(args.cloned());
    Ok(create)
}

/// Writes `copies` as one tar archive to `out`, each at its path in the container. Every
/// entry is owned by root, readable by everyone and writable by its owner only, whatever
/// this computer's account and umask made of it: the CI's user can read Horizon's
/// localci and the clones but not change them. Links are kept as links, never followed,
/// so a link in a repository cannot bring a file of this computer into the container;
/// FIFOs, sockets and devices are left out.
pub(super) fn archive(copies: &[Copy], out: impl std::io::Write) -> std::io::Result<()> {
    let mut builder = tar::Builder::new(out);
    for copy in copies {
        add(&mut builder, copy, Path::new(copy.to.trim_start_matches('/')))?;
    }
    builder.into_inner()?.flush()
}

/// The tar header of an entry Horizon copies: owned by root and readable by everyone,
/// writable by no one else.
fn header(kind: tar::EntryType, mode: u32, mtime: u64, size: u64) -> std::io::Result<tar::Header> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("root")?;
    header.set_groupname("root")?;
    let readable = if kind == tar::EntryType::Directory {
        0o555
    } else {
        0o444
    };
    header.set_mode((mode & 0o755) | readable);
    header.set_mtime(mtime);
    header.set_size(size);
    Ok(header)
}

/// Adds `from` at `inside`, a folder with everything in it. Each entry is opened from
/// its folder without following a link, so a link that replaces a folder or a file
/// while the archive is written is copied as a link and never read through.
#[cfg(unix)]
fn add(builder: &mut tar::Builder<impl std::io::Write>, copy: &Copy, inside: &Path) -> std::io::Result<()> {
    use rustix::fs::{Mode, OFlags};
    let from = &copy.from;
    let refused = || std::io::Error::other(format!("cannot copy {}", from.display()));
    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let (mut folder, parts) = if let Some(within) = &copy.within {
        let below = from.strip_prefix(within).map_err(|_| refused())?;
        (rustix::fs::open(within, directory, Mode::empty())?, below.to_owned())
    } else {
        let parent = from.parent().ok_or_else(refused)?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        (
            rustix::fs::open(parent, directory, Mode::empty())?,
            PathBuf::from(from.file_name().ok_or_else(refused)?),
        )
    };
    let mut parts: Vec<_> = parts.iter().map(std::ffi::OsStr::to_owned).collect();
    let name = parts.pop().ok_or_else(refused)?;
    for part in parts {
        folder = rustix::fs::openat(&folder, &part, directory | OFlags::NOFOLLOW, Mode::empty())?;
    }
    add_at(builder, &folder, &name, inside)
}

#[cfg(unix)]
fn add_at(
    builder: &mut tar::Builder<impl std::io::Write>,
    folder: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    inside: &Path,
) -> std::io::Result<()> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;
    let stat = rustix::fs::statat(folder, name, AtFlags::SYMLINK_NOFOLLOW)?;
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::Symlink => {
            let target = rustix::fs::readlinkat(folder, name, Vec::new())?;
            let target = std::ffi::OsStr::from_bytes(target.as_bytes());
            let mtime = u64::try_from(stat.st_mtime).unwrap_or(0);
            let mut header = header(tar::EntryType::Symlink, 0o777, mtime, 0)?;
            builder.append_link(&mut header, inside, target)?;
        }
        FileType::Directory => {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let sub = rustix::fs::openat(folder, name, flags, Mode::empty())?;
            let metadata = std::fs::File::from(sub.try_clone()?).metadata()?;
            let mtime = u64::try_from(metadata.mtime()).unwrap_or(0);
            let mut header = header(tar::EntryType::Directory, metadata.mode(), mtime, 0)?;
            builder.append_data(&mut header, inside, std::io::empty())?;
            let mut names = Vec::new();
            for entry in rustix::fs::Dir::read_from(&sub)? {
                let entry = entry?;
                let entry_name = entry.file_name().to_bytes();
                if entry_name != b"." && entry_name != b".." {
                    names.push(std::ffi::OsString::from(std::ffi::OsStr::from_bytes(entry_name)));
                }
            }
            for entry in names {
                add_at(builder, &sub, &entry, &inside.join(&entry))?;
            }
        }
        FileType::RegularFile => {
            // Without blocking: a FIFO that replaced the file since would wait for a writer.
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let file = std::fs::File::from(rustix::fs::openat(folder, name, flags, Mode::empty())?);
            // The size of what was opened: exactly that much goes into the archive.
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                return Ok(());
            }
            let size = metadata.len();
            let mtime = u64::try_from(metadata.mtime()).unwrap_or(0);
            let mut header = header(tar::EntryType::Regular, metadata.mode(), mtime, size)?;
            builder.append_data(
                &mut header,
                inside,
                Exact {
                    inner: file,
                    left: size,
                },
            )?;
        }
        // FIFOs, sockets and devices are left out.
        _ => {}
    }
    Ok(())
}

/// Adds `from` at `inside`, a folder with everything in it.
#[cfg(not(unix))]
fn add(builder: &mut tar::Builder<impl std::io::Write>, copy: &Copy, inside: &Path) -> std::io::Result<()> {
    add_path(builder, &copy.from, inside)
}

#[cfg(not(unix))]
fn add_path(builder: &mut tar::Builder<impl std::io::Write>, from: &Path, inside: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(from)?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs());
    let kind = metadata.file_type();
    if kind.is_symlink() {
        let mut header = header(tar::EntryType::Symlink, 0o777, mtime, 0)?;
        builder.append_link(&mut header, inside, std::fs::read_link(from)?)?;
    } else if kind.is_dir() {
        let mut header = header(tar::EntryType::Directory, 0o755, mtime, 0)?;
        builder.append_data(&mut header, inside, std::io::empty())?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            add_path(builder, &entry.path(), &inside.join(entry.file_name()))?;
        }
    } else if kind.is_file() {
        let file = std::fs::File::open(from)?;
        let size = file.metadata()?.len();
        let mut header = header(tar::EntryType::Regular, 0o644, mtime, size)?;
        builder.append_data(
            &mut header,
            inside,
            Exact {
                inner: file,
                left: size,
            },
        )?;
    }
    Ok(())
}

impl Job {
    /// A container of the image as the worker account on the proof's volume, with the
    /// GPU, this computer's packages and Horizon's localci that the proof needs. Horizon's
    /// localci and the Rust git mirrors go in as copies, owned by root, so the CI's user
    /// can read but not change them.
    pub(super) fn ci_container(&self) -> Result<Launch, String> {
        let mut command = self.docker();
        command
            .args(["run", "--rm", "--name", &self.container, "--user", "10001:10001", "-v"])
            .arg(format!("{}:/workspace", self.volume))
            .args(["-w", "/workspace/checkout", "-e", "HOME=/workspace/home"]);
        let mut copies = Vec::new();
        // A proof with plain commands runs them with the image's shell and needs no localci.
        if self.spec.commands.is_empty() {
            copies.push(Copy::new(self.trusted_localci()?, super::localci::IN_CONTAINER));
        }
        // A GPU profile is proved with a GPU or not at all: CI that passes without one
        // says nothing about the worker that has one.
        if self.wants_gpu() {
            if !Path::new("/dev/nvidiactl").exists() {
                return Err(
                    "the default profile needs a GPU and this computer has no NVIDIA GPU, so Horizon cannot prove it here"
                        .to_owned(),
                );
            }
            command.args(["--gpus", "all"]);
        }
        // Packages this computer restored, private ones included, need no feed credential:
        // NuGet reads the ones copied into the volume as a fallback folder before it asks
        // a source.
        if let Some(packages) = self.package_folder() {
            command.args(["-e", &format!("RestoreAdditionalProjectFallbackFolders={packages}")]);
        }
        copies.extend(self.crate_copies(&mut command));
        Ok(Launch { command, copies })
    }

    /// Runs `launch` as its `docker run` would, streaming its output like any proof
    /// command: created, given its copies, started attached, and removed after.
    pub(super) fn stream_launch(
        &self,
        launch: Launch,
        log: &mut Option<std::fs::File>,
        limit: Option<Duration>,
        report: bool,
    ) -> Result<Vec<u8>, String> {
        let Some(start) = self.created(launch)? else {
            return Err("stopped".to_owned());
        };
        let result = self.stream_inner(start, log, limit, report);
        self.remove_container();
        result
    }

    /// As [`Self::stream_launch`], returning what the container wrote to its standard
    /// output, as [`Self::capture`] does.
    pub(super) fn capture_launch(&self, launch: Launch, limit: Duration) -> Result<Vec<u8>, String> {
        let Some(start) = self.created(launch)? else {
            return Err("stopped".to_owned());
        };
        let result = self.capture(start, limit);
        self.remove_container();
        result
    }

    /// Creates the container of `launch` and copies its files in; the command that
    /// starts it attached, or `None` once the proof stopped.
    fn created(&self, launch: Launch) -> Result<Option<Command>, String> {
        let Launch { command, copies } = launch;
        let args: Vec<std::ffi::OsString> = command.get_args().map(std::ffi::OsStr::to_os_string).collect();
        if copies.is_empty() {
            // Nothing to copy: the command runs as it is.
            return Ok(Some(command));
        }
        let create = || {
            let mut create = self.docker();
            create.args(create_args(&args)?);
            // The first create pulls Horizon's base image, which may take as long as a build.
            self.capture(create, super::BUILD_TIMEOUT)
        };
        // A container of the name that an earlier removal could not take away in time
        // goes first, as `docker run --rm` would have left none.
        if let Err(error) = create().or_else(|error| {
            if error.contains("already in use") {
                self.remove_container();
                create()
            } else {
                Err(error)
            }
        }) {
            return Err(format!("cannot create the proof's container: {error}"));
        }
        if let Err(error) = self.copy_in(&copies) {
            self.remove_container();
            return Err(error);
        }
        if self.cancel.is_cancelled() {
            self.remove_container();
            return Ok(None);
        }
        let mut start = self.docker();
        start.args(["start", "--attach", &self.container]);
        Ok(Some(start))
    }

    /// Copies `copies` into the created container as one archive on `docker cp`'s
    /// standard input, within [`STEP_TIMEOUT`]; Stop ends it.
    fn copy_in(&self, copies: &[Copy]) -> Result<(), String> {
        let mut copy = self.docker();
        copy.args(["cp", "-", &format!("{}:/", self.container)]);
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut copy, 0);
        let mut child = copy
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start docker cp: {error}"))?;
        let input = child.stdin.take();
        let copies = copies.to_vec();
        let writer = std::thread::spawn(move || match input {
            Some(input) => archive(&copies, std::io::BufWriter::new(input)).map_err(|error| error.to_string()),
            None => Err("docker cp took no input".to_owned()),
        });
        let stderr = child.stderr.take().map(|pipe| {
            std::thread::spawn(move || {
                use std::io::Read as _;
                let mut text = String::new();
                let _ = pipe.take(4096).read_to_string(&mut text);
                text
            })
        });
        let started = Instant::now();
        let status = loop {
            if let Ok(Some(status)) = child.try_wait() {
                break status;
            }
            if self.cancel.is_cancelled() || started.elapsed() > STEP_TIMEOUT {
                super::stream::end_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(if self.cancel.is_cancelled() {
                    "stopped".to_owned()
                } else {
                    format!(
                        "copying files into the proof's container took more than {} minutes",
                        STEP_TIMEOUT.as_secs() / 60
                    )
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let written = writer
            .join()
            .unwrap_or_else(|_| Err("the archive writer stopped".to_owned()));
        let stderr = stderr.and_then(|reader| reader.join().ok()).unwrap_or_default();
        if !status.success() {
            return Err(format!(
                "cannot copy files into the proof's container: {}",
                stderr.trim()
            ));
        }
        written.map_err(|error| format!("cannot copy files into the proof's container: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_command_becomes_a_create_that_keeps_its_container() {
        let run: Vec<std::ffi::OsString> = [
            "run",
            "--rm",
            "--name",
            "proof",
            "-v",
            "volume:/workspace",
            "image",
            "sh",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        let create = create_args(&run).unwrap();
        assert_eq!(
            create,
            ["create", "--name", "proof", "-v", "volume:/workspace", "image", "sh"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        );
        assert!(
            create_args(&["exec".into()]).is_err(),
            "only docker run is turned into a create"
        );
        let keeps: Vec<std::ffi::OsString> = ["run", "--rm", "image", "localci", "run", "--skip", "--rm"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(
            create_args(&keeps)
                .unwrap()
                .last()
                .map(|arg| arg.to_string_lossy().into_owned()),
            Some("--rm".to_owned()),
            "a --rm in the container's own command stays"
        );
    }

    #[test]
    fn a_file_that_shrinks_while_it_is_read_fails_the_archive() {
        let mut short = Exact {
            inner: std::io::Cursor::new(b"abc".to_vec()),
            left: 5,
        };
        let mut read = Vec::new();
        let error = std::io::Read::read_to_end(&mut short, &mut read).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
        let mut long = Exact {
            inner: std::io::Cursor::new(b"abcdef".to_vec()),
            left: 3,
        };
        let mut read = Vec::new();
        std::io::Read::read_to_end(&mut long, &mut read).unwrap();
        assert_eq!(read, b"abc", "a file that grew gives what it had when opened");
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_within_a_folder_never_follows_a_link_in_its_path() {
        let cache = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("1.0.0")).unwrap();
        std::fs::write(outside.path().join("1.0.0/private.txt"), "secret").unwrap();
        // The package id was a folder when it was checked, and is a link now.
        std::os::unix::fs::symlink(outside.path(), cache.path().join("example")).unwrap();
        let copy = Copy::within(
            cache.path(),
            cache.path().join("example/1.0.0"),
            "/host-nuget/example/1.0.0",
        );
        let mut archived = Vec::new();
        assert!(archive(&[copy], &mut archived).is_err(), "the link is not followed");
        std::fs::create_dir_all(cache.path().join("real/2.0.0")).unwrap();
        std::fs::write(cache.path().join("real/2.0.0/lib.dll"), "package").unwrap();
        let copy = Copy::within(cache.path(), cache.path().join("real/2.0.0"), "/host-nuget/real/2.0.0");
        let mut archived = Vec::new();
        archive(&[copy], &mut archived).unwrap();
        let mut entries = tar::Archive::new(archived.as_slice());
        let names: Vec<String> = entries
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().display().to_string())
            .collect();
        assert_eq!(names, ["host-nuget/real/2.0.0", "host-nuget/real/2.0.0/lib.dll"]);
    }

    #[test]
    fn the_archive_puts_each_copy_at_its_place_and_keeps_links_as_links() {
        let folder = tempfile::tempdir().unwrap();
        let tree = folder.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join("sub/file.txt"), "inside").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // A group-writable file and one only its owner may read, as umasks make them.
            std::fs::set_permissions(tree.join("sub/file.txt"), std::fs::Permissions::from_mode(0o664)).unwrap();
            std::fs::write(tree.join("private"), "x").unwrap();
            std::fs::set_permissions(tree.join("private"), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let secret = folder.path().join("secret");
        std::fs::write(&secret, "of this computer").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, tree.join("link")).unwrap();
        let bundle = folder.path().join("head.bundle");
        std::fs::write(&bundle, "bundle").unwrap();
        let mut bytes = Vec::new();
        archive(
            &[Copy::new(&tree, "/snapshot"), Copy::new(&bundle, "/src.bundle")],
            &mut bytes,
        )
        .unwrap();
        let mut read = tar::Archive::new(bytes.as_slice());
        let entries: Vec<(String, tar::EntryType, String)> = read
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().display().to_string();
                let header = entry.header();
                let kind = header.entry_type();
                assert_eq!(
                    (header.uid().unwrap(), header.gid().unwrap()),
                    (0, 0),
                    "{path} is root's"
                );
                let mode = header.mode().unwrap();
                assert_eq!(mode & 0o022, 0, "{path} is writable by its owner only: {mode:o}");
                if !kind.is_symlink() {
                    assert_eq!(mode & 0o444, 0o444, "{path} is readable by everyone: {mode:o}");
                }
                let mut text = String::new();
                if kind.is_file() {
                    std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
                }
                (path, kind, text)
            })
            .collect();
        assert!(entries.contains(&(
            "snapshot/sub/file.txt".to_owned(),
            tar::EntryType::Regular,
            "inside".to_owned()
        )));
        assert!(entries.contains(&("src.bundle".to_owned(), tar::EntryType::Regular, "bundle".to_owned())));
        #[cfg(unix)]
        assert!(
            entries.contains(&("snapshot/link".to_owned(), tar::EntryType::Symlink, String::new())),
            "a link stays a link: {entries:?}"
        );
        assert!(
            !entries.iter().any(|(_, _, text)| text == "of this computer"),
            "no file outside the copies goes in"
        );
    }
}
