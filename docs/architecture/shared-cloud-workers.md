# ADR-805: Explicitly shared CPU workers

Status: Proposed implementation contract; shared workers are not yet supported.
Date: 2026-09-24
Deciders: Repository maintainer through reviewed implementation PRs.
Tracking: [#805](https://github.com/peters/horizon/issues/805).

## Context

Cloud Workspaces currently gives each cloud one provider allocation. The version-1
`Deployment` in `crates/horizon-core/src/cloud_runtime/state.rs` contains both provider
and project state. Provider operation IDs equal cloud IDs. The worker scripts use
one `/workspace/repository.git`, `/workspace/home`, capability manifest, browser
host and desktop. Changing only allocation lookup would overwrite sibling source,
credentials and tools. A per-cloud local file lock cannot serialize two projects
or controllers on different machines.

The first shared mode supports one RunPod CPU worker and mutually trusted projects
of the same user. Dedicated placement remains the default. Sharing has a common
resource and failure domain; directory and process separation is not a security
boundary. There is no scheduler, autoscaling, GPU sharing or arbitrary dependency
merging. The existing MVP qualification in #813 remains a separate gate.

## Decision

### Separate allocation, project and presentation identity

| Record | Identity and contents | Authority |
| --- | --- | --- |
| Allocation | Stable allocation ID, display name, original provider operation ID, local account binding, immutable image digest, verified runtime capabilities, observed resources, storage ownership, SSH trust, lifecycle intent | Local durable allocation journal for provider operations; worker manifest for membership |
| Project deployment | Stable project ID, immutable cloud/workspace/session membership, allocation reference, committed repository revision, selected profile, namespace, agent sessions and tool grants | Local project journal and corresponding worker member record |
| Presentation | Cloud, panel IDs, layout and transport attachment | Existing workspace/session persistence; never compute ownership |

New project IDs are globally unique opaque IDs generated once before the first
intent write. Repository paths, titles and profile names are not identities.
Cloud and workspace IDs alone are insufficient where saved sessions can duplicate
them: the persisted owning session identity participates in the project binding.
Copying presentation state cannot mint a second project or transfer ownership.
Allocation IDs never change when members join or leave. The first project's cloud
ID is not the lifetime owner of a shared allocation.

Keep `WorkerSpec`, `CreateState`, provider inspection, storage and power operations
in `horizon-cloud`. Keep membership, controller coordination, presentation and
project session policy in `horizon-core` and the existing `horizon-cloud-worker`
binary. Provider verification uses the original allocation specification, never
a joining project's profile. Do not revive the removed remote-development model.

### Versioned configuration and placement binding

Repository `.horizon/cloud.yml` stays at version 1 with its current supported
fields. It describes runtime requirements and contains no allocation ID, account
binding, endpoint, credential or shared-worker alias. No new YAML field is needed
for initial sharing. YAML without an explicit local placement choice continues
to create a dedicated worker.

Introduce a version-1 machine-local placement envelope, separate from portable
profiles and account secrets. Its tagged `placement` is either `new_worker` (the
central default when omitted) or `existing_worker` with an allocation ID.
`new_worker` carries a `sharing` mode: `dedicated` is the central default, while
`trusted_shared` is an explicit user choice for a new compatible CPU allocation.
Copy the resolved sharing mode into the durable allocation record before provider
I/O. For the shared runtime protocol, initialize its manifest with that same mode
after worker identity/readiness verification and before first-project bootstrap. Only `trusted_shared` can
admit a second member; an existing-worker choice never upgrades a dedicated
allocation. Legacy migration records `dedicated` explicitly. Image capability
alone is not sharing consent, and no in-place sharing-mode conversion is supported
in this first delivery. The existing-worker choice also binds the existing local account reference and SSH identity; changing
machine defaults cannot redirect a saved allocation to another account. Persist
the resolved choice with the project before side effects. Unknown versions,
unknown placement variants and dangling references fail closed. A label change
cannot retarget a saved placement. Do not silently fall back to creating compute.

The envelope is a proposed internal contract, not a currently accepted settings
file or CLI option. Introduce its parser, UI choice and public protocol together
with their documented version/support checks in the delivery slices below.

### Compatibility before attachment

An existing worker must be explicitly enabled for trusted sharing, RunPod CPU,
currently reachable, identity-verified, running and free of destructive intent.
Its immutable image must advertise the new shared-project contract (version 1),
in addition to existing source and selected tool contracts. A legacy image is
ineligible even if its installed executables appear compatible.

The initial existing-worker path requires an image-only profile (`build` absent)
with an immutable digest equal to the allocation's digest. Mutable tags and
repository build recipes receive an incompatibility explanation and the ordinary
new-worker option. Attachment never builds, pushes, installs packages or restarts
services to repair a mismatch. A repository can declare a separate image-only
profile for a prebuilt common image. New workers still use existing build/push
resolution and validate the shared contract before allocation when sharing is
explicitly enabled.

Persist a separate immutable runtime protocol at image validation: `shared_v1`
when the image verifies the shared contract, or `legacy_dedicated` when a dedicated
request uses an otherwise supported older image. `trusted_shared` requires
`shared_v1`; there is no fallback on failed verification. New `legacy_dedicated`
allocations preserve the existing dedicated image contract and local-journal
lifecycle. They never initialize a membership manifest or accept another member.
The protocol is selected before provider I/O and cannot be inferred afresh from a
later failed readiness probe. Thus existing YAML/images remain usable without
silently gaining sharing or losing their lifecycle operations.

Requested agent/browser sets must be subsets of verified installed capabilities;
project-enabled tools are exactly the requested subset, not the worker union.
A desktop request also requires verified installed desktop capability; exclusivity
alone is insufficient. Check any requested remote-browser runtime contract and
the project's explicit account grant independently of installed local browsers.
Validate runtime/source contract versions, CPU-only operation, platform and
storage compatibility. CPU/memory requirements must fit the observed allocation;
report them as requirements, not reserved resources or enforced per-project
quotas. Display contention and total capacity honestly. No sum-of-declarations
heuristic promises that concurrent workloads fit. Storage declarations must fit
the actual mounted capacity, with observed free space reported separately.

Desktop ownership is initially exclusive: at most one member may reserve the
worker's desktop, including an attaching or removing member whose cleanup is
uncertain. A second request is rejected before source or credential transfer.
A project without desktop capability cannot discover the desktop tool. Separate
concurrent desktops are a future contract version, not an implicit shared target.

### Durable membership and concurrency

For shared-capable allocations, keep a worker-authoritative version-1 membership
manifest on the persistent workspace volume, outside project directories. All
worker membership and service/port reservations execute under one OS allocation
lock. Write a temporary file, synchronize it, atomically replace the manifest and
synchronize its parent before any corresponding side effect. A monotonically
increasing revision enables compare-and-swap requests. Corrupt or unsupported
state blocks ordinary mutations; neither missing files nor timeouts reset membership.

Initial bootstrap is a separate fenced operation, available only to the owning
controller for a newly verified allocation. Persist a unique bootstrap token and
`prepared` intent locally, then consume that permission as `requested` before
sending the first initialization command. Under the worker allocation lock, the
first command verifies the recorded creation and mounted-storage provenance and
requires no existing membership manifest or project state before writing anything.
An existing bootstrap record goes through same-token recovery; conflicting tokens
or allocation identities are rejected. A newly verified worker mounting retained
storage is not a fresh membership store, and an absent marker alone grants no
initialization permission. Synchronize a separate bootstrap record containing that
token, allocation identity and `initializing`
phase before writing the empty manifest. Admission is disabled in this phase.
After synchronizing the manifest and its directory, atomically persist
`initialized` in the bootstrap record; only that phase permits project admission.
Persist the matching initialized receipt locally after verification.

Recovery with a matching `initializing` record may finish the same empty manifest,
because admission has never been enabled; a nonempty/conflicting manifest blocks
recovery. An `initialized` record requires an intact matching manifest, even if the
controller lost the success reply. Missing/corrupt membership after initialization
never permits a new empty manifest. A missing bootstrap record after a requested
operation is likewise uncertain, not fresh-worker proof: preserve the fence and
require recovery of authoritative state. Repeated initialization returns the
recorded outcome for the same token without resetting a manifest. No admission or
source/credential transfer may bypass these checks.

Each request has a persisted operation ID, immutable request fingerprint and
expected manifest revision. Retrying the same operation returns its recorded
outcome or reconciles its side effects. Reusing an ID with different inputs is
rejected. Each allocation has exactly one persisted provider-controller identity and local
journal. Only that controller may invoke provider lifecycle operations; other
machines can request worker-side project operations but cannot stop, resume or
delete compute. The owner holds its OS allocation lock across provider intent,
I/O and verified completion. Uncertainty leaves a durable pending operation that
blocks any successor action after a crash. There is no automatic controller
transfer or recovery from a copied journal; loss of the owning journal blocks
provider mutations pending a separately designed ownership-recovery procedure.
This is a cooperative same-user contract, not protection against arbitrary use
of the provider account outside Horizon. The worker manifest serializes membership
across machines. Never hold two project
locks while requesting an allocation lock: acquire allocation then project lock,
and communicate through bounded worker commands without lock-order inversions.

Attachment states are `attaching`, `active`, `removing` and `removed`. Persist
`attaching` and its namespace/port/desktop/grant reservations before source or
credential transfer. Only then create that project's source, worktrees and tools.
An interrupted attachment retains its reservation and operation ID; inspection
must reconcile that same project before retrying. Mark active only after actual
source, tool and session readiness. Cancellation records removal intent and
cleans only resources owned by that operation. Uncertain cleanup retains the
member and reservations. Tombstones prevent a late request recreating a removed
member or recycling its namespace; an explicit new project uses a new identity.

For `shared_v1` allocations, provider stop/restart/delete requires a fresh
authoritative member snapshot and
a separate worker action. First atomically persist a worker-wide transition fence
on the worker containing the operation, full member set, manifest revision and
all action consequences: affected processes/sessions and unsaved in-memory work,
service/route downtime, tool-allocation cleanup, ephemeral disk loss, retained or
deleted persistent data and ongoing storage charges; this blocks attach/detach and new session/grant mutations.
Refuse to prepare the fence while attachment/removal or session/grant side effects
are in flight or uncertain. Each such operation must hold a durable reservation
until reconciled, so a lock released during I/O cannot hide unfinished work.
Return that exact snapshot for confirmation in all interfaces. Confirmation must
match the fence and action. Persist provider intent locally before provider I/O.
Reconcile a lost reply against the same allocation and fence, never repeat a
create or clear the fence because a client disconnected. Cancel an unexecuted
confirmation only through an explicit reconciled transition.

While a worker is unreachable, cached membership is not sufficient to authorize
a new destructive action. A previously persisted, confirmed fence permits
reconciliation of that exact action. After a confirmed stop, a durable matching
fence remains sufficient for the explicitly confirmed continuation; resuming
clears it on the worker only after provider and runtime identity verification.
Only the provider owner can perform that continuation, after settling the earlier
operation; a second controller cannot resume and clear a fence while an older
controller could still submit a delayed stop. A process crash or transport timeout
alone never proves that a provider request is no longer in flight.
A missing worker yields lost status for every member, not replacement compute.
No force-delete shortcut based on an empty local list is part of this delivery.

### Project runtime namespace and routes

New shared projects use `/workspace/projects/<project-id>/` for repository source,
LFS/submodule material, per-agent worktrees, session records, runtime files, logs,
credential files and tool sockets. Each agent additionally has its own home and
configuration below that project. Branches, tmux server/socket/session identities,
browser contexts and controlling-agent IDs include both project and session IDs.
Worker scripts accept validated identities; they never select a project from the
SSH caller's current directory or an ambient global HOME. Preserve committed-only
source transfer, pinned initial revisions and the existing no-replay launch fence.

Reserve service/application ports atomically in the allocation manifest before
launch. Record project, service, internal port and exposed route; never guess from
framework defaults. An enabled project service receives its assigned port in its
launch environment and the same route is used by panels and agents. Reserved
ports must exclude SSH, worker tooling and other occupied ports; bind conflicts
fail honestly without taking another process's port. Retain uncertain reservations
until the owning service is confirmed stopped. Externally started processes are
not made collision-free merely by recording a number in a manifest. Reach project
services through verified SSH forwarding and project browser routes. If direct
provider exposure is selected, reserve a bounded port pool in the initial
allocation specification; exhausting that pool rejects the route instead of
recreating the worker or assuming live provider port-map mutation.

Run project browser services with separate roots, contexts and grants. Each tool
request resolves the actual controlling agent against its project membership and
capability subset. No ambient browser or desktop binding from the first member
may leak into later agents. Worker-side supervision, enabled tools and application
processes continue with no client attached. Closing a view only drops transport.

### Credentials and removal

Transfer credentials only after compatibility validation and durable membership
reservation, using the existing explicit repository-bound grants. Agent homes,
Git helpers, package-feed configuration and browser-account grants are project
scoped. Tool discovery is filtered before startup. A worker image containing a
tool does not authorize it for a project. Trusted sibling processes can still read
or interfere with shared-container data; do not advertise adversarial isolation.

Track external tool allocation and credential-binding ownership by project and
session. Removing a member revokes its runtime copies and owned external sessions,
not the shared account credential or a sibling's allocation. Shared external
bindings require a durable owner set; ambiguous release cannot drop another
owner or mark cleanup complete. Credential rotation/removal reconciles only the
explicitly selected bindings.

Stopping project sessions affects only recorded project processes, without
provider stop. Removing a cloud first performs explicit project cleanup; it never
deletes the worker or sibling data. Separate retained project data from an
explicit delete-data action. Once no members remain, show an idle allocation with
its retained data/storage and ongoing costs. Deleting that allocation is still a
separate worker-wide action with exact provider/storage ownership checks.

### Version-1 deployment migration

Introduce version-2 local deployment records and version-1 allocation journals.
To discover or create migration intent, first hold only the old cloud lock. Persist
the intent, including its allocation/project IDs generated once, then release the
old lock. Acquire the allocation lock followed by the old cloud/project lock,
matching the global order, and revalidate the intent and source journals before
publishing anything. Never request an allocation lock while holding a project
lock. Competing recovery uses the same persisted IDs; changed or conflicting
intent requires a fresh read instead of retaining an earlier lock target.

Preserve the original provider operation ID, `CreateState` (including uncertainty), `WorkerSpec`, worker ID, volume journal
and required-storage fence, SSH trust files, profile, source readiness, timing and
all session/branch/worktree identities. Never reconstruct provider facts from UI
state. A preallocation record remains preallocation; migration never requests a
worker. Missing/corrupt companion journals block migration rather than erase fences.

Before publishing any allocation, atomically replace the old deployment with a
rejecting version-2 `migrating` envelope that includes the legacy payload and the
migration identity. Synchronize it and its parent directory while holding the
old lock. Older clients now fail on version 2 instead of modifying stale v1 state.
A crash before this marker still leaves v1 writable: recovery must reread and
compare the deployment and companion files with the recorded intent under the
old lock. If they changed, refresh and synchronize the intent from the current
valid state before installing the marker. No allocation is published from a stale
pre-marker snapshot. Corruption or conflicting identities block recovery.

After the marker, publish and synchronize the allocation, publish the version-2
project reference, then mark the migration complete. Check exact identities and
content at each recovery boundary. Version-2 readers reject ordinary operations
until both journals and migration completion agree. Retain an immutable legacy
backup for diagnosis, never as an alternate writable authority. This ordering
prevents both old-reader mutation after publication and provider I/O from partial
new records. Never merge two legacy records merely because their provider IDs
match; flag conflicting ownership and retain both fences.

Existing deployed projects retain a `legacy_dedicated` namespace descriptor with
their exact old paths, tmux identity and credentials. Do not relocate live files,
reconfigure tools, restart sessions or modify the remote worker during local
migration. Their allocation has exactly one member and stays ineligible for
sharing. Existing dedicated behavior remains usable through the migrated model.
Legacy dedicated stop/resume/delete uses its durable provider and
storage journals under the owning controller's allocation lock. This compatibility
path has exactly one fixed member and no attachment API, so it requires no remote
membership manifest or bootstrap marker. Its worker-action confirmation still
names that sole member and the full process/storage consequences. Preserve all
uncertain-create, pending-stop and storage-cleanup fences. Project-only actions
cannot call the provider lifecycle implicitly. Never select this path because a
shared worker's manifest is missing: eligibility is the immutable
`legacy_dedicated` protocol recorded by migration or preallocation image validation,
not runtime probe failure. New `shared_v1` allocations use the worker manifest
protocol even when their sharing mode is dedicated.

Where old records lack a durable credential/account reference, retain that absence
rather than fabricate historical ownership from current defaults. Treat configured
settings only as a candidate binding and require read-only verification against
the saved worker/specification before enabling provider mutations, then durably pin
the verified binding in the allocation journal before mutation. Unresolved or
conflicting identity remains fenced; later defaults cannot replace a pinned binding. Preserve the original SSH trust files and
never replace their pins merely to make a candidate connection succeed. A legacy
preallocation request still needs explicit local bindings before its first I/O.

A share-capable worker must be created explicitly with the new image/contract;
there is no in-place image upgrade or silent sharing of existing allocations.

### UI, CLI and MCP

Extend the established New Cloud flow with `New worker` as the default and `Use
existing worker`. The new-worker path offers an explicit `Allow trusted projects
to share this worker` choice, off by default, mapped to the persisted sharing mode.
Show names, observed CPU/memory, capabilities, members, known
prices with observation time, and precise incompatibility reasons before compute
or credential changes. Keep ordinary panels, immutable membership, per-cloud
layouts and fullscreen. A compact shared indicator and Cloud-menu member list
are sufficient; do not add another canvas management surface.

Expose allocation list/inspect, attachment, project stop/remove, reconnect and
worker transition prepare/confirm/reconcile through one typed core service used
by UI, public CLI and MCP. Protocol requests carry stable IDs, expected revisions
and idempotency keys, not raw provider credentials or SSH commands. Status returns
structured incompatibility, pending operation, lost-worker and resource failure
reasons. The current `cloud_deploy` example and browser-only MCP are not equivalent
public coverage; sharing must not ship with UI-only attachment. Apply existing
calling-workspace/agent ownership checks before resolving project references.

## Alternatives and consequences

Reusing a cloud's provider ID is smaller but retains global credentials, tool
roots and destructive actions. Reject it. A local-only membership registry cannot
establish that another client has no active members. Reject it. A hosted scheduler
could serialize everything but adds an account service and exceeds this issue.
Choose durable worker membership plus local provider intent, accepting that new
destructive actions are unavailable while authoritative membership is unreachable.

Strict common-image matching and one desktop owner limit initial convenience but
make compatibility and input ownership testable. Preserve dedicated placement for
incompatible projects. Sharing may cost more under contention; measure it before
making recommendations. Historical runtime-storage ADRs do not supersede this
Cloud Workspaces contract.

## Serial delivery and acceptance

1. Land this design contract without adding supported settings or runtime behavior.
2. Add typed allocation/project identity and a restart-safe local migration, with
   legacy fixtures, corruption tests and crashes at every publication boundary.
   Include an old controller updating v1 after a pre-marker crash and verify that
   recovery captures its new stop/delete/storage/session state.
3. Add worker-authoritative membership, fenced actions and project namespaces in
   focused slices; test conflicting controllers, lost replies, cancellation,
   port/desktop reservation and sibling preservation before host integration.
   Crash before/after each bootstrap write, lose the initialization reply and
   remove an initialized manifest: only proven pre-admission bootstrap can finish
   an empty manifest; lost membership must never be replaced with an empty set.
   Include delayed provider-stop execution versus another controller's attempted
   resume, and in-flight attachment versus worker transition preparation.
4. Integrate the core coordinator and credential/tool ownership. Add equivalent
   public CLI/MCP operations, then the minimal UI using that same policy. Split
   mechanical extraction prerequisites and stay within normal PR scope limits.
5. Retain synthetic repositories and a self-contained regression smoke plan.
   Qualify three workspaces/projects with multiple agents on one actual CPU
   allocation; prove concurrent servers and correct routes, all capability cases,
   reconnect without replay, worker loss, resource exhaustion, migration and all
   destructive-action consequences. Preserve sibling PIDs, dirty files and grants.
6. Run the full local/hosted matrix and independent reviews on each final head.
   UI qualification uses a task-owned desktop presented live through a native
   Device panel, with decoded motion video, launch/resize/Fit/fullscreen and
   restart-persistence evidence. Only then report the feature complete.
7. Benchmark the same synthetic workload on shared and dedicated allocations:
   image size, observed CPU/memory, provider price/region/time, storage charges,
   build/test duration, cold provisioning, first project, additional attachment,
   application readiness/first frame, reconnect and recreation separately. Start
   small only where offered; no 2-vCPU/4-GB capacity or savings promise. Delete and
   verify only task-owned compute and temporary credentials.

Issue #805 remains open through these slices. A design review or a schema-only
merge is not evidence that sharing, remote execution or cost savings work.
