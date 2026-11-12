use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use horizon_core::SshConnection;

use super::{
    LocalUploadFile, RemoteDirectoryListing, SSH_POLL_INTERVAL, SSH_PROBE_CONNECT_TIMEOUT_SECS, UploadControl,
    UploadMessage, UploadProgressContext, WorkerExit, non_empty_output, poll_upload_control, send_upload_snapshot,
    wait_for_child,
};

const SSH_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const SSH_UPLOAD_CHUNK_SIZE: usize = 256 * 1024;

/// The bytes of one write into the pipe of `ssh`.
///
/// A write to a pipe returns when the whole slice is inside the pipe, so a slice
/// that is larger than the pipe waits in the write while the far side is slow to
/// read it. A smaller piece keeps the reported progress moving and lets the loop
/// of the worker answer a `Cancel` between two pieces.
const SSH_WRITE_PIECE_SIZE: usize = 32 * 1024;

/// The pause of the loop that watches the child and the thread that fills its pipe.
const SSH_UPLOAD_POLL_INTERVAL: Duration = Duration::from_millis(50);

const RESOLVE_REMOTE_DIR_SCRIPT: &str = r#"
path=$1
if [ -z "$path" ]; then
  path=$HOME
fi
case "$path" in
  "~")
    path=$HOME
    ;;
  "~/"*)
    path=$HOME/${path#~/}
    ;;
esac
cd -- "$path" 2>/dev/null || exit 1
pwd -P
"#;

const LIST_REMOTE_DIRS_SCRIPT: &str = r#"
path=$1
if [ -z "$path" ]; then
  path=$HOME
fi
case "$path" in
  "~")
    path=$HOME
    ;;
  "~/"*)
    path=$HOME/${path#~/}
    ;;
esac
cd -- "$path" 2>/dev/null || exit 1
printf '__PWD__%s\n' "$PWD"
if [ "$PWD" != "/" ]; then
  printf '__DIR__..\n'
fi
for entry in * .[!.]* ..?*; do
  [ -d "$entry" ] || continue
  printf '__DIR__%s\n' "$entry"
done
"#;

const WRITE_REMOTE_FILE_SCRIPT: &str = r#"
set -eu
dir=$1
name=$2
cd -- "$dir" 2>/dev/null || exit 1
tmp_path=".horizon-upload-${name}.$$"
cleanup() {
  rm -f -- "$tmp_path"
}
trap cleanup EXIT HUP INT TERM
cat > "$tmp_path"
mv -f -- "$tmp_path" "$name"
trap - EXIT HUP INT TERM
"#;

pub(super) fn probe_remote_directory(connection: &SshConnection) -> Result<String, String> {
    resolve_remote_directory(connection, "")
}

pub(super) fn resolve_remote_directory(connection: &SshConnection, requested_path: &str) -> Result<String, String> {
    let output = run_ssh_script(connection, RESOLVE_REMOTE_DIR_SCRIPT, &[requested_path])?;
    resolved_directory(&output)
}

/// Resolves the destination of an upload over SSH, and ends at once on a `Cancel`.
///
/// The upload worker reads the directory of the destination before it sends the
/// first byte. That read opens its own `ssh`, so a connect on a slow link takes
/// seconds, and a person who changes their mind must not wait for it: the child
/// is polled and killed here, as the children of the other steps of the worker.
///
/// # Errors
/// Returns why the far side did not answer, or that the person cancelled.
pub(super) fn resolve_remote_directory_cancellable(
    connection: &SshConnection,
    requested_path: &str,
    control_rx: &Receiver<UploadControl>,
) -> Result<String, WorkerExit> {
    let remote_command = build_remote_shell_command(RESOLVE_REMOTE_DIR_SCRIPT, &[requested_path]);
    let mut command = Command::new("ssh");
    command
        .args(connection.ssh_probe_transport_args(SSH_PROBE_CONNECT_TIMEOUT_SECS))
        .arg(remote_command);
    let output = run_cancellable_command(command, control_rx)?;
    resolved_directory(&output).map_err(WorkerExit::Failed)
}

fn resolved_directory(output: &str) -> Result<String, String> {
    let resolved = output.trim();
    if resolved.is_empty() {
        return Err("remote directory probe returned an empty path".to_string());
    }
    Ok(resolved.to_string())
}

pub(super) fn list_remote_directories(
    connection: &SshConnection,
    requested_path: &str,
) -> Result<RemoteDirectoryListing, String> {
    let output = run_ssh_script(connection, LIST_REMOTE_DIRS_SCRIPT, &[requested_path])?;
    parse_remote_directory_listing(&output)
}

pub(super) fn run_ssh_upload(
    connection: &SshConnection,
    file: &LocalUploadFile,
    destination_dir: &str,
    progress: UploadProgressContext,
    control_rx: &Receiver<UploadControl>,
    progress_tx: &std::sync::mpsc::Sender<UploadMessage>,
) -> Result<(), WorkerExit> {
    let mut command = Command::new("ssh");
    command
        .args(connection.ssh_transport_args())
        .arg(build_remote_upload_command(destination_dir, &file.name));
    let detail = format!("Uploading {} over SSH", file.name);
    stream_file(&mut command, file, &detail, progress, control_rx, progress_tx)
}

/// One report of the thread that writes the file into the pipe of `ssh`.
enum WriteEvent {
    /// The bytes of the file that the pipe took so far.
    Written(u64),
    /// The whole file is in the pipe, and the pipe is closed.
    Done,
    /// The file was not sent, with the words for the person.
    Failed(String),
}

/// Streams `file` to the standard input of `command`, an SSH command that writes it on
/// the remote host, with progress and cancellation.
///
/// The file travels in a thread of its own, because a write into the pipe of
/// `ssh` waits while the far side is slow to read it: a stall of the link holds
/// that write for as long as the stall lasts. The loop here watches the child and
/// the control of the window instead, so `Cancel` ends the upload in a moment
/// also when the write is stuck.
pub(super) fn stream_file(
    command: &mut Command,
    file: &LocalUploadFile,
    detail: &str,
    progress: UploadProgressContext,
    control_rx: &Receiver<UploadControl>,
    progress_tx: &std::sync::mpsc::Sender<UploadMessage>,
) -> Result<(), WorkerExit> {
    send_upload_snapshot(progress_tx, progress.snapshot_for(file, 0, detail));

    let local_file = File::open(&file.path)
        .map_err(|error| WorkerExit::Failed(format!("failed to read {}: {error}", file.path.display())))?;

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| WorkerExit::Failed(format!("failed to start SSH upload: {error}")))?;

    let stdin = child.stdin.take().ok_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        WorkerExit::Failed("failed to capture SSH upload stdin".to_string())
    })?;

    let (events, reports) = mpsc::channel();
    let path = file.path.clone();
    // The thread is not joined: it ends when its write ends, and a `Cancel` closes
    // the pipe under it. It holds no handle of the child, so it cannot keep one
    // alive after this loop returns.
    let _writer = thread::spawn(move || write_file_to_pipe(&path, local_file, stdin, &events));

    let mut current_file_bytes = 0_u64;
    let mut last_snapshot_at = Instant::now();

    loop {
        poll_upload_control(&mut child, control_rx)?;

        let mut whole_file_sent = false;
        let mut writer_gone = false;
        loop {
            match reports.try_recv() {
                Ok(WriteEvent::Written(bytes)) => {
                    let first_bytes = current_file_bytes == 0 && bytes > 0;
                    current_file_bytes = bytes;
                    if first_bytes
                        || last_snapshot_at.elapsed() >= SSH_PROGRESS_INTERVAL
                        || current_file_bytes >= file.size_bytes
                    {
                        send_upload_snapshot(progress_tx, progress.snapshot_for(file, current_file_bytes, detail));
                        last_snapshot_at = Instant::now();
                    }
                }
                Ok(WriteEvent::Done) => whole_file_sent = true,
                Ok(WriteEvent::Failed(error)) => {
                    let _ = child.kill();
                    let output = super::child_output(&mut child);
                    let _ = child.wait();
                    return Err(WorkerExit::Failed(non_empty_output(&output).unwrap_or(error)));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    writer_gone = true;
                    break;
                }
            }
        }

        if whole_file_sent {
            send_upload_snapshot(progress_tx, progress.snapshot_for(file, file.size_bytes, detail));
            return wait_for_child(&mut child, control_rx);
        }
        if writer_gone {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorkerExit::Failed(format!(
                "the writer of {} stopped before the end of the file",
                file.name
            )));
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|error| WorkerExit::Failed(format!("failed to poll SSH upload: {error}")))?
        {
            return handle_early_child_exit(&mut child, status, current_file_bytes, file.size_bytes);
        }

        thread::sleep(SSH_UPLOAD_POLL_INTERVAL);
    }
}

