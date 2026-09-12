# Worker-owned versioned byte capture

This opt-in Linux worker operation protects an explicitly selected set of
Git paths on an explicitly attested retained volume. It is **not an atomic or
application-consistent snapshot, an independent backup, or a full repository/task
checkpoint**. It does not advance `RepositoryCheckpoint` or authorize Delete.

`horizon-byte-capture start` reads a bounded JSON enrollment from stdin:

```json
{
  "version": 1,
  "preparation": { "version": 1, "workspace_local_id": "example", "runtime_id": "00000000-0000-4000-8000-000000000001", "source": { "repository": "example/repository", "commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "branch": "work/example" }, "work_branch": "work/example" },
  "selected": ["src/example.rs", "notes.txt"],
  "retained_volume_attested": true
}
```

Use the exact existing Git preparation identity, not the illustrative values.
The explicit attestation means the operator has selected retained storage; the
service does not independently prove provider retention, replication, mount
replacement or durability. The destination and checkout must be on the same
filesystem. Selection is explicit; existing capture policy refuses excluded
credential/configuration/cache paths, unsupported nodes and ambiguous index
state. This policy is not a content-based secret detector: only enroll paths
whose contents are authorized for this same-volume private capture.

The launcher severs controller I/O and starts one worker-owned process. There is
no client heartbeat, mandatory product lifetime or provider lifecycle action.
Neither image entrypoint nor workspace setup automatically enrolls a capture.
`status <binding>` reads only; `cancel <binding>` records cancellation without
replaying capture, deleting data or stopping tasks. A repeated `start` observes
an existing enrollment regardless of selected-path order and does not restart a cancelled, failed or uncertain
service. A consumed enrollment with no status is `claimed_unknown`, not success.
The pure `capture-binding` command derives enrollment identity without checkout
access. Existing enrollment status is read before checkout admission; a new
enrollment still requires `capture-plan` validation before its exclusive claim.

## Publication and limits

- The private destination is
  `/workspace/.horizon-worker/byte-captures/<binding>`, outside the checkout.
  Checkout identity is checked against its completed preparation receipt.
- Each attempt uses the existing literal index/worktree capture and immutable
  digest-named bundle store. File synchronization, publication, complete bundle
  readback and encoded-byte hash verification precede success recording.
  Publication explicitly selects the named layout: `bundles/<digest>/record.hzov`.
  Anonymous-layout files are not imported or treated as free capacity. Incomplete
  digest claims remain permanently retained and cause visible refusal; no reclaim
  or publication replay is automatic. Selected-filesystem validation is still
  required; local named publication is not HPS qualification or durability proof.
- Bundles and first-success receipts for each distinct content version remain
  immutable. Current status is atomically replaced and synchronized. Publication
  or response uncertainty can leave an unacknowledged bundle; it remains counted
  against capacity. No garbage collection or overwrite recovery is automatic.
- The target cadence is 10 seconds. Status records capture start/end, current and
  preceding verified-success times; the measured interval, not the timer setting,
  determines whether the 30-second target was met. After 30 seconds without a
  verified success, status is stale. A stored `running` state is not live process
  proof; stale running observations are presented as stale.
- Capture-detected concurrent edits are retryable: status becomes `degraded`,
  preserving the prior verified success, and the next interval samples again.
  A verified success clears degradation. Identity, unsupported-content, storage
  and capacity errors remain terminal; no automatic enrollment restart occurs.
- Maximum selection is 128 paths; existing limits are 64 MiB per file and
  128 MiB distinct payload per bundle. Complete encoded records are accounted
  separately, up to a 1 GiB per-enrollment record budget and 4,096 records.
  Metadata consumes additional bounded space. Capacity exhaustion is visible;
  it never deletes prior versions. New enrollments require new explicit action.
  An already verified identical bundle needs no additional quota and is
  resynchronized/read back even at the byte or record-count ceiling.
- Each capture child has a 15-second watchdog, 15 CPU-second limit, 2 GiB virtual
  memory limit and 160 MiB per-file write limit. Cancellation targets only that
  newly created child. These bounds do not promise hard process teardown or
  bounded filesystem latency for an uninterruptible storage operation.

Enrollment **version 1** keeps the initial exact commit as its capture base. A
later commit/HEAD change fails visibly and retains previous captures.

Opt in to **version 2** in a new enrollment to keep selected bytes protected
across ordinary commits on the exact enrolled `work_branch`. Each version-2
bundle contains the complete literal index and working-tree state of **every
selected path**, including unchanged committed bytes and explicit absent nodes.
Its manifest binds the observed current commit and work branch. This is not an
export of other tracked files, Git history or all objects reachable from HEAD.
LFS pointers and hydrated selected bytes remain literal; no filter is invoked.
A new enrollment validates this bounded read-only capture before claiming a
slot. During capture, detected branch/HEAD/selected-index movement, including
newly unsupported index states after valid admission, is retryable `changed`.
Ordinary edits to unselected index entries are outside this detection guarantee.
A wrong, detached or unborn branch or unsupported index at admission is a terminal refusal. The next
attempt can observe a later commit on the enrolled branch without re-enrollment.
Version 2 has a distinct canonical binding; existing version-1 enrollments,
receipts and retry/observation behavior are not migrated or reinterpreted.

Automatic re-enrollment, Git history protection, submodule recursion, agent/task resumption, write freezing,
verified-loss replacement and destructive-Delete gates are separate outcomes.
Versioned literal captures can mix file times. Even retained storage is not
protection against deleting that storage or every provider failure mode.

## Local proof

Run `python3 -B -W error containers/remote-worker/test_byte_capture.py` for pure
guards. After exact-source review and candidate build, explicitly add
`--binary /absolute/path/to/horizon-repository` for a network-isolated bwrap
namespace. A private temporary directory is mounted at `/workspace`; the host
path is not modified. The fixture constructs synthetic initial Git receipts
(it is not a Git-setup/provider acceptance test), then uses actual worker CLI
start/status/cancel and capture code. It proves controller process exit, two
versions within the observed target interval, staged/unstaged/untracked/removal
bytes, readback into fresh scratch paths, unchanged prior versions, and visible
failure after HEAD drift for version 1. The version-2 lane then proves refusal
before claiming on a wrong branch, continued capture across a normal commit,
retained committed selected bytes followed by new dirty bytes, two observed
intervals within 30 seconds, earlier readback and no relaunch on repeated Start.
No credentials, provider resources or image upload
are involved. Exact-head runtime results are still required; preparing this
recipe does not claim they passed.
