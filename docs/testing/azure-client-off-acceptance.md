# Azure client-off acceptance: harness and runbook

Azure lane of the approved disposable-client test in
[#475](https://github.com/peters/horizon/issues/475), for
[#474](https://github.com/peters/horizon/issues/474). The harness lives in
`scripts/azure-client-off/` (`client_off.py` with deterministic tests under
`tests/`, and `provision-client.sh`). This document is the runbook; it records no
completed allocation and no passing result. Nothing in this lane touches the user's
PC, existing Horizon processes or existing workspaces.

## Topology

| Role | What it is | Owner of its paths |
| --- | --- | --- |
| Client A | Disposable Ubuntu VM in its own exact resource group, running the exact Horizon Linux client (built from `client_sha`) under a virtual display, with an isolated persistent client home on its OS disk (retained across deallocation) | Azure lane |
| Worker B | Separate persistent Azure CPU worker in its own exact group, created on A through the product path (setup, Prepare Repository, saved-Shell Start); its Stop, saved-Stop check and Start are product operations too | Azure lane (product Azure paths and adapter) |
| Observer C | This controller, outside A and B: read-only ARM reads and one pinned SSH session per sample with a key that sshd restricts (`restrict,command=`) to a forced reader of the progress and checkpoint files | Azure lane |

C never renews a lease, delivers a keepalive, reconnects a terminal, checkpoints or
replays a task. C may enforce the declared cleanup deadline.

## Manifest, frozen before anything is rented

```json
{
  "subscription_id": "<exact UUID>",
  "location": "northeurope",
  "run_id": "<32 hex digits drawn now: head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \\n'>",
  "client_group": "horizon-client-<run_id>",
  "client_vm_size": "Standard_B2s",
  "client_sha": "<40-hex commit the client binary was built from>",
  "client_binary_sha256": "<digest of that binary, from record-client-build.sh>",
  "worker_group": "horizon-ws-<workflow>-<job>",
  "worker_image": "<registry>/horizon-remote-worker@sha256:<digest>",
  "hourly_cost_micros": 41000,
  "budget_micros": 2000000,
  "cleanup_deadline_utc": "<ISO-8601 with an explicit offset, within 24 h and far enough ahead for the run>",
  "off_minutes": 12,
  "lease_seconds": 600
}
```

`client_off.py --manifest m.json validate` refuses to run anything until the
manifest is complete: exact UUID, a fresh `run_id` with A's group named
`horizon-client-<run_id>` and B's group the adapter's `horizon-ws-<workflow>-<job>`
(every name the harness may create or delete is unique to one run, because ARM
resource IDs are name-based and offer no conditional delete or power operation),
two different exact groups, a full commit SHA and
the binary digest, a digest image reference, positive price and budget (the example
values above are a `Standard_B2s` list price and a two-currency-unit budget; replace
them with the current price and your bound), a deadline within 24 hours carrying an
explicit UTC offset and, for the phases that rent or keep compute alive, far enough
ahead to outlast them (`validate`: the 30-minute provisioning bound, the observer
install at its bounds (fifteen bounded reads, the key derivation and one bracketed
probe before the 10-minute run-command bound, seven reads and a probe to reconcile
after it; about 47 minutes), the off-phase setup at its bounds (eleven bounded ARM reads
for the client, worker and identity-bracketed state attestations, the observer
probe, and the deallocation with its poll, which share one 10-minute bound; about
29 minutes), the off interval and everything that must still follow it: the return
phase's own setup and the cleanup window (about 42 minutes: 22 for the return at its
bounds and the 20-minute cleanup bound); `off` and `install-observer-key`: their share of the
same sum; `return`: its own setup at its bounds (seven bounded reads and the start
with its poll, about 22 minutes) plus the cleanup window it must leave intact), so the
reaper can never reach a group mid-run; the phases arm their runtime deadline from
the same numbers,
an off interval of at least ten minutes that exceeds the configured lease. `verdict` and `cleanup` also run after the deadline: a late cleanup is exactly
the case that must run. The
manifest, current price and deadline are posted on #474 before the paid run starts;
credentials and identifiers stay private.

## Client A prerequisites for the product path

The product on A authenticates to Azure the same way the controller does: the
subscription-pinned Azure CLI credential (`az account get-access-token --subscription
<id>`), never a stored bearer token. `provision-client.sh` installs no Azure CLI and
gives A no identity today, so before the product pass A needs, in this order:

1. The Azure CLI on A (added to the cloud-init package list; no extension install).
2. An identity A can log in with non-interactively. The intended shape is a
   system-assigned managed identity on A with `az login --identity`, granted a role
   that can create resource groups and deployments in the subscription and assign the
   worker pull identity (`horizon-worker-puller`) to the worker VM; the exact role
   assignment is an authorization change and is posted on #474 for approval before it
   is made. No user credential is copied to A.
3. A Horizon configuration on A whose `remote.azure` list holds the exact profile the
   run uses (subscription, `northeurope`, VM size, the pull identity, the registry
   login server, the declared hourly cost and the disk SKU); the product binds that
   profile immutably to the worker at creation.
4. For the Git lane, a user-supplied repository-scoped PAT for the authorized
   disposable repository. It is typed only into the repository preparation's token
   field on A (stdin-only delivery to the worker) and is never written to A's disk or
   to any manifest, journal or receipt.

## Procedure

1. **Prepare locally** (Linux controller with GNU `timeout`). In a fully clean
   checkout at `client_sha` run `record-client-build.sh --out client-build.json`: it
   refuses modified or untracked files, builds the `horizon` binary for
   `x86_64-unknown-linux-gnu` from that tree, refuses anything that is not an ELF
   x86-64 binary, and records HEAD together with the digest of the binary it just
   produced, so the digest can only belong to that commit; confirm the worker image digest is pullable by the pull identity; generate
   a fresh Ed25519 key pair for A (`key`, `key.pub`); record the pre-existing resource
   groups in the manifest subscription as JSON
   (`az group list --subscription <id> --query '[].name' -o json > groups.json`) and
   the pre-existing resources
   (`az resource list --subscription <id> --query '[].id' -o json > resources.json`)
   for the cleanup comparison.
2. **Provision A**: `provision-client.sh --manifest m.json --ssh-private-key key
   --horizon-binary <binary from the record> --build-record client-build.json
   --ssh-source-cidr <controller address>/32 --out client.json`. Diagnostics go to
   stderr and `client.json` is the exact-A descriptor the later phases take. A's Ed25519 host key is read through
   the control plane (run command inside the exact VM) before the first connection,
   and every SSH and SCP call uses the fresh client key explicitly with
   `IdentitiesOnly`, `BatchMode`, strict checking against that pin and a bounded
   controller-side timeout. It refuses a binary whose commit or digest differs from
   the record and the manifest, a `.pub` that does not belong to the private key, and
   a VM size the location does not offer to the subscription without restrictions
   (checked with `az vm list-skus` before anything is created). The group and VM
   creates are treated as ambiguous once sent: each is reconciled by reading the exact
   resource back with this run's tags for as long as the bound allows, and the group
   is journaled the moment it is confirmed, with every journal and descriptor write
   checked, every local file (descriptor, its temporary, the journal and its
   temporary, the pinned host-key file) reserved with an exclusive create before the
   first cloud call, the group record required to carry the expected name and ARM ID,
   and the public address polled under the bound while the network converges. After
   the host key is read, the VM is attested again (instance identity and tags) before
   the key is pinned. Everything it writes
   locally is private to the caller (`umask 077`), and `--out` may not alias the
   creation journal. It creates the
   run-named group (journaling it in `created-groups.json` the moment it exists) and
   the VM with the reaper tags, the manifest deadline and the run identity, admits
   SSH only from the given range, waits for cloud-init to
   finish with the display and window-manager services active and the client
   directory present, copies the binary, verifies its digest on A, and runs a launch
   gate: the client must open a Horizon window on the virtual display (runtime
   libraries and a software Vulkan driver are installed by cloud-init). The whole
   provisioning and readiness path, from the first cloud call to the launch gate,
   runs under one absolute 30-minute bound. The descriptor records A's group ID, VM
   ID, instance identity (`vmId`) and `run_id`, each read back from ARM and checked
   for shape: ARM IDs are name-based paths that a same-name recreation keeps, so the
   off and return phases attest the instance identity and the run identity as well
   as the IDs and the full tag set immediately before they touch anything and on
   every poll afterwards, and every observer sample records which A instance was
   off. Azure offers no identity-conditioned power or run-command operation; the
   residual window between that read and the call reaching ARM is covered by the
   names being unique to this run, not by a compare-and-swap.
3. **Baseline on A** (product path): start Horizon on A's display with
   `HOME=/home/horizon/.horizon-client-home` (the descriptor's `client_home`; Horizon
   keeps its state under `$HOME/.horizon`, reported as `client_state_root`), the same
   `HOME` the launch gate used, and drive it through the overview (input on A's
   virtual display through `xdotool`, screenshots through `import`, exactly as the
   slice smokes did):
   - **Environments** → **New remote workspace**: under *Worker and repository* pick
     the Azure profile (the exact `remote.azure` name), enter the worker image digest
     reference, repository, branch, working directory, command and disk size, and
     optionally an *Azure CPU cost limit*; **Review request** shows the complete
     profile, the declared price and the immutable-binding disclosure; tick the
     consent box and press **Create task-free worker**. Nothing is checked out and no
     task starts here. Record the workspace, owning session, workflow and job
     identities and B's exact group ID (`horizon-ws-<workflow>-<job>`).
   - **Check this setup** until the saved phase is Ready with the attested pin (the
     host key is read through ARM's run-command channel, never trusted on first
     connection); the pin is the `host_key` the observer descriptor carries.
   - **Review repository preparation** → optionally *Include explicit first-token
     installation* with the PAT typed into the token field → confirm; **Check
     preparation receipt** proves the checkout without a second submission.
   - **Show saved panels** → start the saved Shell task (the deterministic counter
     script that writes an increasing counter to a progress file under
     `/workspace`); **Show session panels** → **Reconnect** to attach the view. Three
     independent panels on B are part of this lane's acceptance; the control path for
     adding the second and third panel intents to a remote workspace is confirmed on
     #474 before the run and recorded here.
   Record worker, session and task identities, the starting counter and dirty file
   hashes. The task must advance its counter at least once per 15-second sample.
   Write `worker.json` for the observer: `vm_name`, `port`, `host_key` (the attested
   key), `observer_key_path` (the private half of a fresh Ed25519 key generated for
   observer C only, never the client's worker key), `progress_path` and optionally
   `checkpoint_path` (plain, distinct paths under `/workspace`, no `.` or `..`
   components, so no two spellings can name one file), and the identity
   recorded now, before A is stopped: `group_id`, `vm_id`, `instance_id` (the VM's
   `vmId`, which a same-name recreation does not keep) and `host` as ARM reports them.
   Then install the observer key as a restricted key: `client_off.py --manifest m.json
   install-observer-key --worker worker.json --public-key observer.pub`. The worker image authorises `HORIZON_SSH_PUBLIC_KEY` as an
   unrestricted root key, so a plain key would not be a read-only channel; the
   harness instead appends one `authorized_keys` line of the form
   `restrict,command="<reader>" ssh-ed25519 ...` inside the worker container through
   the ARM run-command channel, after the same identity, image-tag and running gates
   the off phase applies. The install reserves the whole off phase after itself, hands
   every probe what is left of its bound, appends only while the run-command bound
   and its reconciliation still fit, and edits `authorized_keys` through an editor
   that opens `/root/.ssh` and the file relative to a verified directory descriptor
   with `O_NOFOLLOW`, so no planted symlink can redirect a root write. `restrict`
   disables pty, port, agent and X11 forwarding and
   user rc; the forced reader (its source and arguments travel base64-encoded, so no
   quoting layer can alter them) opens the two paths component by component with
   `O_NOFOLLOW` below `/workspace`, returns a capped prefix of regular files only and
   prints one JSON object naming the paths it was installed for, whatever command
   the client asked for; the off phase and every sample require those echoed paths to
   be exactly the descriptor's, so a stale or foreign observer line can never supply
   another task's counter. The off phase
   requires this channel to answer before A is touched. The install step probes first
   and appends nothing unless B explicitly refuses the key (an answer means it is
   already installed; anything else leaves the state unknown and appends nothing);
   afterwards it requires the forced reader to answer a session that requested `id`,
   and refuses the key otherwise. `observer-key-line` prints the same line for
   inspection or for an image that installs it itself. The line lives in the running
   container only: the worker entrypoint rewrites `authorized_keys` on every container
   start. A first-class observer account in the worker image is lead-owned and would
   replace this step.
   If this run created B's group, journal its exact identity: `client_off.py
   --manifest m.json journal-group --group <B's group> --created created-groups.json`
   (it reads the ARM ID and full tag set into the journal).
4. **Off**: `client_off.py --manifest m.json --journal journal.ndjson off --worker
   worker.json --client client.json` (the provisioning output; the journal path is the
   one the verdict reads in step 6). Binds the whole phase to one absolute
   deadline (the manifest deadline minus the return phase and the cleanup window it
   must leave, so A is deallocated only when it can still be brought back): every ARM
   call, probe, poll and the sampling itself is cut off there (an observation gets
   what is left, never more than its own 30-second bound, and none starts with less
   than 10 s left), so the reaper can never act during a live phase. Validates both descriptors, attests that A
   is the exact VM provisioned for this run (group ID, VM ID and the full tag set,
   which includes the client binary digest, read from ARM now) and currently
   running, confirms B is the baseline worker carrying the manifest image's tag and
   running, proves the observer key is the restricted one (the forced reader must
   answer; a key whose session runs the requested command fails the gate and nothing
   is stopped; every session requests a pty and a remote port forward, and only the
   refusal of both, the `restrict` option's doing on a worker whose sshd already
   disables agent and X11 forwarding, counts as a restricted channel: a `command=`
   line without `restrict` answers but is reported as unrestricted, and every later
   sample from such a channel is unreadable), requires the declared interval plus its
   setup to fit before the phase deadline (re-checked with the remaining budget right
   before the deallocation, whose call and poll share one 10-minute bound), creates
   the journal exclusively with that baseline as its header only once every check that
   could still refuse has passed, so a refusal leaves no header behind and the same
   path can be retried (a dry run keeps its plan in memory and never touches that path)
   (flushed and synced before the irreversible call, as is every sample),
   re-attests A in the same read as its running state, then deallocates A only,
   requires `PowerState/deallocated` from the re-attested A on every poll, then
   samples every 15 s for `off_minutes`: A's power state together with the evidence
   of its attestation (group and VM IDs, instance identity, the tag maps ARM showed
   on VM and group, kept verbatim so the offline verdict re-derives it; the identity
   is read before and after the state and the state counts only when both agree), B's
   group and VM identity, instance identity, image-reference tag,
   power state and public address as ARM reports them, and the counter and
   checkpoint sequence through one restricted, pinned session to that address only
   (any answer other than the forced reader's JSON object is an unreadable sample).
   B's identity is read again after its state and reading (ARM IDs compared
   case-insensitively, instance identity, image tag and endpoint exactly); if it
   changed in between, the sample keeps the identity it named but no state or reading. Each
   sample's instant is taken at its start, so acquisition latency never makes it late. The journal is one JSON line per sample and its
   path must be unused.
5. **Return**: `client_off.py --manifest m.json return --client client.json`. Bound
   the same way to the manifest deadline minus the cleanup window, and the start is
   issued only while its 10-minute start-and-poll bound still fits. Attests
   the same exact A again, requires it to be deallocated, starts it only and requires
   `PowerState/running`. On A, start Horizon again with the same home, open
   **Environments**, select the same saved environment (same workspace, owning
   session, generation and exact resource ID; **Check provider status** is a read
   only), and **Show session panels** → **Reconnect** to the same B and task
   sessions: same worker identity, no additional create, no task replay, dirty bytes
   intact, no credential rotation.
6. **Verdict**: `client_off.py --manifest m.json verdict --journal-in journal.ndjson`.
   The journal's first line is the baseline header the off phase wrote (worker
   identity and image); a journal without it, or for another image, never passes.
   Pass requires the declared interval crossed, the lease exceeded, the 15-second
   cadence met (every sample carries its scheduled and actual instant; a sample more
   than 5 s late, a skipped slot, or consecutive observations more than 15 s apart
   beyond one second of wake-up latency is a finding), A deallocated and attested as
   the provisioned resource in every sample, the first sample matching the baseline identity,
   B's identity, endpoint and running state unchanged, B carrying the manifest
   image's reference tag throughout, and a counter that advances between every pair
   of consecutive samples. Worker-owned checkpoint progress is reported as a
   separate boolean and is never inferred from the counter.
7. **Worker lifecycle** is a different assertion and runs only after the offline and
   reconnect evidence is captured; never Stop or start B during the off interval. It
   uses the product controls on A, in the *Explicit Stop* section of the overview:
   **Stop environment…** → confirm (one Stop; records intent, deallocates B, verifies
   `PowerState/deallocated` with the retained `worker-data` disk); if the Stop ends
   unverified, **Check saved Stop** (read-only; confirms only deallocated compute with
   the retained disk and the saved address); then **Start environment…** → confirm
   (records Start intent, starts only the exact worker, accepts only the saved
   identity and pin, resolves to Reconciling); then **Show session panels** →
   **Reconnect** and read the counter file and the dirty files back: same worker,
   same pin, retained bytes, no task resumed. The adapter proved the same sequence
   live in runs 16 to 18 (`azure-workspace-live-acceptance.md`); this step proves it
   through the product.
8. **Remove the observer key**: `client_off.py --manifest m.json remove-observer-key
   --worker worker.json --public-key observer.pub`, once the return and the reconnect
   check on A are done and before the run is reported finished. B may outlive the
   run (a product worker is left in place by cleanup), and the run's observer private
   key must not keep a reading channel into it. The phase attests B, removes exactly
   the observer's `authorized_keys` line inside the worker container through the ARM
   run-command channel (matched whole and literally; a read error leaves the file
   untouched), re-attests B, and passes only once B explicitly refuses the observer
   key under the pinned host key (an answer means it is still authorized; a transport
   failure or a reader answering with nothing proves nothing and the removal stays
   unproven); like cleanup it also runs after the manifest deadline. A worker restart drops the line as well (the entrypoint rewrites
   the file), but the run does not rely on that.
9. **Cleanup**: `client_off.py --manifest m.json cleanup --groups-before groups.json
   --resources-before resources.json --created created-groups.json`. Runs under one
   20-minute bound (every ARM call is handed what is left of it; the manifest margin
   is the return phase plus this bound). Deletes only the manifest groups that this run
   journaled as created and that did not exist before the run (anything else is
   refused and reported; the journal holds each group's ARM ID and full tag set from
   creation), re-reads each group immediately before its delete and refuses one whose
   ID or tag set differs from the journaled identity (absent, recreated or retagged).
   Because ARM group IDs are name-based paths that a same-name recreation keeps, a
   journaled record authorizes a delete only when its tags bind it to this run: A's
   group must be `horizon-client-<run_id>` carrying the manifest's `run_id`, and B's group must carry the
   adapter's `horizon-workflow-id` and `horizon-job-id` from which its own name is
   derived; anything else is refused before it is even read. It then waits for
   proven absence (an unreadable answer is unknown, never absence) and proves the
   peers untouched: every resource outside the deleted groups must be exactly the set
   recorded in step 1 (a vanished peer resource, or a group recreated under its old
   name, is a finding), and the remaining group names must match the list from step 1.

## Labelling

- A run whose B was created through the adapter's live driver instead of the
  product path is an **adapter-only rehearsal**. It exercises A, C, the off interval
  and the cleanup, and it is reported as such; it is never the #475 product pass.
- The product paths this pass needs are merged as of 2026-09-13: configured setup
  and consent (#561), Prepare Repository (#567), Check saved Stop (#574), the first
  explicit Stop (#575), and durable explicit Start with its configured Azure admission
  and overview control (#576, #578, #584). What still gates the first paid run is
  listed under *Client A prerequisites*: the Azure CLI and an approved identity on A,
  the PAT for the disposable repository, and the confirmed control path for the second
  and third panel; the compact worker image is lead-owned and the full image proved
  the adapter lane meanwhile.
- Counter progress proves the task kept running. Checkpoint proof needs
  worker-owned checkpoints advancing during the interval.

## Status

No allocation has been made under this runbook yet. The harness and its
deterministic tests are in place and the product Azure lifecycle is merged; the
manifest, current prices, budget and deadline are posted on #474 before the first
paid run, after the client prerequisites above are approved and in place.
