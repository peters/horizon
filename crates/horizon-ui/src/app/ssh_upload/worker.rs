mod ssh;

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use horizon_core::SshConnection;

const SSH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SSH_PROBE_CONNECT_TIMEOUT_SECS: u16 = 5;

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
    pub suggested_destination: Option<String>,
    pub ssh_upload_error: Option<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UploadProgressContext {
    completed_files: usize,
    total_files: usize,
    completed_bytes: u64,
    total_bytes: u64,
}

impl UploadProgressContext {
    fn snapshot_for(self, file: &LocalUploadFile, current_file_bytes: u64, detail: &str) -> UploadSnapshot {
        UploadSnapshot {
            completed_files: self.completed_files,
            total_files: self.total_files,
            completed_bytes: self
                .completed_bytes
                .saturating_add(current_file_bytes.min(file.size_bytes)),
            total_bytes: self.total_bytes,
            current_file_name: Some(file.name.clone()),
            current_file_size: Some(file.size_bytes),
            detail: detail.to_string(),
        }
    }
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
        let host = connection.host.clone();
        let taildrop_target = match detect_taildrop_target(&host) {
            Ok(target) => target,
            Err(error) => {
                tracing::warn!(host = %host, %error, "failed to detect Taildrop target");
                None
            }
        };
        let (suggested_destination, ssh_upload_error) = match ssh::probe_remote_directory(&connection) {
            Ok(probed_directory) => {
                let suggested_destination = match last_destination {
                    Some(path) if !path.trim().is_empty() => path,
                    _ => probed_directory,
                };
                tracing::debug!(host = %host, destination = %suggested_destination, "ssh upload available");
                (Some(suggested_destination), None)
            }
            Err(error) => {
                let ssh_upload_error = classify_ssh_probe_error(&error);
                tracing::warn!(host = %host, %error, "ssh upload unavailable");
                (None, Some(ssh_upload_error))
            }
        };

