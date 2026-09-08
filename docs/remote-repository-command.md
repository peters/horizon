# Explicit worker repository materialization

`cargo build -p horizon-repository` builds a headless, one-shot helper. The
[remote worker image](../containers/remote-worker/README.md#explicit-repository-helper)
installs it from a separate pinned-toolchain build stage. It is not included in
Horizon release assets, and existing running workers are not automatically upgraded.
No default invocation creates anything. The original synchronous command is:

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
Use the retained setup commands below when one-shot admission and subsequent
read-only observation are required. Independent supervision, recovery and client
integration are separate; the original `materialize` command remains unchanged.

## Retained setup commands

```sh
horizon-repository setup < setup-request.json
horizon-repository setup-status < setup-request.json
```

Both accept one strict versioned JSON object followed by EOF, bounded to 128 KiB
including whitespace. The larger input bound accommodates three supported paths
with worst-case JSON escaping. Unknown, duplicate, missing and malformed fields
are rejected. Provide the same complete immutable intent on every observation.

```json
{
  "version": 1,
  "retained_root": "/worker/retained-workspace",
  "workspace_local_id": "workspace_1",
  "objects_directory": "/worker/source.git/objects",
  "bundle_store": "/worker/bundles",
  "bundle_manifest": "<64 hexadecimal SHA-256 characters>",
  "destination": "repository"
}
```

The retained root must already exist, be privately owned, and satisfy the core
storage/confinement requirements. Neither command creates or repairs it. Its
ancestry, mount configuration and authorized source must remain trusted and stable.
The root and claim must outlive client connections and runtime generations; a retry
ID or new request does not select another slot. Remote ownership must be established
separately. Setup materializes under the fixed `setup-data` child; the example
checkout is `/worker/retained-workspace/setup-data/repository`.

Only `setup` can admit and consume a fresh in-process grant. An existing matching
claim observes the result instead, even when no result exists. A conflicting intent
fails without modifying the existing claim. `setup-status` only opens/reads: it
never creates, synchronizes, adopts, repairs, replays or cleans any retained state.

Before fresh admission, a read-only identity scan rejects a retained write root
inside either input tree, including aliases; it does not rely on read-only mounts
or path spelling. Each input scan is confined without symlink/mount traversal and
bounded to 262,144 nodes, 16 MiB of aggregate relative paths and depth 64. Missing,
unsafe or excessive inputs fail closed before a claim is written. Existing-claim
and status observation bypass source validation so saved results remain observable
when the original inputs are unavailable. These checks still require stable trusted
topology through execution, not concurrent same-user namespace changes.

Responses are complete JSON plus newline, bounded to 128 KiB. They contain
`version`, `status`, `recording`, `reason` and `execution`. A non-null `execution`
contains `state`, `reason`, `source_metadata`, `checkout`, `possible_destination`,
`base_commit` and `bundle_manifest`, with the historical receipt meanings described
above. Paths are intentionally returned only to the authorized caller; diagnostics
remain redacted. A requested destination alias in a failed receipt does not prove
that publication occurred.

| Command status | Exit | Meaning |
| --- | --- | --- |
| `absent` | 0 | Status observed no claim in the qualified root; not authorization to bypass admission. |
| `claimed_unknown` | 4 | Matching claim, no recorded result; not unstarted, running, successful or safe to replay. |
| `completed` | 0/1/2 | Receipt exists: published exits 0, rejected exits 2, other execution states exit 1. |
| `recording_unconfirmed` | 1 | Recording was not acknowledged; retain all state and inspect any known execution. |
| `error` | 1 | Storage, identity, claim or result could not be safely observed/admitted; never treat as absence. |
| `rejected` | 2 | Invalid command request, before admission. |

`recording: "acknowledged"` means this invocation freshly recorded and verified its
execution result on qualified healthy storage. `"observed"` means read-only access
to a historical record, not this reader's synchronization or current liveness.
`"not_acknowledged"` provides no recording guarantee. On `recording_unconfirmed`,
`execution: null` means preflight rejected before execution; otherwise the known
execution result is preserved even though recording was not acknowledged. In both
cases admission may already have consumed the one-shot claim.

Exit 3 retains the same output-failure meaning as `materialize`. Output loss never
rolls back setup, removes its claim, or authorizes replay. A fresh status request
can read a previously recorded result after the producer exits. Missing/corrupt
output remains unknown until safely observed. No response establishes current task
liveness, current checkout contents, cleanup permission, cloud or power-loss proof.

These commands remain synchronous: they do not detach themselves from SSH, launch
tasks, transfer source, create a supervisor or integrate the client. A worker-owned
independent launcher is available separately below for submission across client loss.
No invocation implicitly stops/deletes a workspace when a local view closes.

## Independent setup submission

The Linux worker image also installs a separate explicit launcher:

```sh
horizon-setup-launch < setup-request.json
```

It accepts the same complete immutable request as `setup`, bounded to 128 KiB
and EOF, with no command-line arguments. It first delegates validation and
qualified read-only observation to `horizon-repository setup-status`. A matching
existing claim returns that observation without spawning setup. Invalid input,
unsafe storage, mismatched intent or unavailable observation never means absent.

Only after a verified absent observation does it spawn the fixed `setup` command,
in a new process session, with a bounded private stdin pipe, no inherited client
stdio, closed extra descriptors and a minimal environment. The launcher writes no
request or log files. The detached child performs input separation and one-shot
admission itself. Racing submitters cannot reconstruct or share its fresh grant.
The worker's PID 1 must reap completed orphan children; the shipped SSH daemon
provides that role. No PID is returned as liveness or recovery authority.

The response is bounded JSON plus newline, at most 256 KiB to accommodate a full
128 KiB observation and envelope. Its fields are `version`, `state`, `observation`:

| State | Exit | Meaning |
| --- | --- | --- |
| `observed` | Original observation exit | Contains the unchanged parsed `setup-status` response; no child launched. |
| `submitted` | 0 | Complete request handoff and EOF; not admission, current execution or completion. |
| `handoff_unconfirmed` | 1 | Child spawned but complete handoff was not confirmed; retain state and observe. |
| `rejected` | 2 | Oversized input or read failure before observation/spawn. |
| `error` | 1 | Observation could not be safely consumed, or child spawn failed. |

Only `observed` has a non-null observation. Exit 3 means the launcher response
could not be completely written/flushed; it never kills or retries an already
spawned child. Closing the SSH request channel after handoff does not terminate
setup. Input, observation and handoff loss must remain unknown until separately
observed. The 30-second observation and 15-second pipe handoff timeouts are best
effort; input without EOF and uninterruptible OS I/O have no hard total deadline.

Reconnect by calling `horizon-repository setup-status` with the same intent.
Its retained result is the recovery surface; detached transient stdout/stderr is
not an additional durable log. If recording failed, later observation may remain
unknown/error and cannot reconstruct a transient unrecorded execution receipt.
Claims and data remain retained, with no automatic cleanup or replay. A successful
handoff does not strengthen storage guarantees or make failed recording successful.

This provides independent setup submission, not source transfer, remote task/log
supervision, client/provider integration, restart recovery or cloud/PC-off proof.
No command implicitly stops/deletes an environment when a local view closes.
