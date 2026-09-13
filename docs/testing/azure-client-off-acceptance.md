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
| Worker B | Separate persistent Azure CPU worker in its own exact group, created on A through the product setup path and prepared through Prepare Repository; its Stop, saved-Stop check and compute Start are product operations; the saved-Shell task start and panel reconnect are gated for Azure until their slices land | Azure lane (product Azure paths and adapter) |
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
  "worker_image": "<registry>/horizon-remote-worker-shell@sha256:<the complete Shell image digest named under Labelling>",
  "hourly_cost_micros": 41000,
  "budget_micros": 2000000,
  "cleanup_deadline_utc": "<ISO-8601 with an explicit offset, within 24 h and far enough ahead for the run>",
  "off_minutes": 12,
  "lease_seconds": 600
}
```

`worker_group` is the one field a product pass cannot freeze before renting: the
product draws B's workflow and job identities when setup is submitted on A (step 3),
after A has been provisioned from this manifest, and `validate` (which
`provision-client.sh` runs before renting A) refuses any `worker_group` that is not
the adapter's `horizon-ws-<workflow>-<job>` with two exact UUIDs. The harness has no
post-setup binding step yet, so **the product pass is gated on a harness change in
this lane** (a `bind-worker` command that writes the product-created group into the
manifest's `worker_group` only if the group carries the adapter's
`horizon-workflow-id` and `horizon-job-id` tags from which that name derives, plus a
`validate` rule that everything except `worker_group` is unchanged since
provisioning; claimed on #474 before it is written). Until it lands, a manifest
whose `worker_group` is edited by hand after provisioning is not a runnable product
path and is not used. The other fields are frozen before anything is rented.

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

1. The Azure CLI on A. `provision-client.sh` does not install it today: before the
   product pass the script's cloud-init must be augmented with the `azure-cli` package
   from Microsoft's repository (a harness change in this lane, no extension install);
   a client provisioned by the current script has no CLI and the product cannot
   authenticate on it.
2. An identity A can log in non-interactively. The intended shape is a
   system-assigned managed identity on A with a custom role listing exactly the
   actions the product paths under test send, and nothing destructive: resource
   groups (`Microsoft.Resources/subscriptions/resourceGroups/read` and `write`);
   deployments (`Microsoft.Resources/deployments/read` and `write`; the transport
   only submits deployments and reads the deployment resource back, never the
   operation-status endpoint); compute (`Microsoft.Compute/virtualMachines/read` and
   `instanceView/read`, because every VM read the transport issues expands
   `instanceView` and the lifecycle decisions come from its power statuses;
   `write`, `deallocate/action` for Stop, `start/action` for Start and
   `runCommand/action`, which setup and Start's readiness path use to attest the
   worker host key; `Microsoft.Compute/disks/read` and `write`); network
   (`read`, `write` and `join/action` on `Microsoft.Network/publicIPAddresses`,
   `networkInterfaces` and `networkSecurityGroups`, `read` and `write` on
   `virtualNetworks` and `virtualNetworks/subnets/join/action`); plus the built-in
   Managed Identity Operator scoped to `horizon-worker-puller` alone. Check saved
   Stop is ARM read-only and needs nothing beyond the reads. The role grants no
   `delete` action: deletion is not part of this pass and no product path on A
   deletes anything (the setup coordinator dispatches no compensating cleanup when a
   later step fails; it preserves the allocation for recovery and retry), so every
   group this run creates is removed by the operator's cleanup from C (step 9) under
   the operator's credentials, never A's. Neither subscription-wide Contributor nor
   any role with `delete` is assigned to A. The residual that Azure RBAC cannot
   remove is that the `write` actions must sit at subscription scope (the product
   creates one new resource group per worker, so no narrower scope exists before the
   run), which lets a compromised A modify resources of those types in unrelated
   groups for the run's duration; the run detects that through the peer comparison
   in step 9 but cannot prevent it, and the only prevention is a subscription holding
   nothing but this lane's resources. **This assignment is not made yet.** Per the
   #474 coordination, the proposal posted there for approval must carry, before any
   role or identity is created or assigned: the action list above with its
   justification from the product transport (file and line per action) and the
   official Azure RBAC operation reference, the exact scope of each assignment, the
   owner of the identity, and an exact expiry and removal plan (the assignment and
   A's identity are removed with A's group in step 9, and no later than the manifest
   deadline, by the operator from C; the custom role definition is deleted once the
   pass is reported). Cost approval is not approval for this authority. No user
   credential is copied to A. Before Horizon is launched, and under the same `HOME`
   Horizon will use, run `az login --identity` as A's Horizon user and prove the token
   path the product will take without printing a token: `az account get-access-token
   --subscription <id> --resource https://management.azure.com/ --query expires_on -o
   tsv` must print the epoch expiry, the same `expires_on` field the product's
   credential parses from the JSON answer. The credential issues the same subscription
   and resource arguments (adding only `--output json` and `--only-show-errors`) with
   stdin closed, so an unauthenticated CLI leaves setup, Stop and Start unable to
   obtain a token.
3. A Horizon configuration on A whose `remote.azure` list holds the exact profile the
   run uses (subscription, `northeurope`, VM size, the pull identity, the registry
   login server, the declared hourly cost and the disk SKU); the product binds that
   profile immutably to the worker at creation.
4. For the Git lane, a user-supplied repository-scoped PAT for the authorized
   disposable repository. It is typed only into the repository preparation's token
   field on A (stdin-only delivery to the worker) and is never written to A's disk or
   to any manifest, journal or receipt.

Two product paths are still refused for Azure and gate the baseline and return
steps below until they land (tracked as the next slices on #474): the saved-Shell
task start (`remote_worker_status/git_start/configured.rs` and the overview's saved
panel Start admit Local Docker and RunPod only) and configured panel attachment
(`remote_panel_attachment/configured.rs`, which every Reconnect uses). Steps marked
**gated** below cannot be executed for an Azure worker today; a product run stops
before the first gated step and collects no counter evidence until both land. A
third refused path, the provider status read (`remote_environment_observation/
configured.rs`, the overview's **Check provider status**), is not used by this
procedure and is tracked separately.

The deterministic task this lane runs on B, for the pinned worker image: saved Shell
panel with program `/bin/sh` and arguments (one JSON array, pasted verbatim into
*Literal arguments (JSON array)*, so the shell receives a single `-c` script)
`["-c", "i=0; while :; do i=$((i+1)); echo $i > /workspace/progress.counter.tmp &&
mv /workspace/progress.counter.tmp /workspace/progress.counter; sleep 5; done"]`,
working directory `.`, which the saved Git task resolves relative to the worker's
fixed checkout `/workspace/horizon/repository`, so `/workspace/progress.counter`
advances every five seconds and every 15-second sample sees a higher value; no
checkpoint path (worker-owned checkpoints are deferred to #471, so the verdict's
checkpoint boolean stays false). Dirty bytes: before the off phase, write one file
`/workspace/horizon/repository/horizon-dirty-marker.txt` containing a fresh random
token from a second panel session, record its SHA-256 in the baseline, and read it
back unchanged at return and after the worker lifecycle step.

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
     the Azure profile (the exact `remote.azure` name) and fill the fields:
     *Digest-pinned image* (the manifest's `worker_image`), *GitHub owner/repository*,
     *Exact commit SHA* (40 hex characters; the draft refuses an empty or short value),
     *Dedicated work branch*, *Repository directory* `.`, *Planned Shell program*
     `/bin/sh`, *Literal arguments (JSON array)* exactly the argv of the deterministic
     task below, *Panel directory (optional)* empty, the disk size, and optionally an
     *Azure CPU cost limit*; **Review request** shows the complete profile, the
     declared price and the immutable-binding disclosure; tick the consent box and
     press **Create task-free worker**. Nothing is checked out and no task starts
     here. Then **Refresh saved page** and select the new row: the repository, panel
     and Stop sections render only for a selected saved row, and the page shown after
     creation is still the previous one. Record the workspace, owning session,
     workflow and job identities and both forms of B's group identity: the
     resource-group name `horizon-ws-<workflow>-<job>` (the manifest's
     `worker_group`) and the full ARM group ID
     `/subscriptions/<id>/resourceGroups/horizon-ws-<workflow>-<job>` (the overview's
     *Exact resource ID* and `worker.json`'s `group_id`). Journal that group the
     moment the saved record shows it, whether or not the setup goes on to succeed:
     `client_off.py --manifest m.json journal-group --group <B's group> --created
     created-groups.json`. Nothing on A deletes a group after a failed setup (the
     coordinator preserves the allocation for retry), so a group journaled only after
     a successful baseline would survive an aborted run; journaling it here is what
     lets step 9 remove it in every outcome.
   - **Check this setup** until it reports the original setup as observed: the saved
     phase becomes `Reconciling` (setup recovery never writes `Ready`) and the record
     carries the attested pin, read through ARM's run-command channel and never
     trusted on first connection. Check reports in the setup notice and the overview
     then reloads the saved page on its own, keeping the selected workspace by ID;
     confirm the reloaded row before the next step. The
     overview deliberately never shows the pin; the `host_key` for `worker.json` is
     taken from A's saved record, whose snapshot is JSON. The store runs in WAL mode,
     so a plain file copy can miss the newest rows: take a consistent copy with
     SQLite's online backup through `python3`, which the client image has (cloud-init
     depends on it; the provisioner installs no `sqlite3` binary):
     `python3 -c 'import sqlite3; s = sqlite3.connect("/home/horizon/.horizon-client-home/.horizon/cloud-run/workflows.sqlite3"); d = sqlite3.connect("/tmp/horizon-store-copy.sqlite3"); s.backup(d)'`,
     and read `state.runtime.ssh.host_key`
     from the `snapshot` column of `remote_workspaces` for the workspace in that copy
     (a supported export of the saved pin is preferable and is tracked on #474;
     scanning the host would defeat the attestation).
   - Under *Remote repository preparation*, tick *Include explicit first-token
     installation* first if the PAT is to be delivered, then **Review repository
     preparation**; the confirmation that follows carries the token field, the
     first-token consent box and **Confirm repository preparation**. **Check
     preparation receipt** shows the receipt of the original preparation and its
     fixed checkout path `/workspace/horizon/repository` without a second submission;
     it is evidence of that preparation, not of present task readiness, which the
     later task start admits on its own.
   - **gated** (Azure saved-Shell Start): **Show saved panels** → on the saved row
     **Start saved Shell task…**, then the **Start saved Shell task** confirmation
     starts the counter task defined above (optionally **Reopen view** first to open
     its disconnected local view). Until the Azure task-start path lands, the counter
     task for an Azure run cannot be started through the product, and the run stops
     here.
   - **gated** (Azure panel attachment): **Show session panels** is a local listing
     of the board's panels and works today; **Reconnect** on one of them is the
     attachment call and needs the Azure path.
   - **gated** (independent panel addition, a separate blocking product gate):
     configured setup seeds exactly one Shell intent and the overview has no control
     that adds a panel intent to an existing remote workspace. The three-panel item
     requires three independent task and panel identities and processes on B, each
     added through the product, each started on its own and each reconnected under
     its original identity; three views attached to one task do not count, and a
     pre-run confirmation on #474 does not stand in for the control. The flow (an
     **Add independent Shell panel** control on a saved remote workspace, tracked
     under #472) lands before this run; its controls, and the three recorded panel
     identities, are written into this step when it does.
   Once the three gates have landed, record worker, session and task identities, the
   starting counter and the dirty-marker hash. The task must advance its counter at
   least once per 15-second sample.
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
   Because the product created B's group in step 3, journal its exact identity now
   if that was not already done: `client_off.py --manifest m.json journal-group
   --group <B's group> --created created-groups.json` (it reads the ARM ID and full
   tag set into the journal). The journal is what authorizes step 9 to delete B:
   retention beyond the run is not authorized, so B is deleted by this run's
   cleanup, not left for a later pass.
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
   `PowerState/running`. On A, start Horizon again with the same home; it opens the
   session chooser, and the reconnect path admits only the recorded owning session,
   so resume that exact persistent session (the owning session ID recorded in the
   baseline) before anything else. Then open **Environments**, select the same saved
   environment (same workspace, owning
   session, generation and exact resource ID), then **Show session panels** and,
   **gated** (Azure panel attachment), **Reconnect** to the same B and task
   sessions: same worker identity, no additional create, no task replay, dirty bytes
   intact, no credential rotation. Before the worker lifecycle step, read the counter
   and the dirty marker once more over the pinned session on A described in step 7
   and record that counter value: the task keeps running after the journal's last
   sample, so only this reading is the value the post-start comparison uses.
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
   reconnect evidence is captured; never Stop or start B during the off interval. The
   harness's deadline arithmetic (`manifest.py`) reserves provisioning, the observer
   install, the off interval, the return and the cleanup window, not this step, and
   `validate` does not check for it. Until the harness gains a lifecycle-aware check
   (tracked on #474), this is a manual operator requirement: freeze the manifest
   deadline at `validate`'s minimum plus a lifecycle margin of 60 minutes (a Stop with
   its 5-minute verification bound, the check, a Start with its 5-minute bound and up
   to 300 s of readiness, the bounded pinned reads and slack), and immediately before
   this step compare the clock with the deadline: unless at least that margin plus
   the cleanup margin (35 minutes) remains, skip the step, report it as not run, and
   proceed to the observer-key removal and cleanup. It uses the product controls on
   A, in the *Explicit Stop* section of the overview:
   **Stop environment…** → confirm (one Stop; records intent, deallocates B and
   verifies only that the compute reached `PowerState/deallocated`). A verified Stop
   is not yet the retention proof: the retained `worker-data` disk and the saved
   address are confirmed by **Check saved Stop**'s read-only observer, so run it after
   every Stop, verified or not (an unverified Stop leaves the record at `Stop
   requested (saved)`; a confirmed observation writes the saved phase `Stopped` and a
   new revision locally), and continue only once the row shows `Stopped (saved, not
   live)`. **Start environment…**
   is offered only for that verified Stop or an existing Start intent. Then **Start
   environment…** → confirm (records Start intent, starts only the exact worker,
   accepts only the saved identity and pin). The result arrives as a notice and the
   overview then reloads the saved page on its own, keeping the selected workspace
   by ID: the reloaded row must show `Reconciling` before this step continues. Only
   when the notice reports an unverified Start and the reloaded row still shows
   `Start requested (saved)` is **Start environment…** pressed again (the retry
   reuses the saved intent and never re-posts a running worker); a row at
   `Reconciling` is never retried. Then read the retained bytes back without the product's
   panel path, which cannot serve this step: the worker's panel runtime (its sockets
   under `/run/horizon/panels`) does not survive the VM restart, attachment refuses an
   unavailable panel, and Start resumes no task, so the counter stops at its last
   value. A verified Start may still report the endpoint as not attested (the saved
   phase is `Reconciling` either way), so the read is retried under a bound: on A,
   open one pinned SSH session with the retained client key
   (`$HOME/.horizon/remote-ssh-identities/<workflow>-<job>.key`, the saved pin as the
   only known host, port 2222, user `root`) every 30 s for at most 10 minutes until it
   answers, and read `/workspace/progress.counter` and
   `/workspace/horizon/repository/horizon-dirty-marker.txt`: the counter is at least
   the value recorded in the pinned read before the Stop (the task legitimately keeps
   counting while the Stop request is submitted and the VM deallocates), a second
   read 60 s later returns the same counter (nothing is running after the start, so a
   changing value would mean a resumed or replayed task), the marker's SHA-256 equals
   the baseline hash, the host key equals the saved pin, and the worker identity in
   the overview is unchanged. A session that never answers within the bound leaves the
   retention unproven; nothing is retried on the worker beyond the reads. Observer C's restricted key does not survive the restart (the entrypoint
   rewrites `authorized_keys`), so it is not the reader here. The adapter proved the same sequence
   live in runs 16 to 18 (`azure-workspace-live-acceptance.md`); this step proves it
   through the product.