        let _ = tx.send(Ok(PreparationResult {
            suggested_destination,
            ssh_upload_error,
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
        let host = connection.host.clone();
        let result = ssh::list_remote_directories(&connection, &requested_path);
        if let Err(error) = &result {
            tracing::warn!(host = %host, path = %requested_path, %error, "remote directory listing failed");
        }
        let _ = tx.send(result);
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
    tracing::debug!(
        host = %connection.host,
        file_count = files.len(),
        transport = ?transport,
        "starting upload worker",
    );
    let total_bytes = files.iter().map(|file| file.size_bytes).sum();
    let total_files = files.len();
    let mut completed_files = 0;
    let mut completed_bytes = 0;

    let resolved_destination = match &transport {
        UploadTransport::Ssh { destination_dir } => match ssh::resolve_remote_directory(connection, destination_dir) {
            Ok(path) => Some(path),
            Err(error) => {
                tracing::warn!(host = %connection.host, destination = %destination_dir, %error, "ssh upload setup failed");
                let _ = progress_tx.send(UploadMessage::Finished(Err(error)));
                return;
            }
        },
        UploadTransport::Taildrop { .. } => None,
    };

    for file in files {
        let progress = UploadProgressContext {
            completed_files,
            total_files,
            completed_bytes,
            total_bytes,
        };
        let upload_result = match &transport {
            UploadTransport::Ssh { .. } => ssh::run_ssh_upload(
                connection,
                file,
                resolved_destination.as_deref().unwrap_or(""),
                progress,
                control_rx,
                progress_tx,
            ),
            UploadTransport::Taildrop { target } => {
                send_upload_snapshot(
                    progress_tx,
                    progress.snapshot_for(file, 0, &format!("Sending {} with Taildrop", file.name)),
                );
                wait_for_command(run_taildrop_upload(file, target), control_rx)
            }
        };

        match upload_result {
            Ok(()) => {
                completed_files += 1;
                completed_bytes += file.size_bytes;
            }
            Err(WorkerExit::Cancelled) => {
                tracing::debug!(host = %connection.host, file = %file.name, "upload cancelled");
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
                tracing::warn!(host = %connection.host, file = %file.name, %error, "upload command failed");
                let _ = progress_tx.send(UploadMessage::Finished(Err(error)));
                return;
            }
        }
    }

    tracing::debug!(host = %connection.host, completed_files, "upload worker finished");
    let _ = progress_tx.send(UploadMessage::Finished(Ok(UploadOutcome {
        cancelled: false,
        completed_files,
        total_files,
        completed_bytes,
        total_bytes,
        detail: format!("Uploaded {completed_files} file(s)"),
    })));
}

fn send_upload_snapshot(progress_tx: &Sender<UploadMessage>, snapshot: UploadSnapshot) {
    let _ = progress_tx.send(UploadMessage::Snapshot(snapshot));
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

    wait_for_child(&mut child, control_rx)
}

fn wait_for_child(child: &mut std::process::Child, control_rx: &Receiver<UploadControl>) -> Result<(), WorkerExit> {
    loop {
        poll_upload_control(child, control_rx)?;

        match child.try_wait() {
            Ok(Some(status)) => return finish_child(child, status),
            Ok(None) => thread::sleep(SSH_POLL_INTERVAL),
            Err(error) => return Err(WorkerExit::Failed(format!("failed to wait for upload: {error}"))),
        }
    }
}

fn poll_upload_control(
    child: &mut std::process::Child,
    control_rx: &Receiver<UploadControl>,
) -> Result<(), WorkerExit> {
    match control_rx.try_recv() {
        Ok(UploadControl::Cancel) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(WorkerExit::Cancelled)
        }
        Err(TryRecvError::Empty) => Ok(()),
        Err(TryRecvError::Disconnected) => Err(WorkerExit::Cancelled),
    }
}

fn finish_child(child: &mut std::process::Child, status: std::process::ExitStatus) -> Result<(), WorkerExit> {
    let output = child_output(child);
    if status.success() {
        return Ok(());
    }

    let detail = non_empty_output(&output).unwrap_or_else(|| format!("command exited with status {status}"));
    Err(WorkerExit::Failed(detail))
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

fn detect_taildrop_target(host: &str) -> Result<Option<String>, String> {
    let ip_output = match Command::new("tailscale").args(["ip", host]).output() {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to run tailscale ip: {error}")),
    };

    let ips = parse_tailscale_ips(&String::from_utf8_lossy(&ip_output.stdout));
    if ips.is_empty() {
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
        .find(|target| ips.iter().any(|ip| ip == &target.ip))
        .map(|target| target.name.unwrap_or(target.ip)))
}

fn parse_tailscale_ips(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
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

fn classify_ssh_probe_error(error: &str) -> String {
    if requires_interactive_ssh_auth(error) {
        return "SSH upload requires non-interactive authentication for new SSH upload commands. This session appears to need a password, OTP, or hardware-key prompt, so drag-and-drop SSH upload is unavailable.".to_string();
    }

    format!("SSH upload is unavailable for this session: {error}")
}

fn requires_interactive_ssh_auth(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "permission denied",
        "keyboard-interactive",
        "password:",
        "passphrase",
        "verification code",
        "one-time password",
        "confirm user presence",
        "agent refused operation",
        "sign_and_send_pubkey",
        "batchmode",
        "no tty present",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{UploadProgressContext, classify_ssh_probe_error, parse_taildrop_targets, parse_tailscale_ips};
    use crate::app::ssh_upload::worker::LocalUploadFile;
    use std::path::PathBuf;

    #[test]
    fn parse_taildrop_targets_reads_ip_and_name() {
        let targets = parse_taildrop_targets("100.70.83.123\tfinter-sin-mac-studio-1\n");

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ip, "100.70.83.123");
        assert_eq!(targets[0].name.as_deref(), Some("finter-sin-mac-studio-1"));
    }

    #[test]
    fn progress_context_clamps_current_file_bytes_to_file_size() {
        let file = LocalUploadFile {
            path: PathBuf::from("large.bin"),
            name: "large.bin".to_string(),
            size_bytes: 1024,
        };
        let progress = UploadProgressContext {
            completed_files: 1,
            total_files: 3,
            completed_bytes: 512,
            total_bytes: 4096,
        };

        let snapshot = progress.snapshot_for(&file, 4096, "Uploading large.bin over SSH");

        assert_eq!(snapshot.completed_bytes, 1536);
        assert_eq!(snapshot.current_file_name.as_deref(), Some("large.bin"));
    }

    #[test]
    fn parse_tailscale_ips_reads_ipv4_and_ipv6_lines() {
        let ips = parse_tailscale_ips("100.70.83.123\nfd7a:115c:a1e0::123\n");

        assert_eq!(ips, vec!["100.70.83.123", "fd7a:115c:a1e0::123"]);
    }

    #[test]
    fn classify_ssh_probe_error_marks_permission_denied_as_interactive_auth() {
        let error = classify_ssh_probe_error("Permission denied (publickey,keyboard-interactive).");

        assert!(error.contains("requires non-interactive authentication"));
    }
}
