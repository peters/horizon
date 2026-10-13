//! Sends dropped files to a cloud worker session over the worker's pinned SSH connection.
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use horizon_core::cloud_runtime::file_drop::{Batch, Destination, remote_names};
use horizon_core::cloud_runtime::ssh::Connection;

use super::{
    LocalUploadFile, SSH_POLL_INTERVAL, UploadControl, UploadMessage, UploadOutcome, UploadProgressContext,
    UploadSnapshot, UploadWorkerHandle, WorkerExit, build_local_upload_files, send_upload_snapshot, ssh::stream_file,
    wait_for_command,
};

/// How long a failed or cancelled drop waits for the worker to remove its partial folder.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(20);

pub(in crate::app::ssh_upload) fn start_cloud_upload(
    destination: Destination,
    paths: Vec<PathBuf>,
) -> UploadWorkerHandle {
    let (progress_tx, progress_rx) = mpsc::channel();
    let (control_tx, control_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = upload(&destination, paths, &control_rx, &progress_tx);
        let _ = progress_tx.send(UploadMessage::Finished(result));
    });
    UploadWorkerHandle {
        progress_rx,
        control_tx,
    }
}

fn upload(
    destination: &Destination,
    paths: Vec<PathBuf>,
    control_rx: &Receiver<UploadControl>,
    progress_tx: &Sender<UploadMessage>,
) -> Result<UploadOutcome, String> {
    let files = local_files(paths)?;
    let names = remote_names(&files.iter().map(|file| file.path.clone()).collect::<Vec<_>>());
    let total_files = files.len();
    let total_bytes = files.iter().map(|file| file.size_bytes).sum();
    let connection = destination
        .connect()
        .map_err(|error| format!("Horizon could not connect to the cloud worker: {error}"))?;
    let batch = destination.batch();
    send_upload_snapshot(
        progress_tx,
        UploadSnapshot {
            completed_files: 0,
            total_files,
            completed_bytes: 0,
            total_bytes,
            current_file_name: None,
            current_file_size: None,
            detail: "Preparing a folder on the cloud worker".to_string(),
        },
    );
    let mut progress = UploadProgressContext {
        completed_files: 0,
        total_files,
        completed_bytes: 0,
        total_bytes,
    };
    let prepare = connection.pinned_command(&batch.prepare_command());
    if let Err(exit) = wait_for_command(spawn(prepare), control_rx) {
        // A failed preparation made no folder; a cancelled one may have.
        if matches!(exit, WorkerExit::Cancelled) {
            remove(&connection, &batch);
        }
        return stopped(exit, progress, "");
    }
    let mut remote_paths = Vec::with_capacity(total_files);
    for (file, name) in files.iter().zip(&names) {
        let mut command = connection.pinned_command(&batch.write_command(name, file.size_bytes));
        let detail = format!("Uploading {} to the cloud worker", file.name);
        if let Err(exit) = stream_file(&mut command, file, &detail, progress, control_rx, progress_tx) {
            let left = if remove(&connection, &batch) {
                String::new()
            } else {
                format!(" Horizon could not remove {} from the worker.", batch.directory())
            };
            return stopped(exit, progress, &left);
        }
        progress.completed_files += 1;
        progress.completed_bytes += file.size_bytes;
        remote_paths.push(batch.path(name));
    }
    tracing::debug!(files = total_files, folder = %batch.directory(), "cloud file drop uploaded");
    Ok(UploadOutcome {
        cancelled: false,
        completed_files: progress.completed_files,
        total_files,
        completed_bytes: progress.completed_bytes,
        total_bytes,
        detail: format!("Uploaded {total_files} file(s) to the cloud worker"),
        remote_paths,
    })
}

/// Folders are refused before anything is sent: a drop uploads files only.
fn local_files(paths: Vec<PathBuf>) -> Result<Vec<LocalUploadFile>, String> {
    if let Some(folder) = paths.iter().find(|path| path.is_dir()) {
        return Err(format!(
            "{} is a folder. Horizon uploads only files to a cloud worker: drop the files in the folder, or an archive of it.",
            folder.display()
        ));
    }
    build_local_upload_files(paths)
}

fn spawn(mut command: Command) -> Result<std::process::Child, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start SSH: {error}"))
}

/// Why the drop stopped; `left` names a partial folder that stayed on the worker.
fn stopped(exit: WorkerExit, progress: UploadProgressContext, left: &str) -> Result<UploadOutcome, String> {
    match exit {
        WorkerExit::Cancelled => Ok(UploadOutcome {
            cancelled: true,
            completed_files: progress.completed_files,
            total_files: progress.total_files,
            completed_bytes: progress.completed_bytes,
            total_bytes: progress.total_bytes,
            detail: format!(
                "Cancelled after {} of {} files. No path was inserted.{left}",
                progress.completed_files, progress.total_files
            ),
            remote_paths: Vec::new(),
        }),
        WorkerExit::Failed(error) => Err(format!("{}{left}", explain(&error))),
    }
}

fn remove(connection: &Connection, batch: &Batch) -> bool {
    let mut command = connection.pinned_command(&batch.remove_command());
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(SSH_POLL_INTERVAL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!(folder = %batch.directory(), "cloud file drop cleanup did not finish");
                return false;
            }
        }
    }
}

/// Puts what the person can do first, then what SSH or the worker said.
fn explain(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    let advice = if lower.contains("host key verification failed") || lower.contains("host identification has changed")
    {
        "The worker did not show the host key that Horizon recorded for this cloud, so Horizon sent nothing. Reconnect the cloud, then drop the files again."
    } else if lower.contains("permission denied") {
        "The worker refused the cloud SSH key of this computer. Reconnect the cloud, then drop the files again."
    } else if [
        "connection refused",
        "timed out",
        "could not resolve",
        "no route to host",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        "Horizon could not reach the cloud worker. Make sure that the cloud runs, then drop the files again."
    } else {
        return format!("The cloud worker did not take the files: {}", error.trim());
    };
    format!("{advice}\n\n{}", error.trim())
}

#[cfg(test)]
mod tests {
    use super::{explain, local_files};

    #[test]
    fn a_folder_is_refused_before_anything_is_sent() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("notes.txt");
        std::fs::write(&file, "notes").unwrap();
        let error = local_files(vec![file.clone(), temp.path().to_path_buf()]).unwrap_err();
        assert!(error.contains("is a folder"), "{error}");
        assert_eq!(local_files(vec![file]).unwrap()[0].size_bytes, 5);
    }

    #[test]
    fn failures_start_with_what_to_do() {
        assert!(explain("Host key verification failed.\n").starts_with("The worker did not show the host key"));
        assert!(explain("ssh: connect to host 192.0.2.1 port 22: Connection refused").contains("could not reach"));
        assert_eq!(
            explain("The worker has no session for this panel.\n"),
            "The cloud worker did not take the files: The worker has no session for this panel."
        );
    }
}