/// Sends the whole of `file` into `pipe` and reports what the pipe took.
///
/// The pipe is dropped at the end, which is the end of the file for `ssh`. A
/// write that fails means that the far side closed the pipe, and the words of the
/// child are read by the caller, which holds the child.
fn write_file_to_pipe(path: &Path, mut file: File, mut pipe: ChildStdin, events: &Sender<WriteEvent>) {
    let mut buffer = vec![0_u8; SSH_UPLOAD_CHUNK_SIZE].into_boxed_slice();
    let mut written_total = 0_u64;

    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                let _ = events.send(WriteEvent::Failed(format!(
                    "failed to read {}: {error}",
                    path.display()
                )));
                return;
            }
        };

        let mut sent = 0;
        while sent < read {
            let end = (sent + SSH_WRITE_PIECE_SIZE).min(read);
            match pipe.write(&buffer[sent..end]) {
                Ok(0) => {
                    let _ = events.send(WriteEvent::Failed(format!(
                        "SSH upload for {} stopped accepting data",
                        path.display()
                    )));
                    return;
                }
                Ok(count) => {
                    sent += count;
                    written_total = written_total.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
                    if events.send(WriteEvent::Written(written_total)).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => {
                    let _ = events.send(WriteEvent::Failed(format!(
                        "failed to stream {} over SSH: {error}",
                        path.display()
                    )));
                    return;
                }
            }
        }
    }

    drop(pipe);
    let _ = events.send(WriteEvent::Done);
}

