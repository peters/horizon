---
applyTo: "crates/horizon-browser-protocol/src/cloud_view.rs,crates/horizon-browser-protocol/src/cloud_view/**"
---

# Trust boundary of cloud browser uploads

A person gives local files to a page in a cloud browser. Horizon (the host)
sends the bytes to the control service of the worker, and the worker browser
attaches the staged files. The wire format is in
`crates/horizon-browser-protocol/src/cloud_view/uploads.rs`. This version adds
only the wire format: the control service ignores the `uploads` of a poll,
refuses each `stage` request and sends no `uploads` state.

## Untrusted toward the host: the page and the worker

- A page or an agent can open a file chooser at any time. The page writes the
  `accept` and `origin` text of a request.
- The control service writes the limits, the errors and the notices. A changed
  worker can send any text or number in them.

A host must show this text only as plain text, shorten long text, and use a
worker limit only to lower its own limits. A host sends only the files that the
person picked or dropped. It never reads a path from the worker and never sends
a host path to the worker.

## Rules of the wire format

- A batch ID is 1 to 64 ASCII letters, digits, `-` or `_`. A file name is one
  path component of at most 255 bytes, without `/`, `\` or control characters.
  Both sides check them with `valid_upload_batch` and `valid_upload_name`.
- An answer names the request that it answers, so a worker can refuse an answer
  to a replaced or closed request.
- A host that predates uploads sends no `uploads`, and a worker that predates
  them sends no `uploads` state and no `staged` receipt. New versions only add
  fields.

## Report

- A file name or batch ID that the checks accept but that can leave its folder.
- A change of the wire format that an older host or worker cannot read.
- Text from the page or the worker that the format presents as trusted.
