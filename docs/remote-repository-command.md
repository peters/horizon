# Explicit worker repository materialization

`cargo build -p horizon-repository` builds a headless, one-shot helper. It is not
automatically installed in worker images or included in Horizon release assets.
No default invocation creates anything; the only command is:

```sh
horizon-repository materialize < request.json
```

Stdin must contain one JSON object followed by EOF, at most 16 KiB including
whitespace. Unknown, duplicate or missing fields, trailing objects and unsupported
versions are rejected. Paths must be absolute UTF-8 paths without parent traversal.

```json
{
  "version": 1,
  "objects_directory": "/worker/source.git/objects",
  "bundle_store": "/worker/bundles",
  "bundle_manifest": "<64 hexadecimal SHA-256 characters>",
  "scratch_parent": "/worker/materialization",
  "destination": "repository"
}
```

The digest must identify an existing verified `RepositoryBundleStore` record.
The caller must separately authorize the entire nominated base commit closure,
both overlay layers and publication. Source/ancestry must remain stable; the
existing private scratch parent must be exclusively controlled by the caller.
Existing packed-source resource limits and trusted system Git/prlimit requirements
apply. Publication requires qualified Linux journaled ext4 with barriers, stable
mount configuration and healthy storage. Other platforms reject without writes;
an unsupported Linux filesystem can retain an unpublished prepared checkout.
This is neither export authorization nor cloud durability or power-loss proof.

Stdout is one JSON response plus newline, bounded to 128 KiB including that newline.
The ceiling covers three retained paths at their worst-case JSON-escaped sizes.
Every response has
`version`, `status`, `reason`, `source_metadata`, `checkout`,
`possible_destination`, `base_commit`, and `bundle_manifest`; unavailable values
are null. Paths intentionally identify retained state for the authorized caller.
Ordinary diagnostics omit request contents and paths.

| Status | Exit | Meaning |
| --- | --- | --- |
| `rejected` | 2 | Invalid command input; no materialization was begun. |
| `unpublished` | 1 | No publication confirmed; retain all reported scratch/checkout paths. |
| `published` | 0 | No-replace rename and required synchronization completed; checkout names the destination. |
| `published_unsynchronized` | 1 | Rename completed, but later synchronization/checks did not; checkout names the destination. |
| `rename_unconfirmed` | 1 | Rename outcome is uncertain; inspect both checkout and possible_destination. |

Invalid command-line arguments print static usage on stderr and exit 2, without
reading stdin. Exit 3 means a complete response could not be written, not that the
operation was rolled back. Stderr diagnostics are best-effort; failure to write them
does not replace the command's exit status. Malformed, absent or lost output and abrupt termination
are unknown observed outcomes regardless of exit status. Stdin and storage I/O
have no hard end-to-end deadline. A successful stdout write is not a remote
acknowledgement, and no process destructor is guaranteed on abrupt termination.

Dropping any receipt never deletes data. Nothing overwrites existing destinations,
starts tasks, fetches remotely, schedules checkpoints, retries or cleans residues.
A later worker-retained operation layer must provide non-creating observation and
recovery before wiring this helper into client reconnect/setup flows.
