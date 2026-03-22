use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use horizon_core::SshConnection;

const SSH_POLL_INTERVAL: Duration = Duration::from_millis(100);

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LocalUploadFile {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
}

impl LocalUploadFile {
    fn from_path(path: PathBuf) -> Result<Self, String> {
        let metadata = fs::metadata(&path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map_or_else(|| path.display().to_string(), ToOwned::to_owned);

        Ok(Self {
            path,
            name,
            size_bytes: metadata.len(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PreparationResult {
    pub suggested_destination: String,
    pub taildrop_target: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteDirectoryListing {
    pub current_dir: String,
    pub entries: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UploadTransport {
    Ssh { destination_dir: String },
    Taildrop { target: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UploadSnapshot {
    pub completed_files: usize,
    pub total_files: usize,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub current_file_name: Option<String>,
    pub current_file_size: Option<u64>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UploadOutcome {
    pub cancelled: bool,
    pub completed_files: usize,
    pub total_files: usize,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub detail: String,
}

pub(super) enum UploadMessage {
    Snapshot(UploadSnapshot),
    Finished(Result<UploadOutcome, String>),
}

enum UploadControl {
    Cancel,
}

pub(super) struct UploadWorkerHandle {
    pub progress_rx: Receiver<UploadMessage>,
    control_tx: Sender<UploadControl>,
}

impl UploadWorkerHandle {
    pub fn cancel(&self) {
        let _ = self.control_tx.send(UploadControl::Cancel);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TaildropTarget {
    ip: String,
    name: Option<String>,
}

pub(super) fn build_local_upload_files(paths: Vec<PathBuf>) -> Result<Vec<LocalUploadFile>, String> {
    let mut files = Vec::new();
    for path in paths {
        files.push(LocalUploadFile::from_path(path)?);
    }

    if files.is_empty() {
        return Err("only local filesystem file drops are supported".to_string());
    }

    Ok(files)
}

pub(super) fn spawn_preparation(
    connection: SshConnection,
    last_destination: Option<String>,
) -> Receiver<Result<PreparationResult, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let taildrop_target = detect_taildrop_target(&connection.host).unwrap_or(None);
        let suggested_destination = match last_destination {
            Some(path) if !path.trim().is_empty() => path,
            _ => probe_remote_directory(&connection).unwrap_or_else(|_| "~".to_string()),
        };

        let _ = tx.send(Ok(PreparationResult {
            suggested_destination,
            taildrop_target,
        }));
    });
    rx
}

pub(super) fn spawn_remote_directory_listing(
    connection: SshConnection,
    requested_path: String,
) -> Receiver<Result<RemoteDirectoryListing, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(list_remote_directories(&connection, &requested_path));
    });
    rx
}

pub(super) fn start_upload(
    connection: SshConnection,
    files: Vec<LocalUploadFile>,
    transport: UploadTransport,
) -> UploadWorkerHandle {
    let (progress_tx, progress_rx) = mpsc::channel();
    let (control_tx, control_rx) = mpsc::channel();

    thread::spawn(move || run_upload_worker(&connection, &files, &transport, &control_rx, &progress_tx));

    UploadWorkerHandle {
        progress_rx,
        control_tx,
    }
}

fn run_upload_worker(
    connection: &SshConnection,
    files: &[LocalUploadFile],
    transport: &UploadTransport,
    control_rx: &Receiver<UploadControl>,
    progress_tx: &Sender<UploadMessage>,
) {
    let total_bytes = files.iter().map(|file| file.size_bytes).sum();
    let total_files = files.len();
    let mut completed_files = 0;
    let mut completed_bytes = 0;

    let resolved_destination = match &transport {
        UploadTransport::Ssh { destination_dir } => match resolve_remote_directory(connection, destination_dir) {
            Ok(path) => Some(path),
            Err(error) => {
                let _ = progress_tx.send(UploadMessage::Finished(Err(error)));
                return;
            }
        },
        UploadTransport::Taildrop { .. } => None,
    };

    for file in files {
        let snapshot = UploadSnapshot {
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
            current_file_name: Some(file.name.clone()),
            current_file_size: Some(file.size_bytes),
            detail: match &transport {
                UploadTransport::Ssh { .. } => format!("Uploading {} over SSH", file.name),
                UploadTransport::Taildrop { .. } => format!("Sending {} with Taildrop", file.name),
            },
        };
        let _ = progress_tx.send(UploadMessage::Snapshot(snapshot));

        let command_result = match &transport {
            UploadTransport::Ssh { .. } => {
                run_scp_upload(connection, file, resolved_destination.as_deref().unwrap_or(""))
            }
            UploadTransport::Taildrop { target } => run_taildrop_upload(file, target),
        };

        match wait_for_command(command_result, control_rx) {
            Ok(()) => {
                completed_files += 1;
                completed_bytes += file.size_bytes;
            }
            Err(WorkerExit::Cancelled) => {
                let _ = progress_tx.send(UploadMessage::Finished(Ok(UploadOutcome {
                    cancelled: true,
                    completed_files,
                    total_files,
                    completed_bytes,
                    total_bytes,
                    detail: format!("Cancelled after {completed_files} of {total_files} files"),
                })));
                return;
            }
            Err(WorkerExit::Failed(error)) => {
                let _ = progress_tx.send(UploadMessage::Finished(Err(error)));
                return;
            }
        }
    }

    let _ = progress_tx.send(UploadMessage::Finished(Ok(UploadOutcome {
        cancelled: false,
        completed_files,
        total_files,
        completed_bytes,
        total_bytes,
        detail: format!("Uploaded {completed_files} file(s)"),
    })));
}

enum WorkerExit {
    Cancelled,
    Failed(String),
}

fn wait_for_command(
    spawn_result: Result<std::process::Child, String>,
    control_rx: &Receiver<UploadControl>,
) -> Result<(), WorkerExit> {
    let mut child = spawn_result.map_err(WorkerExit::Failed)?;

    loop {
        match control_rx.try_recv() {
            Ok(UploadControl::Cancel) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(WorkerExit::Cancelled);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Err(WorkerExit::Cancelled),
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child_output(&mut child);
                if status.success() {
                    return Ok(());
                }

                let detail =
                    non_empty_output(&output).unwrap_or_else(|| format!("command exited with status {status}"));
                return Err(WorkerExit::Failed(detail));
            }
            Ok(None) => thread::sleep(SSH_POLL_INTERVAL),
            Err(error) => return Err(WorkerExit::Failed(format!("failed to wait for upload: {error}"))),
        }
    }
}

fn child_output(child: &mut std::process::Child) -> String {
    let mut output = String::new();

    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let mut stderr_output = String::new();
        let _ = stderr.read_to_string(&mut stderr_output);
        if !stderr_output.trim().is_empty() {
            if !output.trim().is_empty() {
                output.push('\n');
            }
            output.push_str(&stderr_output);
        }
    }

    output
}

fn non_empty_output(output: &str) -> Option<String> {
    let trimmed = output.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn probe_remote_directory(connection: &SshConnection) -> Result<String, String> {
    resolve_remote_directory(connection, "")
}

fn resolve_remote_directory(connection: &SshConnection, requested_path: &str) -> Result<String, String> {
    let output = run_ssh_script(connection, RESOLVE_REMOTE_DIR_SCRIPT, requested_path)?;
    let resolved = output.trim();
    if resolved.is_empty() {
        return Err("remote directory probe returned an empty path".to_string());
    }
    Ok(resolved.to_string())
}

fn list_remote_directories(connection: &SshConnection, requested_path: &str) -> Result<RemoteDirectoryListing, String> {
    let output = run_ssh_script(connection, LIST_REMOTE_DIRS_SCRIPT, requested_path)?;
    parse_remote_directory_listing(&output)
}

fn run_ssh_script(connection: &SshConnection, script: &str, arg: &str) -> Result<String, String> {
    let remote_command = build_remote_shell_command(script, arg);
    let output = Command::new("ssh")
        .args(connection.ssh_transport_args())
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

fn build_remote_shell_command(script: &str, arg: &str) -> String {
    format!("sh -lc {} -- {}", shell_escape(script.trim()), shell_escape(arg),)
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

fn detect_taildrop_target(host: &str) -> Result<Option<String>, String> {
    let ip_output = Command::new("tailscale").args(["ip", "-4", host]).output();

    let ip_output = match ip_output {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to run tailscale ip: {error}")),
    };

    let ip = String::from_utf8_lossy(&ip_output.stdout).trim().to_string();
    if ip.is_empty() {
        return Ok(None);
    }

    let targets_output = Command::new("tailscale").args(["file", "cp", "--targets"]).output();

    let targets_output = match targets_output {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to query Taildrop targets: {error}")),
    };

    let targets = parse_taildrop_targets(&String::from_utf8_lossy(&targets_output.stdout));
    Ok(targets
        .into_iter()
        .find(|target| target.ip == ip)
        .map(|target| target.name.unwrap_or(target.ip)))
}

fn parse_taildrop_targets(output: &str) -> Vec<TaildropTarget> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }

            let mut fields = trimmed.split_whitespace();
            let ip = fields.next()?.to_string();
            let name = fields.next().map(ToOwned::to_owned);
            Some(TaildropTarget { ip, name })
        })
        .collect()
}

fn run_scp_upload(
    connection: &SshConnection,
    file: &LocalUploadFile,
    destination_dir: &str,
) -> Result<std::process::Child, String> {
    Command::new("scp")
        .args(connection.scp_transport_args())
        .arg(&file.path)
        .arg(scp_destination(connection, destination_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start scp upload: {error}"))
}

fn run_taildrop_upload(file: &LocalUploadFile, target: &str) -> Result<std::process::Child, String> {
    Command::new("tailscale")
        .args(["file", "cp"])
        .arg(&file.path)
        .arg(format!("{target}:"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start Taildrop upload: {error}"))
}

fn scp_destination(connection: &SshConnection, destination_dir: &str) -> String {
    format!(
        "{}:{}",
        connection.transport_target(),
        scp_quote_path(Path::new(destination_dir)),
    )
}

fn scp_quote_path(path: &Path) -> String {
    shell_escape(&path.display().to_string())
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{build_remote_shell_command, parse_remote_directory_listing, parse_taildrop_targets, scp_quote_path};

    #[test]
    fn parse_taildrop_targets_reads_ip_and_name() {
        let targets = parse_taildrop_targets("100.70.83.123\tfinter-sin-mac-studio-1\n");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ip, "100.70.83.123");
        assert_eq!(targets[0].name.as_deref(), Some("finter-sin-mac-studio-1"));
    }

    #[test]
    fn parse_remote_directory_listing_sorts_parent_first() {
        let listing =
            parse_remote_directory_listing("__PWD__/srv\n__DIR__logs\n__DIR__..\n__DIR__.cache\n").expect("listing");

        assert_eq!(listing.current_dir, "/srv");
        assert_eq!(listing.entries, vec!["..", ".cache", "logs"]);
    }

    #[test]
    fn build_remote_shell_command_quotes_script_and_argument() {
        let command = build_remote_shell_command("printf '%s' \"$1\"", "/tmp/with space");

        assert_eq!(command, "sh -lc 'printf '\"'\"'%s'\"'\"' \"$1\"' -- '/tmp/with space'");
    }

    #[test]
    fn scp_quote_path_wraps_spaces_in_single_quotes() {
        assert_eq!(scp_quote_path(Path::new("/tmp/with space")), "'/tmp/with space'");
    }
}