8. **Remove the observer key**: `client_off.py --manifest m.json remove-observer-key
   --worker worker.json --public-key observer.pub`, once the return and the reconnect
   check on A are done and before the run is reported finished. Step 9 deletes B
   only when its journaled identity still matches and within its 20-minute bound (the
   manifest budget is validated, not metered against elapsed cost), so B can outlive
   the run when that delete is refused, fails or runs out of time, and the run's
   observer private key must not keep a reading channel into it. The phase attests B, removes exactly
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
- Merged product paths as of 2026-09-13: configured setup and consent (#561),
  Prepare Repository (#567), Check saved Stop (#574), the first explicit Stop (#575),
  and durable explicit Start with its configured Azure admission and overview control
  (#576, #578, #584). Still refused for Azure and therefore open gates for this pass:
  the saved-Shell task start and configured panel attachment (Reconnect); the
  provider status read is also refused but not used by this procedure. Also open:
  the Azure CLI and an approved, logged-in identity on A, the PAT for the disposable
  repository, the independent panel-addition control for the second and third panel,
  and the manifest's post-setup worker binding. The worker image for this run is the
  lead's tested complete Shell image, exactly
  `horizonworkersa898ee.azurecr.io/horizon-remote-worker-shell@sha256:01c2ea1ed90ee3d3db7a557bfcc0138fc75248317639ce0adb031cd42d06239f`
  (source `290daba7000c4d02f8fbc9c843eea882a68bb6e5`); the older adapter image
  `horizon-remote-worker@sha256:20cc03ef…` proved the adapter lane only and is not
  valid for a product pass. A manifest naming any other digest is an adapter-only
  rehearsal at best.
- Counter progress proves the task kept running. Checkpoint proof needs
  worker-owned checkpoints advancing during the interval.

## Status

No allocation has been made under this runbook yet. The harness and its
deterministic tests are in place and the product Azure lifecycle is merged; the
manifest, current prices, budget and deadline are posted on #474 before the first
paid run, after the client prerequisites above are approved and in place.
