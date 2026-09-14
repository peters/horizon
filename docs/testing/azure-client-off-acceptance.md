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
| Worker B | Separate persistent Azure CPU worker in its own exact group, created on A through the product setup path and prepared through Prepare Repository; its Stop, saved-Stop check, compute Start, saved-Shell task start, panel reconnect and status check are product operations | Azure lane (product Azure paths and adapter) |
| Observer C | The controller's read-only role, outside A and B: the harness's ARM reads and one pinned SSH session per sample with a key that sshd restricts (`restrict,command=`) to a forced reader of the progress and checkpoint files. C is a logical role, not a separate credential: the harness holds one `Az` client over the operator's ambient `az` login, so C's ARM reads use that login; only C's SSH channel is a distinct, restricted identity | Azure lane |
| Operator | The operator's own Azure CLI login on the same controller, the only credential that changes anything from outside A: it provisions and later deallocates and restarts A (`provision-client.sh`, `off`, `return`), journals and reaper-tags B's product-created resources, deletes the run's groups (`cleanup`) and removes A's role assignments and custom role | Azure lane |

C never renews a lease, delivers a keepalive, reconnects a terminal, checkpoints or
replays a task, and the harness phases that act as C issue no ARM write. Two
Azure control-plane credentials mutate B: the product's Create, Prepare
Repository, Stop and Start run on A under A's managed identity, and every
mutating harness phase or controller command in this runbook names the operator
(four further, non-Azure credentials are described where they appear and are
never interchanged: the fresh SSH key pair for A that step 1 generates and only
the controller uses to reach A; the repository PAT that Prepare Repository
delivers to B; the product's saved worker SSH client identity, used by the
SSH-dependent product flows (Prepare Repository, panel attachment, the saved
task start) and by the step 7 pinned reads, while Stop, Check saved Stop and
Start use the managed identity alone; and observer C's restricted key, a fresh
separate key pair generated for C alone and recorded in `worker.json`, never the
product key); the C-versus-operator
separation is procedural (the phases and commands that write) rather than a
credential boundary, because the harness has no second Azure login. The operator may enforce
the declared cleanup deadline.

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
  "worker_group": "unbound",
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
after A has been provisioned from this manifest. `validate` (which
`provision-client.sh` runs before renting A) therefore accepts exactly two values for
it, the literal `unbound` and the adapter's `horizon-ws-<workflow>-<job>` with two
exact UUIDs, and the manifest has two matching states. The example above is frozen in
the first one. *Unbound*: `worker_group` is the literal `unbound`; `validate`
accepts it, `provision-client.sh` (which needs only A's fields) runs and records the
digest of the unbound manifest in the descriptor as `manifest_sha256`, and every
command that acts on B (`install-observer-key`, `off`, `return`, `verdict`,
`remove-observer-key`) refuses to start, because `client_off.py` validates the
manifest before dispatching any of them. Two commands accept the unbound state so
that a crash between the product's create and the binding never strands a paid
resource: `journal-group`, so B can be journaled as soon as it exists, and `cleanup`
in an unbound mode that deletes only groups present in `created-groups.json` whose
journaled adapter tags derive their own name and that are absent from the pre-run
list (it never derives a name from the manifest), so step 10 stays runnable after a
crash before `bind-worker`; the harness tests cover exactly that sequence. *Bound*:
`bind-worker`, run once by the operator after step 3, reads the product-created
group, requires the adapter's `horizon-workflow-id` and `horizon-job-id` tags from
which its `horizon-ws-<workflow>-<job>` name derives, requires it to be absent from
the pre-run group list, requires the descriptor's `manifest_sha256` to equal the
unbound digest of the manifest being bound (nothing but `worker_group` may change
between provisioning and binding), puts B's VM under the deadline reaper with a
read-back, journals the group in `created-groups.json` (written before the manifest,
so a crash between the two leaves B deletable), and only then writes the name into
`worker_group` and prints the bound manifest's digest; from then on `validate`
requires the adapter name. A manifest edited by hand in either direction is not a
runnable product path and is not used. The other fields are frozen before anything
is rented.

For the bound state, `client_off.py
--manifest m.json validate` refuses to run anything until the
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
an off interval of at least ten minutes that exceeds `lease_seconds`, a
harness-only threshold that the verdict applies (the product's persistent Azure
target configures no lease and sets no termination timer on B). `verdict` and `cleanup` also run after the deadline: a late cleanup is exactly
the case that must run. The
manifest, current price and deadline are posted on #474 before the paid run starts
in redacted form: the public post carries the VM sizes, image digest, prices,
budget, off interval, lease, deadline and the SHA-256 of the full unbound manifest
file, while `subscription_id`, `run_id`, `client_group` and, once bound,
`worker_group` stay in the private manifest; the hash covers the unbound file
only, so it lets the lead verify later that the frozen, pre-bind fields were
preserved (the bound manifest differs in exactly `worker_group` and is hashed
and recorded separately once `bind-worker` has written it); credentials never
appear in the public post, the manifest, the journal or the receipts (the steps
below say where they do exist transiently: A's CLI keeps its managed-identity
token cache locally, and the PAT passes through Horizon's memory and transport
during preparation).

## Client A prerequisites for the product path

The product on A authenticates to Azure the same way the controller does: the
subscription-pinned Azure CLI credential (`az account get-access-token --subscription
<id>`), never a stored bearer token. `provision-client.sh` installs the CLI and
attaches the identity only when asked, and it never grants a role or logs in, so
before the product pass A needs, in this order:

1. The Azure CLI on A. `provision-client.sh --with-azure-cli` installs the `azure-cli`
   package from Microsoft's repository during cloud-init (no extension install, no
   login); the readiness gate waits for cloud-init, so the install completes before A
   counts as ready, and the descriptor records `azure_cli_installed`. A client
   provisioned without the flag has no CLI and the product cannot authenticate on it.