fn run_ssh_script(connection: &SshConnection, script: &str, args: &[&str]) -> Result<String, String> {
    let remote_command = build_remote_shell_command(script, args);
    let output = Command::new("ssh")
        .args(connection.ssh_probe_transport_args(SSH_PROBE_CONNECT_TIMEOUT_SECS))
        .arg(remote_command)
        .output()
        .map_err(|error| format!("failed to launch ssh probe: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = non_empty_output(stderr.as_ref()).unwrap_or_else(|| "ssh probe failed".to_string());
        return Err(detail);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Runs `command` to its end, and answers a `Cancel` of the window while it waits.
///
/// The child is polled instead of waited for, so a connect that holds for the
/// whole bound of the link still ends in a moment when the person presses
/// `Cancel`. The words of the run are its standard output; a failure carries the
/// standard error of the child, as the probe does.
fn run_cancellable_command(mut command: Command, control_rx: &Receiver<UploadControl>) -> Result<String, WorkerExit> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| WorkerExit::Failed(format!("failed to launch ssh upload setup: {error}")))?;

    loop {
        poll_upload_control(&mut child, control_rx)?;

        match child.try_wait() {
            Ok(Some(status)) => {
                // The child ended, so its pipes hold every word that it wrote.
                let output = super::child_output(&mut child);
                if !status.success() {
                    let detail =
                        non_empty_output(&output).unwrap_or_else(|| format!("command exited with status {status}"));
                    return Err(WorkerExit::Failed(detail));
                }
                return Ok(output);
            }
            Ok(None) => thread::sleep(SSH_POLL_INTERVAL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkerExit::Failed(format!("failed to wait for ssh: {error}")));
            }
        }
    }
}

