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

Remaining actions: integrate provider-retained volumes and remote backups;
preserve task/agent state and session markers; implement explicit Stop/recovery;
complete exact-head cloud acceptance without touching unrelated resources.