2. An identity A can log in non-interactively. The intended shape is a
   system-assigned managed identity on A with a custom role whose `Actions` are
   exactly the following operations, the ones the product paths under test send,
   and no `delete` operation (the write operations are still modification
   authority, see the residual):

   ```text
   Microsoft.Resources/subscriptions/resourceGroups/read
   Microsoft.Resources/subscriptions/resourceGroups/write
   Microsoft.Resources/deployments/read
   Microsoft.Resources/deployments/write
   Microsoft.Compute/virtualMachines/read
   Microsoft.Compute/virtualMachines/write
   Microsoft.Compute/virtualMachines/instanceView/read
   Microsoft.Compute/virtualMachines/deallocate/action
   Microsoft.Compute/virtualMachines/start/action
   Microsoft.Compute/virtualMachines/runCommand/action
   Microsoft.Compute/locations/operations/read
   Microsoft.Compute/disks/read
   Microsoft.Compute/disks/write
   Microsoft.Network/publicIPAddresses/read
   Microsoft.Network/publicIPAddresses/write
   Microsoft.Network/publicIPAddresses/join/action
   Microsoft.Network/networkInterfaces/read
   Microsoft.Network/networkInterfaces/write
   Microsoft.Network/networkInterfaces/join/action
   Microsoft.Network/networkSecurityGroups/read
   Microsoft.Network/networkSecurityGroups/write
   Microsoft.Network/networkSecurityGroups/securityRules/write
   Microsoft.Network/networkSecurityGroups/join/action
   Microsoft.Network/virtualNetworks/read
   Microsoft.Network/virtualNetworks/write
   Microsoft.Network/virtualNetworks/subnets/read
   Microsoft.Network/virtualNetworks/subnets/write
   Microsoft.Network/virtualNetworks/subnets/join/action
   ```

   Why each group: the transport creates and reads the worker's resource group,
   submits the deployment and reads the deployment resource back (never the
   operation-status endpoint); every VM read expands `instanceView` and the
   lifecycle decisions come from its power statuses; `deallocate/action` is Stop,
   `start/action` is Start and `runCommand/action` is the host-key attestation used
   by setup, by Start's readiness path and by Prepare Repository's allocation
   inspection before it dispatches (which inspects the worker and can attest the
   host key through the same channel); deallocate and start are accepted with
   202 and the provider then polls the VM's `instanceView` (covered by the reads
   above), while run-command alone follows the returned `Azure-AsyncOperation`
   status resource with bounded reads, which is the
   `Microsoft.Compute/locations/operations/read` operation, so a role without it
   submits the attestation command successfully and fails while polling it; the
   template creates the data
   disk, the public IP, the NIC, the security group and the virtual network with
   its inline `workers` subnet (the subnet is created and, on a setup retry,
   updated as part of the VNet write, and Azure authorizes that child write as
   `subnets/write`, so it is listed); the security group is created with its
   inline `worker-ssh` rule, whose child write Azure authorizes separately as
   `securityRules/write`; the security group is attached
   to that subnet, which needs `networkSecurityGroups/join/action`, and the NIC
   references only the public IP and that child subnet, which needs
   `publicIPAddresses/join/action`, `subnets/join/action` and the child
   `subnets/read` operation. Plus the built-in Managed Identity Operator
   scoped to `horizon-worker-puller` alone. Check saved Stop is ARM read-only and
   needs nothing beyond the reads. The role grants no
   `delete` action: deletion is not part of this pass and no product path on A
   deletes anything (the setup coordinator dispatches no compensating cleanup when a
   later step fails; it preserves the allocation for recovery and retry), so the
   removal of every group this run creates is authorized and attempted only by the
   operator's cleanup from the controller (step 10) under the operator's own
   credentials, never A's or the observer's;
   a refused, failed or timed-out step 10 leaves the group for the operator, it is
   not a complete run. Neither subscription-wide Contributor nor
   any role with `delete` is assigned to A. The residual that Azure RBAC cannot
   remove is that the `write` actions must sit at subscription scope (the product
   creates one new resource group per worker, so no narrower scope exists before the
   run), which lets a compromised A modify resources of those types in unrelated
   groups for the run's duration. One operation in the list is broader than a
   write: `Microsoft.Compute/virtualMachines/runCommand/action` at subscription
   scope lets A execute arbitrary commands as administrator inside any VM in the
   subscription, not only B, for as long as the assignment exists. That
   command-execution blast radius is disclosed on #474 as its own approval item,
   separate from the write residual, and the only isolation that removes it is a
   subscription holding nothing but this lane's resources; the run itself neither
   detects nor prevents it. The Managed Identity Operator grant has the same shape:
   scoped to `horizon-worker-puller` it limits which identity A may assign, but
   combined with the subscription-scope VM writes it lets a compromised A attach
   that identity to a VM of its own and pull from the registry with it; that
   identity-assignment path is disclosed on #474 with the same approval and
   isolation requirement. Step 10's peer comparison detects only a vanished
   or added peer resource or group (it compares resource IDs and group names, not
   properties or tags), so an in-place mutation of a peer would pass it; the run
   neither prevents nor fully detects that residual, and the only prevention is a
   subscription holding nothing but this lane's resources. **This assignment is not
   made yet.** Per the
   #474 coordination, the proposal posted there for approval must carry, before any
   role or identity is created or assigned: the action list above with its
   justification from the product transport (file and line per action) and the
   official Azure RBAC operation reference, the exact scope of each assignment, the
   owner of the identity, and an exact expiry and removal plan. Step 10's `cleanup`
   deletes resource groups only; deleting A's group removes the system-assigned
   identity but can leave its role assignments and the custom role definition
   behind, so the removal is an explicit operator step from the controller, before the manifest
   deadline and before A's group is deleted (a system-assigned principal may no
   longer resolve by `--assignee` once A is gone): when the roles are granted,
   each step is recorded the moment it succeeds, not after the whole grant (the
   custom role's definition ID from `az role definition create`, then each
   assignment's ID from its own `az role assignment create` output, written to
   the run's private record before the next command runs). These are remote,
   non-transactional writes, so each intended write is recorded before it is
   sent (the chosen custom role name, and for each assignment the principal,
   role and scope), and a timeout or lost response is reconciled from that
   record rather than assumed absent: `az role definition list --subscription
   <id> --custom-role-only true --name <chosen name>` and `az role assignment
   list --subscription <id> --assignee <principal> --scope <scope> --role <role>`
   either return the accepted write, whose ID is then recorded, or confirm
   absence; A's Horizon is not launched for the product path until every
   intended write is either recorded with its ID or verified absent. Ordering:
   a system-assigned identity exists only once A's VM exists, so step 2
   provisions A first (its launch gate needs no Azure identity and touches no
   Azure resource; with `--assign-identity` its `az vm create` attaches the
   system-assigned identity and the descriptor records `system_assigned_identity`,
   and without the flag the operator gives A's exact VM the identity afterwards, the
   one operator mutation of A outside the harness phases: `az vm identity assign
   --subscription <id> --ids <A's VM ID from client.json>`), the operator verifies
   it with
   `az vm show --subscription <id> --ids <A's VM ID> --query identity.type -o
   tsv` printing `SystemAssigned`, and reads the principal with `az vm show
   --subscription <id> --ids <A's VM ID> --query identity.principalId -o tsv`,
   which must be non-empty;
   creates the role and the two assignments as described here, verifies them,
   and only then runs the login and token probe on A and continues to step 3;
   A does nothing with Azure in between. If the definition or
   the second assignment fails after an earlier step succeeded, nothing further
   is granted, the steps already recorded are rolled back immediately with the
   same delete commands and verified with the same empty listings, and the
   failure is posted on #474 before any retry; A is never provisioned or used
   with a partial grant. At removal time,
   remove exactly those two with `az role assignment delete --subscription <id>
   --ids <custom-role assignment ID> <Managed Identity Operator assignment ID>`
   (never a filter by assignee or scope, which would also remove any unrelated
   assignment the principal holds), then `az role definition delete --subscription
   <id> --name <the recorded custom role definition ID>` (the ID, never the display
   name, which another custom definition could share), verified with `az role
   assignment list --subscription <id> --all --assignee <principal ID>` printing an
   empty list and `az role definition list --subscription <id> --custom-role-only
   true --query "[?id=='<the recorded definition ID>']"` printing an empty list
   (every call pinned to the manifest
   subscription, as the harness pins its own), and the verification recorded with
   the run. Cost approval is not
   approval for this authority. No Azure user credential is copied to A: the
   managed identity is A's only Azure credential. The repository PAT in item 4 is a
   different credential, typed by the operator into the preparation's token field
   on A for one delivery and never stored there. After step 2 has provisioned A
   (its launch gate starts and stops Horizon once, with no Azure identity and no
   Azure call) and the roles above are verified, and before the product-path
   launch in step 3, under the same `HOME`
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
   profile immutably to the worker at creation. Target validation requires the
   image's registry prefix to equal the profile's `registry_login_server`, so for
   the image named under Labelling that field must be exactly
   `horizonworkersa898ee.azurecr.io`; any other value has Create refused.
4. For the Git lane, a user-supplied repository-scoped PAT for the authorized
   disposable repository. It is typed only into the repository preparation's token
   field on A (stdin-only delivery to the worker) and is never written to A's disk or
   to any manifest, journal or receipt.

One product gate blocks the baseline and return steps below until it lands: the
independent panel-addition control (tracked under #472) that the three-panel item
needs. The Azure provider paths this procedure drives are merged: the saved-Shell
task start (#615), panel attachment for Reconnect (#610) and the provider status
read (#613), each admitting Azure through the same admission as Stop and Start.
The step marked **gated** below cannot be executed for an Azure worker today; a
product run stops before it and collects no three-panel evidence until that
control lands.

The deterministic task this lane runs on B, for the pinned worker image: saved Shell
panel with program `/bin/sh` and arguments (one JSON array, pasted verbatim into
*Literal arguments (JSON array)*, so the shell receives a single `-c` script)
`["-c", "i=0; while :; do i=$((i+1)); echo $i > /workspace/progress.counter.tmp &&
mv /workspace/progress.counter.tmp /workspace/progress.counter; sleep 5; done"]`
for the first task, which is the one the observer samples (`worker.json`'s
`progress_path`) and the one the verdict's monotonic check reads; the second and
third tasks of the three-panel item run the same script with their own files,
`/workspace/progress-2.counter` and `/workspace/progress-3.counter` (and matching
`.tmp` names), never the first task's path, so no task can overwrite another's
value, and their liveness is proven by their own independent identities and by
reading their files in the step 7 pinned reads with the same non-decreasing and
stable-after-Start checks, not by the harness verdict, which reads one path,
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
   for the cleanup comparison; and prove that the deadline reaper this runbook
   relies on exists and is running in the manifest subscription: the automation
   account `horizon-spike-reaper` must have its schedule enabled and a job that
   completed successfully within the last 30 minutes (`az automation job list
   --subscription <id> --resource-group horizon-worker-registry
   --automation-account-name horizon-spike-reaper --query
   "[?status=='Completed'] | sort_by(@, &endTime) | [-1].{id:jobId, end:endTime}"`),
   recorded with the run; without that proof nothing is rented, because the
   controller-loss cost stop below is the reaper.
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
     task below, *Panel directory (optional)* empty, the disk size `32` GiB (the
     size the adapter evidence fixed for this candidate; the form has no default
     and the manifest does not bind it, so it is recorded with the baseline
     evidence), and the
     *Azure CPU cost limit*, which the product treats as optional but this
     acceptance requires; **Review request** shows the complete profile, the
     declared price and the immutable-binding disclosure. The manifest's
     `hourly_cost_micros` and `budget_micros` are validation-only metadata (the
     harness checks that both are positive and no phase meters or enforces them,
     for A or for B); B's only enforced limit is the profile's
     `declared_hourly_cost_micros` and the ceiling typed here, both whole
     micro-units of the billing currency per hour (1,000,000 = one currency unit;
     the product performs no USD conversion and shows no live quote). So before the
     consent box is ticked the review screen must show the declared price as
     exactly the micro-unit value posted for B on #474 (`107000` per hour, typed as
     plain digits because the field is parsed as an integer and rejects
     separators, for
     `Standard_D2s_v3` under the billing assumption stated there; the external
     retail price that value was derived from is recorded on #474, not checked
     here), the *Azure CPU cost limit* must be that same value, so admission refuses
     any profile declaring more (the limit is a per-hour compute ceiling, not the
     run's total budget, which only the manifest and the operator bound), and the
     profile's `vm_size` must read `Standard_D2s_v3`; a mismatch aborts the step
     before **Create task-free worker** and the profile is corrected on A first.
     Then tick the consent box and press **Create task-free worker**.
     Nothing is checked out and no task starts here. Then **Refresh saved page** and select the new row: the repository, panel
     and Stop sections render only for a selected saved row, and the page shown after
     creation is still the previous one. Record now the workspace, owning session,
     workflow and job identities and the resource-group name
     `horizon-ws-<workflow>-<job>` (the value `bind-worker` will write into
     `worker_group`, which stays `unbound` until then), which follows
     from those identities. The full ARM group ID
     `/subscriptions/<id>/resourceGroups/horizon-ws-<workflow>-<job>` (the overview's
     *Exact resource ID* and `worker.json`'s `group_id`) is recorded the moment
     the overview shows it: a confirmed Create already persists the worker
     handle, so it is usually visible right after creation while Azure is still
     provisioning: the handle is recorded as soon as the provider returns any
     observed status, a `Provisioning` one included, so a normal provisioning
     observation shows the resource ID and is not a missing identity. The saved
     allocation is left without a worker identity, and the overview at `No
     resource identity recorded`, only when no status was observed at all: the
     Create response was lost or unconfirmed, or the provider or ARM call
     errored before a status came back, in which case the overview may never show it (recovery
     can legitimately observe an absent worker), so the group ID is derived from
     the recorded workflow and job identities as
     `/subscriptions/<id>/resourceGroups/horizon-ws-<workflow>-<job>` and verified
     with the `az group show` read below; a group that read cannot find is the
     separate recovery outcome described there. The remaining `worker.json` fields
     (`vm_id`, `instance_id`, `host`, the attested host key) are captured only
     after that Check reports the deployment observed. The group name is
     derived from the workflow and job identities the record carries from the
     moment the allocation is saved, before the deployment is sent, so it is known
     even when an accepted deployment followed by a lost observation leaves the
     record without a worker identity yet. Once the deployment has produced the
     worker VM (the product's Check reports it, or the reads below show it), bind it:
     `client_off.py --manifest m.json bind-worker --group horizon-ws-<workflow>-<job>
     --groups-before groups.json --created created-groups.json --client client.json`,
     which checks the adapter tags, the pre-run absence and the descriptor's manifest
     digest, tags B's VM for the reaper with a read-back, journals the group and
     writes `worker_group` in one step; before the VM exists it refuses and reports
     that `journal-group` is still available. The
     `journal-group` command below is its journaling half. The journal is
     append-only and `journal-group` refuses a group already present in it, so
     the two are alternatives, never a sequence: after `bind-worker` has run,
     `journal-group` is not run for B; `journal-group` is run only when B is not
     yet in `created-groups.json` (today's harness, or a crash before the binding
     with B already created). In that case journal it the moment the record exists,
     whether or not the setup goes on to succeed: `client_off.py --manifest m.json
     journal-group --group horizon-ws-<workflow>-<job> --created
     created-groups.json`. The command refuses a group it cannot read, and a
     refusal is unknown, not absence (**Check this setup** cannot say that Azure
     holds no group: its observed result covers a reconciled-missing group too), so
     the operator resolves the read directly: `az group show --subscription <id>
     --name horizon-ws-<workflow>-<job>` either returns the group, which is then
     journaled, or a definitive `ResourceGroupNotFound`, which is recorded with the
     time; any other answer (throttling, transport, authorization) is retried, and
     the run does not continue while the group is unreadable. Nothing on A deletes
     a group after a failed setup (the coordinator preserves the allocation for
     retry), so a group journaled only after a successful baseline would survive an
     aborted run. The window between the product's create and this journal entry
     is real: if A, Horizon or the operator fails inside it, the group is neither
     journaled nor reaper-tagged, and an asynchronous deployment can still
     materialize an untagged VM afterwards. A controller-side recovery cannot
     close that window after a permanent failure, so it is a blocking
     prerequisite for the paid run with a safety path that does not depend on
     the controller: either the worker deployment template carries the reaper
     tags itself (a lead-owned product change, requested on #474 with this
     runbook: `purpose` and a `deadline` taken from the profile or the request)
     or a subscription-side policy appends them to every resource created in a
     `horizon-ws-*` group (an authority the lane does not hold, requested the same
     way). Until one of those is in place the paid run does not start. The
     controller-side recovery below covers the surviving-controller case only;
     it runs before
     the run proceeds after any interruption of step 3, and again before step 10:
     `az group list --subscription <id> --query "[?starts_with(name, 'horizon-ws-')].{name:name, tags:tags}"`
     is compared with `groups.json`; a group absent from the pre-run list whose
     `horizon-workflow-id` and `horizon-job-id` tags both equal the workflow and
     job identities recorded for this run (both, as `bind-worker` and the cleanup's
     own binding check require: several jobs can share a workflow, and a
     workflow-only match could journal a peer worker) is journaled with
     `journal-group` before anything else happens. If that identity
     was never recorded (A died before it could be read), no group is journaled or
     deleted by guesswork: the candidates are posted on #474 with their tags and
     creation times and resolved by the lead before cleanup, because another lane
     may hold `horizon-ws-` groups in the same subscription. With the harness
     `bind-worker` step, this journal entry is written in the same step as the
     binding. Then, as the operator on the
     controller (a write, so an operator step even though the same `az` login also
     serves C's reads), read the journaled group's VM: `az vm list --subscription <id> --resource-group
     horizon-ws-<workflow>-<job> --query '[].{id:id, name:name}'`. If it lists no
     `worker` VM, the deployment may still be in flight (the VM appears only as the
     deployment progresses), so absence is not yet known: read `az deployment group
     show --subscription <id> --resource-group horizon-ws-<workflow>-<job> --name
     worker --query properties.provisioningState`; while it answers `Accepted` or
     `Running`, wait, run **Check this setup** and read again. Only when the
     deployment is absent, `Failed` or `Canceled` is the outcome known. A
     `Failed` or `Canceled` deployment is terminal whether or not a VM was left
     behind (ARM can leave a partially created VM; the product's observer may
     even report it under the exact worker identity with a `Failed` status, which
     is a recorded identity that is not usable, distinct from no identity
     recorded, and the evidence names which of the two it was): record it with
     the time, do not continue to the baseline, do
     not bind, tag or start anything, and go to step 10. A `Succeeded` deployment
     with an empty VM list is a mismatch, not a wait: the worker existed and is
     gone, so record it with the time and the deployment's outputs, do not
     continue, and go to step 10 with the journaled group. A group that exists in
     that state was journaled above and step 10 deletes it; if the group itself was never readable,
     `journal-group` has refused and appended nothing, so there is no B entry,
     step 10 reports the worker group as unjournaled and deletes only A's group,
     and that is the correct outcome: no cleanup record is written by hand.
     Otherwise, once the VM
     exists, put it under the deadline reaper, which the
     product deployment
     does not do (it tags B with worker identity tags only, and the reaper skips a VM
     without both `purpose` and `deadline`): `az tag update --subscription <id>
     --resource-id <B's VM ID> --operation merge --tags purpose=horizon-azure-vm-spike
     deadline=<the manifest's cleanup_deadline_utc>` and read the two tags back with
     `az vm show --subscription <id> --ids <B's VM ID> --query tags`. The tags go on
     the VM only: the product checks its own tags as a subset, so extra VM tags are
     tolerated, while step 10 refuses a group whose tag set changed since it was
     journaled, so the group is never retagged. This is the cost stop if A, Horizon
     or the operator dies before step 10: the reaper deallocates B after the
     deadline (it never deletes, so the retained disk bills until step 10 or the
     operator removes the group). `bind-worker` performs exactly this tagging with
     the same read-back as part of the binding; the manual commands above are the
     path for a run whose manifest was bound by hand, which is not a product pass,
     and in every case the run does not proceed without the read-back recorded.
   - **Check this setup** until it reports the original setup as observed with a
     complete worker identity and the attested pin: the saved
     phase becomes `Reconciling` (setup recovery never writes `Ready`) and the record
     carries the attested pin, read through ARM's run-command channel and never
     trusted on first connection. `Original setup observed` alone is not enough:
     the check can report it while Azure is still provisioning and before any
     host key is saved (the notice itself says it is not readiness), and the
     next bullets and `worker.json` need the pin, so a check that observes the
     setup without the identity and pin is repeated until they are present. Check reports in the setup notice only; the saved
     page is not reloaded for it (unlike the Stop-section operations), so press
     **Refresh saved page**, which keeps the selected workspace by ID, and confirm
     the refreshed row before the next step. The
     overview deliberately never shows the pin; the `host_key` for `worker.json` is
     taken from A's saved record, whose snapshot is JSON. The store runs in WAL mode,
     so a plain file copy can miss the newest rows: take a consistent copy with
     SQLite's online backup through `python3`, which the client image has (cloud-init
     depends on it; the provisioner installs no `sqlite3` binary):
     as A's Horizon user, in one shell whose exit trap removes the copy on every
     exit, interrupted or not:

     ```sh
     (
       umask 077
       copy=$(mktemp /home/horizon/store-copy.XXXXXX) || exit 1
       trap 'rm -f "$copy"' EXIT INT TERM
       python3 - "$copy" <<'PY'
     import json, sqlite3, sys
     src = sqlite3.connect("/home/horizon/.horizon-client-home/.horizon/cloud-run/workflows.sqlite3")
     dst = sqlite3.connect(sys.argv[1])
     src.backup(dst)
     src.close()
     for (workspace, snapshot) in dst.execute("select workspace_local_id, snapshot from remote_workspaces"):
         print(workspace, json.loads(snapshot)["state"]["runtime"]["ssh"]["host_key"])
     PY
     )
     ```

     Take the `host_key` printed for this run's workspace. The copy holds the pinned
     host key and the repository details, so it is never a predictable or
     world-readable path and never outlives the subshell (a supported export of the
     saved pin is preferable and is tracked on #474; scanning the host would defeat
     the attestation).
   - Under *Remote repository preparation*, tick *Include explicit first-token
     installation* first: a fresh worker holds no credential, and the unticked
     path ("use the worker's existing credential; no PAT will be sent") is outside
     this acceptance, so the tick, the token and the first-token consent are
     mandatory here. Then **Review repository
     preparation**; the confirmation that follows carries the token field, the
     first-token consent box and **Confirm repository preparation**. **Check
     preparation receipt** shows the receipt of the original preparation without
     a second submission (its notice is `Original preparation complete. This
     receipt does not prove current task readiness.`; the fixed checkout path
     `/workspace/horizon/repository` is validated by the preparation protocol and
     is not displayed, so it is recorded as protocol-validated evidence, not read
     off the receipt);
     it is evidence of that preparation, not of present task readiness, which the
     later task start admits on its own.
   - **Show saved panels** → on the saved row **Start saved Shell task…**, then the
     **Start saved Shell task** confirmation starts the counter task defined above
     (optionally **Reopen view** first to open its disconnected local view). The
     confirmation is refused if the saved record moved to a Stop, Stopped or Start
     phase since it was prepared, or if the profile binding drifted.
   - **Show session panels** is a local listing of the board's panels; **Reconnect**
     on one of them is the attachment call, which admits Azure under the saved
     identity and pin and starts nothing.
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
   Only once the panel-addition control has landed, record worker, session and task identities, the
   starting counter and the dirty-marker hash. The task must advance its counter at
   least once per 15-second sample.
   Write `worker.json` for the observer: `vm_name`, `port`, `host_key` (the attested
   key), `observer_key_path` (the private half of a fresh Ed25519 key generated for
   observer C only, never the client's worker key), `progress_path` and optionally
   `checkpoint_path` (plain, distinct paths under `/workspace`, no `.` or `..`
   components, so no two spellings can name one file), and the identity
   recorded now, before A is stopped: `group_id`, `vm_id`, `instance_id` (the VM's
   `vmId`, which a same-name recreation does not keep) and `host` as ARM reports them.
   Then, as an operator step (the command mutates B through ARM run-command; it
   runs under the same `az` login as everything else, and is named so that it is
   never counted among C's reads), install the observer key as a restricted key: `client_off.py --manifest m.json
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
   tag set into the journal). The journal is what authorizes step 10 to delete B:
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
   `PowerState/running`. On A, start Horizon again with the same home. The reconnect
   path admits only the recorded owning session, so the session that ends up open
   must be that exact persistent session (the owning session ID recorded in the
   baseline): startup may open a single recoverable session directly without the
   chooser, in which case verify the opened session's ID against the baseline
   before anything else, and when the chooser does appear resume that recorded
   session from it; a different session is closed, never used. Then open **Environments**, select the same saved
   environment (same workspace, owning
   session, generation and exact resource ID), then **Show session panels** and
   **Reconnect** to the same B and task sessions: same worker identity, no additional create, no task replay, dirty bytes
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
   deadline at `validate`'s minimum plus 227 minutes (225 plus two rounding minutes, because `manifest.py` floors
   the observer-install and return-setup bounds to whole minutes): a
   product-baseline reserve of
   90 minutes (`validate` reserves nothing for step 3: the setup deployment and its
   check, Prepare Repository, the panel steps and the identity recording all run
   before `off` against the same absolute deadline; the reserve is a hard wall,
   not a description: before step 3 starts the operator writes down its wall
   clock, provisioning start plus the 30-minute provisioning bound, the 47-minute
   observer-install bound and these 90 minutes, and if step 3 has not completed
   by then the baseline is aborted and recorded, no `off` phase runs, and the run
   goes to steps 8 to 10), 15 minutes for the manual
   reconnect check on A after `return`, 45 minutes for step 8 and the role removal
   (the observer-key removal can spend about 33 minutes at the harness's bounds, plus the 10-minute role-removal bound, and
   `validate` reserves nothing for either), a lifecycle margin of 60 minutes (a Stop
   with its 5-minute verification bound, the check, a Start with its 5-minute bound
   and up to 300 s of readiness, the bounded pinned reads and slack) and the
   15-minute return margin the gate below counts but `validate`'s minimum does
   not. Worked example with `off_minutes` 12: `validate`'s minimum is 160 minutes,
   so the deadline is at least 387 minutes after provisioning starts; at the gate
   below, with every bound spent (30 provisioning, 47 observer install, 90
   baseline, 29 off setup, 12 off, 22 return setup, 15 reconnect check), 245
   minutes have elapsed and at least 140 remain, which is what the gate requires.
   Immediately before
   this step compare the clock with the deadline: unless at least that margin plus
   80 minutes remains (step 8 at its bounds, about 33 minutes, the 10-minute role removal, the
   15-minute return margin and the 20-minute cleanup bound), skip the step, report
   it as not run, and proceed to the observer-key removal and cleanup. It uses the product controls on
   A, in the *Explicit Stop* section of the overview:
   **Stop environment…** → confirm (one Stop; records intent, deallocates B and
   verifies only that the compute reached `PowerState/deallocated`). A verified Stop
   is not yet the retention proof: the retained `worker-data` disk and the saved
   address are confirmed by **Check saved Stop**'s read-only observer, so run it after
   every Stop, verified or not (an unverified Stop leaves the record at `Stop
   requested (saved)`; a confirmed observation writes the saved phase `Stopped` and a
   new revision locally), and continue only once this run's check has reported the
   notice `Retained Stop confirmed at this check; completion is saved` and the
   reloaded row shows `Stopped (saved, not live)`. The row alone is not enough: a
   record already at `Stopped` keeps that label, and Start stays offered, when a
   later check fails or is unverified. A check without that notice is repeated
   only while its notice is a pending or provider-side one, at most three times
   inside the gate above; the notice `Worker is absent: retained Stop cannot be
   certified`, or an identity or observer mismatch, is terminal: retention is
   recorded as unproven and the run proceeds to steps 8 to 10. **Start environment…**
   is offered only for that verified Stop or an existing Start intent. Then **Start
   environment…** → confirm (records Start intent, starts only the exact worker,
   accepts only the saved identity and pin). The result arrives as a notice and the
   overview then reloads the saved page on its own, keeping the selected workspace
   by ID, and the reloaded row decides which of two paths follows: `Reconciling`
   is the success path and this step continues to the reads below; `Start
   requested (saved)` is the retry path. Only
   when the reloaded row still shows `Start requested (saved)` and the notice
   reports either an unverified Start or the loss of the local completion (`The
   Start could not finish locally. Refresh saved inventory; if Start intent
   remains, press Start again.`) is **Start environment…** pressed again (the retry
   reuses the saved intent and never re-posts a running worker). The product
   collapses provider and ARM failures, an expired token among them, into the same
   unverified notice, so the notice alone cannot separate a lost result from a
   denied one; before every retry the operator classifies out of band: on the
   controller two reads, identity from the VM resource and power state from the
   instance view (a plain `az vm show` carries no instance view):

   ```sh
   SUB=$(jq -r .subscription_id m.json)
   VM_ID=$(jq -r .vm_id worker.json)
   az vm show --subscription "$SUB" --ids "$VM_ID" --query vmId -o tsv
   az vm get-instance-view --subscription "$SUB" --ids "$VM_ID" \
     --query "instanceView.statuses[?starts_with(code, 'PowerState/')].code | [0]" -o tsv
   ```

   The first must print the recorded `instance_id` and the second one of the
   full codes `PowerState/deallocated`, `PowerState/stopped` (which the provider
   treats as stopped-allocated and accepts for Start), `PowerState/starting` or
   `PowerState/running`,
   and on A the token probe from prerequisite 2 must
   still print an expiry; a missing or replaced VM, or a failed probe, ends the
   retries as a non-retryable outcome. Retries are budgeted: at most three presses
   in total, each only after the previous result has been reloaded, and none once
   the clock is past the gate above (the deadline minus 140 minutes), because every
   press can spend the 5-minute Start bound and up to 300 s of readiness. An
   identity or absence result is never retried, and a row at `Reconciling` is
   never retried. The reads below happen only after this run's own Start has
   succeeded and the reloaded row shows `Reconciling`; after a non-retryable Start
   outcome, or a retry budget exhausted at `Start requested (saved)`, retention is
   recorded as unproven and the lifecycle step ends here without touching the
   worker (a read at that point could come from a stale or replaced endpoint and
   would prove nothing about the saved worker). Then read the retained bytes back
   without the product's panel path, which cannot serve this step: the worker's panel runtime (its sockets
   under `/run/horizon/panels`) does not survive the VM restart, attachment refuses an
   unavailable panel, and Start resumes no task, so the counter stops at its last
   value. A verified Start may still report the endpoint as not attested (the saved
   phase is `Reconciling` either way), so the read is retried under a bound: on A,
   open one pinned SSH session with the retained client key
   (`$HOME/.horizon/remote-ssh-identities/<workflow>-<job>.key`, the saved pin as the
   only known host, port 2222, user `root`) every 30 s for at most 10 minutes until it
   answers, and read `/workspace/progress.counter`, and once the three-panel gate
   has landed `/workspace/progress-2.counter` and `/workspace/progress-3.counter`
   under exactly the same checks against their own baseline values, and
   `/workspace/horizon/repository/horizon-dirty-marker.txt`: the counter is at least
   the value recorded in the pinned read before the Stop (the task legitimately keeps
   counting while the Stop request is submitted and the VM deallocates), a second
   read 60 s later returns the same counter (nothing is running after the start, so a
   changing value would mean a resumed or replayed task), the marker's SHA-256 equals
   the baseline hash, the host key equals the saved pin, and the worker identity in
   the overview is unchanged. The product's Start compares the VM it observed
   immediately before its own call with the one after it, not with the worker that
   existed before the Stop, so the operator closes that gap from the controller:
   `az vm show --subscription <id> --ids <B's VM ID> --query '{id:id, vmId:vmId}'`
   must return the `vm_id` and `instance_id` recorded in `worker.json` in step 3,
   and `az group show --subscription <id> --name horizon-ws-<workflow>-<job> --query
   id` must return its `group_id`; a same-name replacement
   created while B was stopped keeps the name but not the `vmId`, and fails here.
   A session that never answers within the bound leaves the
   retention unproven; nothing is retried on the worker beyond the reads. Observer C's restricted key does not survive the restart (the entrypoint
   rewrites `authorized_keys`), so it is not the reader here. The adapter proved the same sequence
   live in runs 16 to 18 (`azure-workspace-live-acceptance.md`); this step proves it
   through the product.
8. **Remove the observer key**, an operator step (the command mutates B through
   ARM run-command under the operator's login, and a run must not end with it
   skipped once the key was installed). It applies only when `install-observer-key`
   was attempted in step 3, which the run records at that moment; if the baseline
   aborted before that (no `worker.json`, or the install never ran), there is no
   line to remove and the command cannot validate the descriptor, so the operator
   destroys the generated observer private key at the exact path recorded when
   it was generated (the key generation in step 3 records that path the moment
   the key exists, the same value that goes into `worker.json` as
   `observer_key_path`; no default filename is assumed, and a key whose recorded
   path is unavailable is reported as a residual credential instead of guessed
   at), records that step 8 did not apply, and proceeds to
   steps 9 and 10. Otherwise: `client_off.py --manifest m.json remove-observer-key
   --worker worker.json --public-key observer.pub` (the phase binds itself to a
   35-minute wall clock, like cleanup's own bound, and hands every CLI call what is
   left of it; an expired bound leaves the removal unproven and takes the
   key-destruction path below), once the return and the reconnect
   check on A have reached any recorded terminal result (passed, failed within
   their bounds, or skipped and recorded as such) and before the run is reported
   finished; a failed or skipped return does not skip this step, since B may
   still be running with the observer line in place. Step 10 deletes B
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
   the file), but the run does not rely on that. The command first
   attests a running B. A failed or exhausted Start does not decide B's state (the
   provider may have sent `start/action` and then failed its identity, readiness
   or host-key observation, leaving B running under a durable Start intent with
   no compensating deallocation), so after step 7 the operator re-reads B's power
   state with the instance-view read above: whenever B is running, this
   step runs in full and must pass. A transitional code (`PowerState/starting`,
   `PowerState/deallocating`, `PowerState/stopping`) is re-read every 30 s for at
   most 10 minutes until it settles; a code still transitional after that bound
   is treated as not running and reported. Only for a B that is not
   `PowerState/running`, whether `PowerState/deallocated`, `PowerState/stopped`
   (stopped-allocated, which a failed or timed-out Start can leave behind and
   which this command cannot attest either), settled from a transitional code
   into one of those, or unavailable, can it not run. Nothing restarts B
   outside the product to make it runnable: in that state the operator destroys
   the observer private key on the controller (`shred -u <the path recorded as
   observer_key_path in worker.json>`, the only copy; the public half is inert
   without it) and records that, so the retained `authorized_keys` line can no
   longer be exercised by anyone, and proceeds to
   step 10. The same destruction applies to every unproven removal: whenever this
   step does not pass (B running but the removal unproven, or B deallocated), the
   key is shredded before B is allowed to outlive the run, and the run records it.
   Step 10 then deletes the group with the OS disk whose container layer holds
   that line (the line lives in the running container's `authorized_keys`, never
   on the data disk, which holds only `/workspace`). If step 10 is
   refused, fails or runs out of time, the residual depends on B's state, which
   the operator re-reads with the instance-view read, waiting out a transitional
   code (`PowerState/starting`, `PowerState/deallocating`, `PowerState/stopping`)
   every 30 s for at most 10 minutes first. A B at `PowerState/running` (the
   normal case after a successful step 7 Start) or `PowerState/stopped`
   (stopped-allocated), or still transitional after that bound,
   keeps billing compute, so the operator deallocates it from the controller,
   the one out-of-product power-state mutation of B this runbook allows (the
   reaper tags and the observer-key line are the other, non-power, controller
   writes to B) and only on
   this failure path, and only after re-attesting the exact B immediately
   before it: `az vm show --subscription <id> --ids <B's VM ID> --query
   '{vmId:vmId, tags:tags}'` must return the recorded `instance_id` and tags
   carrying the recorded `horizon-workflow-id` and `horizon-job-id` (a same-name
   recreation keeps the resource ID but not the `vmId`); any mismatch or
   unreadable answer refuses the deallocation and is reported as part of the
   residual instead. Then `az vm deallocate --subscription <id> --ids <B's VM ID>
   --no-wait`, then the instance-view read every 30 s for at most 10 minutes
   until it prints `PowerState/deallocated`; a deallocation that does not
   complete in that bound is itself reported. In every state the run is then
   reported with the residual (a deallocated VM with its retained disks, the
   group still present, and in the container layer an inert observer line that
   the entrypoint rewrites on the next container start), the group is handed to
   the lead on #474 for removal, and the run is not reported clean.
9. **Role removal.** As the operator and while A's group
   still exists, remove A's authority exactly as prerequisite 2 records it, every
   call under `timeout 90` (the harness's own CLI bound) and the whole substep
   under 10 minutes of the 45 reserved for it and step 8: `az
   role assignment delete --subscription <id> --ids <custom-role assignment ID>
   <Managed Identity Operator assignment ID>`, then `az role definition delete
   --subscription <id> --name <the recorded custom role definition ID>`, then
   verify with `az role assignment list --subscription <id> --all --assignee <A's
   principal ID>` printing `[]` and `az role definition list --subscription <id>
   --custom-role-only true --query "[?id=='<the recorded definition ID>']"`
   printing `[]`, and record both listings with the run. When the substep's
   bound expires, or a listing is not empty, record which recorded IDs are still
   present and continue to step 10 anyway, so that compute stops on time; the run
   is then reported with A's residual authority as a finding and the remaining
   removal is handed to the lead on #474 with the exact IDs. A run
   whose listings are not empty is not reported clean even if the groups are
   deleted.
10. **Cleanup**: `client_off.py --manifest m.json cleanup --groups-before groups.json
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
   Finally, whatever the outcome, the controller destroys A's private key
   (`shred -u key; rm -f key.pub`) and, if step 8 has not already done so, the
   observer's private key at the recorded `observer_key_path`: after a proven
   absence they are unnecessary secrets, and when A's group or B could not be
   proven absent the run records that a surviving VM may still authorize the
   public halves, which are inert once the private halves are destroyed.

## Labelling

- A run whose B was created through the adapter's live driver instead of the
  product path is an **adapter-only rehearsal**. It exercises A, C, the off interval
  and the cleanup, and it is reported as such; it is never the #475 product pass.
- Merged product paths as of 2026-09-14: configured setup and consent (#561),
  Prepare Repository (#567), Check saved Stop (#574), the first explicit Stop (#575),
  durable explicit Start with its configured Azure admission and overview control
  (#576, #578, #584), panel attachment for Reconnect (#610), the provider status read
  (#613) and the saved-Shell task start (#615). Every product path this procedure
  drives now admits Azure. Still open before the paid run: an approved, logged-in
  identity on A, the PAT for the disposable repository, the independent
  panel-addition control for the second and third panel, and the reaper tags on the
  worker deployment (or the subscription policy that appends them). The worker image for this run is the
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
