---
applyTo: "crates/horizon-core/src/cloud_runtime/file_drop.rs,crates/horizon-core/src/cloud_runtime/file_drop/**,crates/horizon-ui/src/app/ssh_upload/cloud.rs,crates/horizon-ui/src/app/ssh_upload/cloud/**,crates/horizon-ui/src/app/ssh_upload/worker/cloud.rs,crates/horizon-ui/src/app/cloud_panel/production/readiness/file_drop.rs,crates/horizon-ui/src/app/cloud_panel/production/readiness/file_drop/**"
---

# Trust boundary of the cloud file drop

A person drops local files on a cloud agent or shell panel. Horizon sends them
to the worker of that panel over the cloud's SSH connection, then pastes their
worker paths into the panel. The design is in the module comments and in the
section "Dropped files" of `examples/cloud-worker/README.md`.

## Trusted: the person and the local files

The person chooses the files, so their names, sizes and contents are the
person's own input. Horizon does not examine the contents.

- A name goes to the worker only as one quoted shell argument. Control
  characters become `_`, and the name is at most 255 bytes.
- The paths go into the panel as one paste, with POSIX quotes.

## Untrusted: the worker and its agents

An agent on the worker can be wrong or hostile. With agent isolation it runs as
`horizon-agent`. Without it, agents run as `root`, the SSH user, and no
boundary exists on the worker.

- The upload accepts only the host key recorded for that worker
  (`StrictHostKeyChecking=yes`). A drop never records a new host key.
- The SSH user owns `/workspace/.horizon-drops`, the session folder and the
  batch folder. The worker commands refuse a folder that is a symbolic link or
  that another user owns, so an agent cannot select where the SSH user writes.
- The worker writes only new files and never follows a name out of the batch
  folder. A file arrives in the private folder `.BATCH.TOKEN.part` and moves into
  the batch folder only after its size is the local size.
- The cleanup removes a batch folder only when the private folder of the same
  drop exists, which the worker makes after the batch folder. It follows no
  symbolic link and leaves folders that another user owns.
- The files and folders belong to the group of `horizon-agent` with modes
  `0640` and `0750`. The agent can read them, not change them.

## Binding

The paths go only to the panel of the same session on the same worker, which
Horizon finds again when the upload ends. A drop never pastes a local path into
a cloud panel, also when the cloud is not connected or the upload fails.

## Deliberate limits: do not report

- The files are not private between the sessions of one worker. All agents of
  a worker run as one user.
- The files stay on the workspace volume until the cloud is deleted.
- A drop that contains a folder is refused. Folders are not uploaded.
- Horizon removes the folders of a cancelled or failed drop after it shows the
  result, over a new connection. When that connection fails, or the cleanup runs
  before a cut-off preparation made its folder, an empty batch folder or partial
  files in the private folder stay. Horizon never pasted their paths.
- An agent without agent isolation is `root` on its worker and can read or
  change anything there, the dropped files included.

## Report

- A drop that pastes a local path into a cloud panel, or pastes worker paths
  before every file arrived.
- A worker command that writes outside the batch folder and its private
  folder, follows a symbolic link, replaces a file or puts a partial file in the
  batch folder.
- A cleanup that can remove a folder or file that another drop or an agent made.
- A connection to the worker that can accept an unknown or a changed host key.
- File I/O, a process start or a wait for a process on the UI thread.
- Paths that go to a panel of another session, another cloud or another worker.
