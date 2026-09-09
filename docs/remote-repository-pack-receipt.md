# Receiving a private exact-base pack

The Linux `repository_overlay::seed::receive::receive_git_base_pack` API receives
one explicitly identified pack into fresh private scratch. It returns an object
directory for the existing namespace, seed and checkout verification APIs. It does
not publish a worker input record, approve source export, create a ready checkout,
start a task or perform network/provider operations. No current command/UI invokes
it automatically. The matching producer is documented in [pack preparation](remote-repository-pack.md).

## Input and verification

Supply `ExpectedGitPack` (exact SHA-1 base commit, SHA-256 and encoded byte length),
an existing private scratch parent, a caller-owned reader, `PackReceiveLimits`, and
a cancellation callback. A prepared producer receipt converts to the expected
identity directly; identity is not an authorization decision.

```text
expected identity + bounded reader
  -> fresh private pack + exact length/EOF/SHA-256
  -> isolated strict Git indexing + exact shallow boundary
  -> bounded exact-base closure enumeration
  -> private object directory -> existing namespace/seed/checkout checks
```

Invalid metadata/limits and unsafe parents are rejected before reservation.
Framing, object count, length and SHA-256 are checked before native decoding. The
shared pack-copy leaf streams bounded chunks rather than buffering the whole pack.

Trusted resource-limited Git indexes the file with strict integrity checks and a
synthesized shallow boundary for the original base commit. The command does not
repair thin packs or use promisors. Indexing reads no original repository metadata,
ambient configuration or external object stores. The generated index is private,
and pack/index relocation refuses replacement of any existing destination.

A second isolated native process enumerates the expected shallow closure without
object names. Its generated revision request requires a commit object, and the
output must begin with the original requested ID and account for every packed
object; a blob, tree or peeled tag cannot stand in for the commit. Strict decoding
plus this bounded count rejects missing closure, unreachable objects and extra
parent history; strict indexing alone does not prove that selection. The raw
original commit is preserved, not rewritten.

This verifies Git pack/closure integrity, not namespace safety, source-export
approval, repository URL ownership or task admission. Existing namespace policy,
link handling, seed identity and checkout verification remain required. Do not
treat a received pack as a ready repository or as authority to retry setup.

## Ownership, limits and failure

The caller controls stable, exclusive scratch ancestry and supplies trusted
`/usr/bin/git` and `/usr/bin/prlimit`. No-follow parent checks are not confinement
against concurrent same-user or privileged mutation. Keep received files and their
ancestry stable until the existing verification pipeline consumes them.

Defaults are the producer's 256 MiB encoded ceiling and the native source defaults:
1 GiB address space, 60 CPU seconds and a 30-second pipe deadline **per child**.
Indexing and closure enumeration are separate children; these are not whole-call
CPU/wall-time or application RSS guarantees. Valid repositories may exceed limits.
Blocking reader/filesystem calls, process creation and kill/reap are outside the
pipe deadline. The reader owns transport latency and cancellation; run off the UI
thread. Cancellation is checked between chunks and native operations.

After reservation, any failure retains the entire named private root, including
partial pack/index/metadata, as unconfirmed. Success retains it too. Display/Debug
omit contents and private paths. Only owned native children are terminated/reaped
when necessary; existing workers and tasks are untouched. There is no automatic
retry, cleanup, file/directory synchronization or crash-durability acknowledgement.

## Evidence and remaining integration

Native tests exercise the real exporter, receiver and existing raw checkout path,
including staged overlay/deletion semantics, initial/empty/parented bases, repeated
nested objects and a 65 MiB-plus raw blob. Failure tests cover digest mismatch,
truncation/trailing input, checksum corruption, wrong base, extra objects/history,
unsafe parents, cancellation, short reads and redacted reader errors. Existing
shared-process/copy tests cover resource limits, stalled children and failed writes.

Worker immutable input publication/observation, pinned transport, source-approval
UI and full cloud/PC-off acceptance remain separate. Closing local views must never
implicitly stop or delete a persistent environment.
