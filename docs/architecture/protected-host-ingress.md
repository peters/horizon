# Opt-in protected Hetzner host ingress

`bootstrap.exposure_ingress` defaults to false and is omitted when false. Only
new Hetzner workers may request it. The ordinary host configuration remains
byte-identical: same startup, SSH22, metadata guard, registry handling, volume
mount, logging and service unit. Rebuild preserves the original bit and refuses
a change with a new-cloud instruction. No existing worker is recreated.

This component produces host readback; it adds no exposure owner, Gate listener,
public tool or endpoint. The SDK worker snapshot still reports SSH22 only.
Normal workspace readiness does not establish exposure readiness.

## Original host boundary

Cloud provisioning validates the complete host plan before a provider create.
Only a fresh server receives its user data. Requested/bound reconciliation does
not resend it. The existing service masks host SSH, locks root, checks the
workspace mount and denies link-local metadata access before startup.

The opt-in START calls a fixed isolated Python interpreter and root-owned script.
Before any Docker create/start, the script requires Linux root, pidfd support,
trusted system Python/Docker/nsenter/cat executables and the existing root-owned
image/environment files. It does not install tools or read provider credentials.
Registry authentication stays inside the original host Docker config. Narrow
command capture never logs child stderr, full inspect output or environment.

The root-only generation directory is under `/run/horizon-worker-ingress`.
Its fresh UUID is also an original Docker label. The container mounts that exact
directory read-only at `/run/horizon-host-ingress`; neither Docker socket nor
host `/proc` is mounted into it. The existing writable workspace is unrelated to
receipt authority and remains agent-owned across restarts.

The exact newly created CID publishes SSH22 unchanged and host TCP20000–20031
to container TCP600–631. Backend632–663 is never published. An explicit
`net.ipv4.ip_unprivileged_port_start=1024` and `NET_BIND_SERVICE` retain root-only
binding. Container bootstrap keeps its existing capabilities; dropping all of
them would break UID/GID changes and workspace ownership. Existing agents retain
the `setpriv` no-new-privileges and empty bounding-set boundary.

## Actual readback and immutable receipt

One shared three-second deadline covers running-state observation, original
process/namespace binding, actual sysctl reads and repeat checks. Only exact CID,
requested image, actual image ID, PID, running state, publications, sysctl config,
mounts and labels are selected from Docker. The actual publication set must be
SSH22 plus all 32 fixed ingress mappings for every represented wildcard family;
extra/duplicate/partial-family, backend, non-TCP and non-wildcard publications
are refused. Image EXPOSE declarations with no binding are not publication.

The host holds original pidfd and netns FD, reads root UID/effective bind
capability, init start time and actual namespace device/inode. `nsenter` reads
the effective boundary through the held namespace FD. Exact Docker/process
bindings and the sysctl are read again; original pidfd must remain live.
This is repeated observational evidence, not an atomic kernel snapshot.

Receipt v1 contains allocation operation, generation UUID, full CID, requested
immutable image, actual image configuration ID, init start ticks, netns
device/inode, effective privilege boundary and the complete observed publication
set. The canonical passive Protocol contract was merged in #1679;
SDK cannot import Protocol because Protocol already depends on SDK. This script
constructs observations and does not implement another generic receipt decoder.
The cross-contract test runs this exact encoder with synthetic comparison facts,
then checks its bytes with the canonical parser. It runs no host preflight.

The file is fsynced in a private temporary name, atomically linked without
replacement to `receipt.json`, the temporary link is removed, and its directory
is fsynced. The resulting file is root-owned0400 with one link. A generation
cannot be republished or refreshed. A later worker must validate actual readonly
mount/metadata, current allocation, PID1, namespace, sysctl and root capability;
Core must retain the original pinned SSH/custody and reject any successor.
Saved receipt JSON or profile intent does not establish those facts.

## Lifetime and failure

`docker start --attach` preserves the original container's output and exit code.
The [Docker CLI implementation](https://github.com/docker/cli/blob/v28.5.1/cli/command/container/start.go#L136-L169)
waits for that original container's status; record and check the actual VM CLI
version in live qualification.
The ordinary worker lifetime is not limited to three seconds. Pulls retain ten
attempts and 15-second retry intervals, with each opt-in pull bounded to 120
seconds. Create is bounded to 30 seconds, narrow commands to three seconds and
forced original removal to 15 seconds. Every captured command has output bounds
and kills/reaps its own child on refusal or timeout.

On exit or signal, receipt removal is attempted before any blocking cleanup.
The exact original CID is force-removed and absence is checked with a successful
daemon query; the attach child is reaped. A lost create response permits only a
bounded query of the original fresh generation label, followed by original
allocation/image/label checks for cleanup. It never grants admission or adopts
a successor. If ownership, removal, join or absence is uncertain, startup exits
unsuccessfully and retains the private generation directory; it never claims
cleanup or publishes a success receipt. The existing unit may restart its owned
worker, but live consumers must refuse that changed generation.

## Qualification limits

Receipt publication does not prove peer-source preservation. Docker may proxy
host IPv6 to container IPv4; actual external source checks must govern supported
paths. This component is not qualified by render tests or a copied receipt.
The [procedure](../testing/procedures/protected-host-ingress.md) requires an
actual task-owned VM and exact UID10001 bind-denial checks before first review
and again on final inputs. No desktop startup execution is permitted. Actual
protected Gate/keeper and Core ingress consumption remain separate requirements.