fn build_remote_upload_command(destination_dir: &str, file_name: &str) -> String {
    build_remote_shell_command(WRITE_REMOTE_FILE_SCRIPT, &[destination_dir, file_name])
}

fn build_remote_shell_command(script: &str, args: &[&str]) -> String {
    let mut command = format!("sh -c {} --", shell_escape(script.trim()));
    for arg in args {
        command.push(' ');
        command.push_str(&shell_escape(arg));
    }
    command
}

fn parse_remote_directory_listing(output: &str) -> Result<RemoteDirectoryListing, String> {
    let mut current_dir = None;
    let mut entries = Vec::new();

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("__PWD__") {
            current_dir = Some(path.to_string());
        } else if let Some(entry) = line.strip_prefix("__DIR__") {
            entries.push(entry.to_string());
        }
    }

    let current_dir =
        current_dir.ok_or_else(|| "remote directory listing did not include a working directory".to_string())?;
    entries.sort_by(|left, right| sort_remote_entries(left, right));
    entries.dedup();

    Ok(RemoteDirectoryListing { current_dir, entries })
}

fn sort_remote_entries(left: &str, right: &str) -> std::cmp::Ordering {
    match (left == "..", right == "..") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase()),
    }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn handle_early_child_exit(
    child: &mut std::process::Child,
    status: std::process::ExitStatus,
    current_file_bytes: u64,
    expected_file_bytes: u64,
) -> Result<(), WorkerExit> {
    if status.success() && current_file_bytes >= expected_file_bytes {
        return Ok(());
    }

    let output = super::child_output(child);
    let fallback = if status.success() {
        "SSH upload ended before all local bytes were sent".to_string()
    } else {
        format!("command exited with status {status}")
    };
    Err(WorkerExit::Failed(non_empty_output(&output).unwrap_or(fallback)))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        build_remote_shell_command, build_remote_upload_command, parse_remote_directory_listing,
        run_cancellable_command, stream_file,
    };
    use crate::app::ssh_upload::worker::{LocalUploadFile, UploadControl, UploadProgressContext, WorkerExit};

    /// The bound of a wait for an event of the test. It is far above the work that
    /// the event needs, so a slow computer does not read as a broken upload.
    const EVIDENCE_BOUND: Duration = Duration::from_secs(5);

    /// The time that the person must see for a `Cancel`.
    const CANCEL_BOUND: Duration = Duration::from_secs(1);

    /// B9: `Cancel` of an upload answers within a second while the write of the
    /// file waits for a far side that stopped reading it.
    ///
    /// The far side of this test is one `sh` that reads a megabyte of its standard
    /// input and then stops reading, which is what a stalled link looks like to the
    /// pipe of `ssh`: the pipe takes at most one buffer and holds every later
    /// write. It writes a marker file before it stops, so the test waits for an
    /// event and not for a guess. The `sleep` is short, so a build that ignores the
    /// cancel ends by itself instead of leaving a process.
    #[test]
    fn a_cancel_ends_an_upload_whose_write_is_stalled() {
        let home = tempfile::tempdir().expect("a directory");
        let marker = home.path().join("the far side stopped reading");
        let path = home.path().join("image.bin");
        let size_bytes: u64 = 4 * 1024 * 1024;
        std::fs::write(&path, vec![7_u8; 4 * 1024 * 1024]).expect("the file of the test");
        let file = LocalUploadFile {
            path,
            name: "image.bin".to_string(),
            size_bytes,
        };
        let progress = UploadProgressContext {
            completed_files: 0,
            total_files: 1,
            completed_bytes: 0,
            total_bytes: size_bytes,
        };
        let (control_tx, control_rx) = mpsc::channel();
        let (progress_tx, _progress_rx) = mpsc::channel();

        // `exec` makes the `sleep` the child of this test, so the cancel kills it
        // and no process of the test outlives the test.
        let script = format!(
            "head -c 1048576 > /dev/null && touch '{}' && exec sleep 20",
            marker.display()
        );
        let mut command = Command::new("sh");
        command.args(["-c", script.as_str()]);

        let person = thread::spawn(move || {
            wait_for_path(&marker);
            let asked_at = Instant::now();
            control_tx
                .send(UploadControl::Cancel)
                .expect("the control of the upload");
            asked_at
        });

        let streamed = stream_file(
            &mut command,
            &file,
            "Uploading image.bin over SSH",
            progress,
            &control_rx,
            &progress_tx,
        );

        let asked_at = person.join().expect("the thread of the person");
        let answered = asked_at.elapsed();

        assert!(
            matches!(streamed, Err(WorkerExit::Cancelled)),
            "the upload did not end as cancelled"
        );
        assert!(answered <= CANCEL_BOUND, "the cancel answered after {answered:?}");
    }

    /// Waits until `path` exists, and fails the test when it never does.
    fn wait_for_path(path: &std::path::Path) {
        let deadline = Instant::now() + EVIDENCE_BOUND;
        while !path.exists() {
            assert!(Instant::now() < deadline, "{} never appeared", path.display());
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// B9: `Cancel` of an upload answers within a second while the child that
    /// reads the remote directory is still in its connect.
    ///
    /// The child of this test writes a marker file and then says nothing, which is
    /// what `/usr/bin/ssh` looks like while it waits for a handshake of a link that
    /// is slow or black-holed.
    #[test]
    fn a_cancel_ends_the_setup_of_an_upload_that_is_still_connecting() {
        let home = tempfile::tempdir().expect("a directory");
        let marker = home.path().join("the child started");
        // `exec` makes the `sleep` the child of this test, so the cancel kills it
        // and no process of the test outlives the test.
        let script = format!("touch '{}' && exec sleep 20", marker.display());
        let mut command = Command::new("sh");
        command.args(["-c", script.as_str()]);

        let (control_tx, control_rx) = mpsc::channel();
        let person = thread::spawn(move || run_cancellable_command(command, &control_rx));

        wait_for_path(&marker);

        let asked_at = Instant::now();
        control_tx
            .send(UploadControl::Cancel)
            .expect("the control of the upload");
        let asked = person.join().expect("the thread of the child");
        let answered = asked_at.elapsed();

        assert!(
            matches!(asked, Err(WorkerExit::Cancelled)),
            "the setup did not end as cancelled"
        );
        assert!(answered <= CANCEL_BOUND, "the cancel answered after {answered:?}");
    }

    #[test]
    fn parse_remote_directory_listing_sorts_parent_first() {
        let listing =
            parse_remote_directory_listing("__PWD__/srv\n__DIR__logs\n__DIR__..\n__DIR__.cache\n").expect("listing");

        assert_eq!(listing.current_dir, "/srv");
        assert_eq!(listing.entries, vec!["..", ".cache", "logs"]);
    }

    #[test]
    fn build_remote_shell_command_quotes_all_arguments() {
        let command = build_remote_shell_command("printf '%s %s' \"$1\" \"$2\"", &["/tmp/with space", "file name.txt"]);

        assert_eq!(
            command,
            "sh -c 'printf '\"'\"'%s %s'\"'\"' \"$1\" \"$2\"' -- '/tmp/with space' 'file name.txt'"
        );
    }

    #[test]
    fn build_remote_upload_command_keeps_destination_and_name_separate() {
        let command = build_remote_upload_command("/tmp/with space", "report 1.txt");

        assert!(command.ends_with("-- '/tmp/with space' 'report 1.txt'"));
    }
}
