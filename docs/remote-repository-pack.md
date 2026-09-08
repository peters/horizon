# Preparing an exact-base Git pack

The Linux `repository_overlay::seed::export::prepare_git_base_pack` API prepares
one standard, non-thin Git pack from an existing `PreparedGitSeed`. This is an
explicit local derived artifact, not a network transfer, worker setup command,
checkpoint, published store record or new export-approval boundary. No current
UI or worker command invokes this API automatically.

The caller must authorize the complete base closure, including files removed by
the overlay and the original raw commit metadata. Existing namespace resolution
and seed preparation remain required; the exporter does not accept an arbitrary
original repository path or walk another unchecked source tree.

## Isolation and contents

The exporter reuses the packed source's private metadata view, native child and
object-store validation. The nominated scratch parent must be private, outside
the entire seed tree, and exclusively controlled with stable ancestry. Symlinks,
recursive alternates, unsafe topology and source/scratch overlap are rejected.
These checks do not defend against concurrent same-user or privileged mutation.

The new view supplies only the exact base object ID and its synthesized shallow
boundary to trusted `/usr/bin/git`, through `/usr/bin/prlimit`. Original source
and private seed configuration, HEAD, refs, hooks, filters, replacement objects,
ambient Git configuration and network transports are not inherited. Parent
history, unreferenced objects and staged-only overlay blobs are not selected.
The original commit is preserved, not rewritten to remove its parent metadata.

Packing uses low-cost compression, one thread and no delta search/reuse. This
trades potentially larger output for simpler resource behavior. It does not use
thin packs. Standard packs contain their delta dependencies; `--shallow` alone
is not an exact-one-commit export boundary. See the [Git pack documentation](https://git-scm.com/docs/git-pack-objects).

## Receipt and failure

Successful preparation returns `PreparedGitPack`: the private file path, original
base commit, encoded byte length and SHA-256 digest. The file is newly created as
`0600` inside retained `0700` scratch. The exporter checks version-2 framing,
nonzero bounded object count, minimum length, encoded-byte ceiling, successful
trusted-producer exit and file flush. Hashing and copying use bounded chunks.

This is **not** an independent pack decode, immutable publication, file/directory
synchronization or crash-durability acknowledgement. The caller must keep the
artifact and ancestry stable until a later consumer revalidates its digest.
That consumer must also validate the expected complete closure and exact shallow
boundary. Strict Git decoding without that boundary rejects the deliberately
absent parent history; [Git index-pack](https://git-scm.com/docs/git-index-pack)
alone does not establish source approval or setup ownership.

Failures before reservation report no residue. Later failures retain the named
scratch directory and any partial `base.pack` as unconfirmed. No rollback,
overwrite, automatic retry, cleanup, task start or provider action occurs. Error
and debug output omit source contents and private paths. Dropping either a
success receipt or failure keeps the artifact; the exact owned Git child is
terminated/reaped when necessary, never an existing worker or task.

## Limits

`PackExportLimits` defaults to a 256 MiB encoded-output ceiling and the existing
native source defaults: 1 GiB child address space, 60 CPU seconds, and a 30-second
pipe deadline. For export, that deadline covers the complete pack pipe operation,
not each chunk. Valid seeds may exceed these limits and are rejected, not truncated
into successful artifacts. The maximum configurable output is derived from the
existing seed import ceiling; it is not permission to lift source-selection limits.

Blocking filesystem calls, process creation and kill/reap latency are outside the
pipe deadline. These are not hard application RSS or total wall-clock guarantees.
Run off the render thread. Caller admission/capacity budgets and later receiver
decompression limits remain separate; compressed byte limits do not bound decoded
memory. Revisit compression and concurrency only with representative measurements.

## Validation boundary

Native tests use the real resolver and seed importer, then strict Git receipt.
They verify exact base contents, removed files, exclusion of parent history and
staged-only/unrelated objects, ignored seed metadata, unchanged input snapshots,
private file identity, a 65 MiB-plus raw base blob, bounded output, cancellation,
write failure and exact owned-child cleanup. Pure tests exercise framing and
limits; they are not independent cryptographic pack-validation tests.

Worker receipt/publication, pinned transport, source-approval UI, repository setup,
remote checkpoints and cloud/PC-off acceptance remain separate work. Closing local
views must never implicitly stop or delete a persistent environment.
