# ADR-383: Retain the worker SSH host identity on workspace storage

Status: Proposed implementation; full remote restart integration pending.
Date: 2026-09-06.
Decider: Repository maintainer through issue #383 and reviewed delivery.

## Context

The persistent development contract requires verified reconnect after explicit
stop or recovery, as well as after client disconnection. The previous image
generated host keys only under `/etc/ssh`. A provider can retain `/workspace`
while replacing its container filesystem. A new server key then invalidates the
client's pinned identity. RunPod explicitly distinguishes container disk from
retained volume storage in its [storage contract](https://docs.runpod.io/pods/storage/types).

## Decision

Retain the host key under the private `/workspace/.horizon-worker/ssh` directory.
Record a no-overwrite access-key digest claim before generation, then synchronize
the key pair before publishing a versioned readiness marker. Startup revalidates
ownership, permissions, parent paths, marker and actual key-pair correspondence.
Only a complete matching identity can be materialized into the existing runtime
paths before SSH starts. A bounded wait admits concurrent initialization; a lost
or damaged claim/key/marker never grants a replacement inside the retained store.

The independent client's SSH pin remains mandatory. Retaining a key does not
authorize adopting a volume, worker or runtime under a different provider identity.
The provider/coordinator must still verify those identities. A missing entire
volume cannot be distinguished from a fresh volume by this helper; recovery must
not silently accept a new server key in that case.

For a validated provider-bootstrap request, first initialization may restrict an
empty, root-owned `/workspace` mount from exactly `0777` to `0700`. This is
fixed-root preparation, not permission repair or volume adoption. It requires a
safe parent and a real mount boundary, holds a no-follow directory descriptor,
and rechecks path/inode/owner/mount identity and effective mode around chmod,
directory synchronization and a second emptiness check before creating identity
state. Already trusted mounted storage is unchanged; nonempty unsafe storage is
refused. Every provider-bootstrap request also validates the real mount and
root/path identity for already protected storage; an image-local directory is
not a retained volume. Non-provider startup behavior is unchanged.
The image starts in `/` and leaves `/workspace` empty. Only after retained
identity validation does the entrypoint create and enter `/workspace/horizon`.
Using that repository directory as the image working directory would let the
container runtime populate a fresh mount before the emptiness check.
A chmod that silently does nothing is refused. If chmod succeeds but a concurrent
entry or identity change is detected, startup fails without deleting that entry
or attempting a rollback/repair.

The provider must establish fresh-volume ownership and exclusive attachment
before requesting this operation; valid bootstrap environment fields and an empty
directory do not prove either. The helper cannot exclude another authorized
mount writer or make chmod and emptiness checks crash-atomic. A crash after chmod
can leave a restricted mount without an identity claim; existing provider identity
and client pin checks remain necessary on recovery, including whole-volume loss.

## Alternatives and trade-offs

- Container-only keys are simple but fail verified reconnect when the runtime
  filesystem is replaced. Automatically accepting a changed key breaks the trust
  contract and is rejected.
- External secret-manager storage would separate identity from workspace storage,
  but adds a credential/service dependency. It remains a possible later backend.
- A retained volume keeps initialization local and inspectable. It depends on
  POSIX ownership/durability semantics and protected backups. It does not provide
  encrypted key storage or isolation between same-root worker tasks.

## Consequences and validation

Keep provider inspection at `/etc/ssh/ssh_host_ed25519_key.pub`; configure only
the verified Ed25519 host key. Refuse unsafe or incompatible state instead of
overwriting keys. Recovery from interrupted first initialization, key rotation
and legacy-key migration require explicit later workflows.

Real-key tests cover restart, concurrent creation, mismatched access, damaged
state and path permissions. A local SSH smoke destroys only its task-owned
container filesystem while keeping its named volume and verifies the same host
identity and repository data. This does not prove live cloud restart, persistence
of agent/session state, volume-loss recovery or PC-off development.

A disposable September 2026 High-Performance network-volume probe observed NFSv4:
root chmod changed `0777` to `0700`, synthetic directories/files retained
`0700`/`0600`, and an unprivileged child was denied listing, reading, creating and
renaming within the protected root. Synthetic file/directory fsync, hard-link
publication and rename/readback succeeded. This is one measured candidate, not a
provider-wide guarantee or a durability certification. Separate sampled Pod and
Standard network volumes exposed FUSE with ineffective permission changes and
must remain rejected. The NFSv4 observation does not satisfy or change the
separate ext4-only repository importer contract. Actual worker bootstrap,
Stop/restart persistence, provider-volume ownership and unchanged client pin
still require exact-source integration and live verification.

Remaining actions: integrate provider-retained volumes and remote backups;
preserve task/agent state and session markers; implement explicit Stop/recovery;
complete exact-head cloud acceptance without touching unrelated resources.
