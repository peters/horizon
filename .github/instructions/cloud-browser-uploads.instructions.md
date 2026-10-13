---
applyTo: "crates/horizon-browser-protocol/src/cloud_view.rs,crates/horizon-browser-protocol/src/cloud_view/**,crates/horizon-cloud-worker/src/browser.rs,crates/horizon-cloud-worker/src/browser/chooser.rs,crates/horizon-cloud-worker/src/browser/chooser/**,crates/horizon-cloud-worker/src/browser/staging.rs,crates/horizon-cloud-worker/src/browser/staging/**"
---

# Trust boundary of cloud browser uploads

A person gives local files to a page in a cloud browser, through the page's
upload button or a drop on the panel. Horizon sends the bytes to the control
service of the worker, and the worker browser attaches the staged files. The
design is in the module comments and in the section "Browser uploads" of
`examples/cloud-worker/README.md`.

## Off by default

The control service relays uploads only when `HORIZON_CLOUD_BROWSER_UPLOADS`
is `1`. Without it, the service must act as one that predates uploads: it does
not register a file chooser consumer, refuses each `stage` request and sends no
`uploads` state. It still removes the staged files of an earlier service when it
starts.

## Trusted: the host

The host is the person's Horizon. It reaches the control service only through
the cloud's authenticated SSH connection. The control service checks each
upload request to keep the files in their folder, not to limit the host.

The control service cannot tell a host from a process on the worker: its
loopback endpoint also serves the browser tools of the agents, without a
credential. A process on the worker can therefore send the polls, parts and
answers of a host. It can keep a chooser waiting for at most 20 minutes, or
answer it with files that it staged. This gives an agent no new power: it
controls the same browsers through the same endpoint, and `set_files` attaches
its own files.

## Untrusted: the page and the agents of the worker

A page or an agent can open a file chooser at any time, and the page writes the
`accept` and `origin` text of the request.

- A chooser waits only while a host that sends uploads polls, and for at most
  20 minutes. Without such a host the control service cancels it.
- The control service gives a browser only complete files that it staged for
  that browser. The size and the SHA-256 of each file agree with the host.
- An answer applies only to the request that it names. An answer to a replaced
  or closed request never reaches a newer request.
- A batch ID and a file name are one path component each. A staged file is a
  new file in its batch folder. Below the browser history folder, the service
  creates the folders and the file through descriptors that never follow a
  symbolic link, and writes each part to the file that it created.
- Counts, sizes, parts and the time that unused files stay are bounded.

## Untrusted toward the host: the worker

The limits, errors and notices come from the control service, and `accept` and
`origin` come from the page. A host must show them only as plain text, shorten
long text, and use a worker limit only to lower its own limits. A host sends
only the files that the person picked or dropped, never reads a path from the
worker and never sends a host path to the worker.

## Rules of the wire format

- A batch ID is 1 to 64 ASCII letters, digits, `-` or `_`. A file name is one
  path component of at most 255 bytes, without `/`, `\` or control characters.
  Both sides check them with `valid_upload_batch` and `valid_upload_name`.
- An answer names the request that it answers, so a worker can refuse an answer
  to a replaced or closed request.
- A drop names the `page` generation of the upload state that the person saw,
  so a worker can refuse a drop that arrives after a navigation. The worker
  counts each change of the top-level URL as a navigation, also a change of the
  fragment or of the history state, so it can refuse a drop on the same
  document. A scroll or a layout change during the transfer is not a
  navigation: the drop then goes to the same point of the changed page.
- A host that predates uploads sends no `uploads`, and a worker that predates
  them sends no `uploads` state and no `staged` receipt. New versions only add
  fields.
- A host sends a `stage` request only for a browser whose state has `uploads`.
  A worker that predates uploads cannot read it and closes the connection.

## Deliberate limits: do not report

- The control service, the browsers and the agents use one account. An agent can
  read or change staged files, as it can change all browser state. Without agent
  isolation that account is `root`.
- The removal of a batch uses its path. An agent that plants a link can make
  the service remove files that the agent itself can remove.
- A used batch stays until its browser closes, because the page reads the files
  after the attachment. At a limit the oldest used batch goes first.
- The `commands` of a poll still go to the browser as before. This includes a
  `drop_files` command with worker paths from the host.
- A remote device of a provider gets no uploads.
- A navigation that the browser did not report yet when a drop arrives. The
  control service reads all reported browser events before it checks a drop.

## Report

- A chooser that can wait without a host that sends uploads, or without a limit.
- A file that a browser gets before its size and SHA-256 agree, or a file of
  another browser or batch.
- A write outside the batch folder of the request, or a write that replaces a
  file.
- An answer that reaches a request other than the one that it names, or a drop
  that reaches a page after a navigation.
- A file name or batch ID that the checks accept but that can leave its folder.
- A change of the wire format that an older host or worker cannot read.
- Text from the page or the worker that the format presents as trusted.
- An answer or a drop that the format cannot bind to the request or page that
  the person saw.
- Upload behavior while `HORIZON_CLOUD_BROWSER_UPLOADS` is not `1`.
