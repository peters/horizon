use std::fs::File;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use horizon_core::SshConnection;

use super::{
    LocalUploadFile, RemoteDirectoryListing, SSH_PROBE_CONNECT_TIMEOUT_SECS, UploadControl, UploadMessage,
    UploadProgressContext, WorkerExit, non_empty_output, poll_upload_control, send_upload_snapshot, wait_for_child,
};

const SSH_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const SSH_UPLOAD_CHUNK_SIZE: usize = 256 * 1024;

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
    let detail = format!("Uploading {} over SSH", file.name);
    send_upload_snapshot(progress_tx, progress.snapshot_for(file, 0, &detail));

    let remote_command = build_remote_upload_command(destination_dir, &file.name);
    let mut child = Command::new("ssh")
        .args(connection.ssh_transport_args())
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| WorkerExit::Failed(format!("failed to start SSH upload: {error}")))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| WorkerExit::Failed("failed to capture SSH upload stdin".to_string()))?;
    let mut local_file = File::open(&file.path)
        .map_err(|error| WorkerExit::Failed(format!("failed to read {}: {error}", file.path.display())))?;

    let mut buffer = vec![0_u8; SSH_UPLOAD_CHUNK_SIZE].into_boxed_slice();
    let mut current_file_bytes = 0_u64;
    let mut last_snapshot_at = Instant::now();

    loop {
        poll_upload_control(&mut child, control_rx)?;
        if let Some(status) = child
            .try_wait()
            .map_err(|error| WorkerExit::Failed(format!("failed to poll SSH upload: {error}")))?
        {
            return handle_early_child_exit(&mut child, status, current_file_bytes, file.size_bytes);
        }

        let read = local_file
            .read(&mut buffer)
            .map_err(|error| WorkerExit::Failed(format!("failed to read {}: {error}", file.path.display())))?;
        if read == 0 {
            break;
        }

        let mut written = 0;
        while written < read {
            poll_upload_control(&mut child, control_rx)?;
            if let Some(status) = child
                .try_wait()
                .map_err(|error| WorkerExit::Failed(format!("failed to poll SSH upload: {error}")))?
            {
                return handle_early_child_exit(&mut child, status, current_file_bytes, file.size_bytes);
            }

            let count = stdin.write(&buffer[written..read]).map_err(|error| {
                let _ = child.wait();
                let output = super::child_output(&mut child);
                let fallback = format!("failed to stream {} over SSH: {error}", file.name);
                WorkerExit::Failed(non_empty_output(&output).unwrap_or(fallback))
            })?;

            if count == 0 {
                let _ = child.wait();
                let output = super::child_output(&mut child);
                let detail = non_empty_output(&output)
                    .unwrap_or_else(|| format!("SSH upload for {} stopped accepting data", file.name));
                return Err(WorkerExit::Failed(detail));
            }

            written += count;
            current_file_bytes = current_file_bytes.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));

            if last_snapshot_at.elapsed() >= SSH_PROGRESS_INTERVAL || current_file_bytes >= file.size_bytes {
                send_upload_snapshot(progress_tx, progress.snapshot_for(file, current_file_bytes, &detail));
                last_snapshot_at = Instant::now();
            }
        }
    }

    drop(stdin);
    send_upload_snapshot(progress_tx, progress.snapshot_for(file, file.size_bytes, &detail));
    wait_for_child(&mut child, control_rx)
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
    use super::{build_remote_shell_command, build_remote_upload_command, parse_remote_directory_listing};

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
