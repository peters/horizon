# Azure Linux VM workspace spike

Bounded, journaled, self-cleaning spike harness for the Azure CPU lane of
[#474](https://github.com/peters/horizon/issues/474). It creates **one exact
task-owned resource group per sample**, measures the timings #474 asks for, checks
the on-worker storage and retention behaviour, deletes the sample and proves that
the group is gone with nothing left under it and no pre-existing resource missing. Run
[`scripts/azure-workspace-preflight`](../azure-workspace-preflight/README.md) first.

`--max-minutes` (default 120) bounds the **active phase**: every blocking call runs
under the remaining time and, once it expires, the harness stops measuring, deletes
the sample and exits 4. Two exceptions extend exposure and must be included when
calculating the maximum: cleanup has its own fixed 25-minute bound, and `--keep`
skips deletion entirely. Compute is bounded independently of this controller by a
mechanism that exists **before** any spike VM does: the subscription-side reaper
created once by [`setup-spike-reaper.sh`](setup-spike-reaper.sh), an Azure Automation
runbook running every 15 minutes with a power-only identity that deallocates every
VM tagged `purpose=horizon-azure-vm-spike` whose `deadline` tag (UTC) has passed.
Every spike VM carries both tags in the same create call, so a VM cannot exist
without them, and the harness refuses to create anything unless the reaper's
schedule is present and enabled (journaled as `reaper_present`). `--prove-reaper`
moves a sample's deadline tag into the past after the restart checks and requires
the reaper alone to deallocate the VM within 25 minutes (gate `reaper_bound`).
Maximum compute exposure after a lost controller is therefore the deadline plus one
reaper interval plus the deallocate time.

Two further layers are defense in depth, not the primary bound: the VM's identity
gets the built-in power-only role `Desktop Virtualization Power On Off Contributor`
on the sample group before the deployment and the VM's cloud-init arms a systemd
timer as its first `runcmd` step (after the package module has installed Docker,
before the workspace bootstrap and the image pull) that deallocates the VM one minute
after the deadline, proven per sample by verifying the timer is enabled
and active, firing it once and observing `PowerState/deallocated` (gate
`guest_lifetime_bound`); and the deployment also creates an Azure VM auto-shutdown
schedule, which depends on the VM and is therefore only a backstop. In every case
disks and the static IP persist until the tagged resource group
(`horizon-spike-474-*`, tag `deadline`) is deleted by hand. It never registers a provider, never publishes
an image and touches nothing outside the sample resource group.

## Prerequisites

- A GNU/Linux controller (the harness uses GNU `timeout`, `date -d` and millisecond
  `%N` timestamps; stock macOS tools do not provide them).
- `az` logged in to the target subscription; `ssh`, `ssh-keygen`, `ssh-keyscan`, `nc`, `jq`, `curl`, `timeout`.
- The subscription-side reaper, created once with
  `scripts/azure-workspace-spike/setup-spike-reaper.sh --subscription <uuid>` (Automation
  account `horizon-spike-reaper` in `horizon-worker-registry` by default; override with
  `--reaper-resource-group` and `--reaper-account`). It needs `Microsoft.Automation`
  registered and rights to assign the power-only role at subscription scope.
- `Microsoft.DevTestLab` registered in the subscription (read-only check; the harness never
  registers providers), because the backstop auto-shutdown is a DevTestLab schedule.
- A worker image published by digest to an Azure Container Registry in the same
  subscription. The image build is documented in
  [`containers/remote-worker/README.md`](../../containers/remote-worker/README.md).
- A user-assigned managed identity with `AcrPull` on that registry (pull-only; no
  registry password or admin user). The harness assigns it to each sample VM.

## Usage

```bash
scripts/azure-workspace-spike/run-vm-spike.sh \
  --subscription <subscription-uuid> \
  --image <registry>.azurecr.io/horizon-remote-worker@sha256:<digest> \
  --puller-identity-id /subscriptions/<subscription-uuid>/resourceGroups/<rg>/providers/Microsoft.ManagedIdentity/userAssignedIdentities/<name> \
  --journal-dir /private/path/spike-journal \
  --preflight-report /private/path/vm-northeurope.json \
  --sample 1 [--region northeurope] [--vm-size Standard_D4s_v3] [--max-minutes 120] \
  [--allow-ssh-from <ipv4-cidr>] [--reaper-resource-group <rg>] [--reaper-account <name>] \
  [--prove-reaper] [--dry-run] [--keep]
```

`--allow-ssh-from` must be a specific IPv4 network (prefix `/8` or longer); `*`,
service tags and the whole internet are rejected before anything is created.

`--preflight-report` is the JSON written by `scripts/azure-workspace-preflight/preflight.py
--candidate vm --live --report ...` for the same region and VM size; the harness
refuses to create anything unless that report is a live `vm` report with
`no_blockers_observed`, schema version 1, a `subscription_digest` matching
`--subscription`, and an `observed_at` at most 24 hours old, and it journals the report path, `observed_at`, tool version
and its own git commit (plus whether the harness directory had uncommitted changes) in
the `start` event. `--dry-run` renders the cloud-init and
exits without creating anything (the report is then optional). `--keep` leaves the
sample resource group for manual inspection; delete it yourself, and expect exit `6`
because the deletion gate was skipped. `--sample` accepts
1 to 99 and only tags and names the journal.

## What one sample does

1. Snapshots the subscription resource inventory (`az resource list`), used at the end to
   prove #474's criterion: nothing remains under the sample group and no pre-existing
   resource disappeared. Every mutating call names the sample group, so a disappearance
   elsewhere is almost certainly another actor in a shared subscription, but the
   inventory cannot attribute it; the proof is then reported as unverified (exit `6`,
   `inventory_proof.reason`) and the sample must be rerun. Additions elsewhere are counted.
2. Generates a fresh Ed25519 client key used only for this sample.
3. Verifies the random group name is absent, creates the resource group, then submits
   **one server-side deployment** containing the network security group (port 2222 from
   the caller's egress address only), virtual network, Standard static public IP, NIC,
   the Linux VM (`Canonical:ubuntu-24_04-lts:server`, key-only SSH, the pull identity
   assigned, a 32 GiB data disk) and the DevTestLab auto-shutdown schedule set one
   minute after the lifetime deadline. The VM is tagged `purpose=horizon-azure-vm-spike`
   and `deadline=<UTC>` in that same call. Before the deployment the power-only role
   above is granted on the group. The VM's cloud-init installs Docker (package module),
   arms the self-deallocate timer as the first `runcmd` step, then formats and mounts
   the data disk as ext4 at `/mnt/horizon-workspace`; exchanges the identity's
   IMDS token for a registry refresh token and logs in with the documented
   `00000000-0000-0000-0000-000000000000` user; pulls the image by digest; creates and
   starts the worker container with the data disk bound at `/workspace` and the client
   public key in `HORIZON_SSH_PUBLIC_KEY`, publishing container port 22 on VM port 2222.
4. (Port 2222 is opened by the deployment's security rule, from the caller's egress
   address or `--allow-ssh-from`; nothing is opened afterwards.)
5. Waits for the endpoint, reads the worker's Ed25519 host key **out of band** through
   ARM-authenticated `az vm run-command` from the retained
   `/workspace/.horizon-worker/ssh` directory, pins it, and verifies key-only SSH with
   `StrictHostKeyChecking=yes`.
6. Records `/proc/fs/ext4/<device>/options` and the mount line for the worker's
   `/workspace`, applies a shell mirror of the qualifier's rules for the journal, and
   runs the **authoritative** qualifier: `horizon-repository setup-status` against a
   fresh 0700 retained root on the data disk (expected `status: absent`) and, as a
   negative control, against a root on the container's overlay filesystem (expected
   `status: error`). The storage gate passes only on the authoritative result.
7. Starts a tmux heartbeat, disconnects for 20 s, reconnects and checks progress.
8. Writes a marker, `az vm deallocate`, `az vm start`, re-reads the host key with
   `ssh-keyscan` and compares it with the pinned key before reconnecting, then checks
   the marker, the public IP, that the data disk is mounted in the container, and
   whether the tmux session survived (it does not: the container restarts; that is
   recorded, not hidden). A failed restart is journaled and guest diagnostics are
   captured out of band; the phase is bounded to 10 minutes. It then fires the
   guest's self-deallocate service once and requires the VM to reach
   `PowerState/deallocated` (gate `guest_lifetime_bound`).
9. Deletes the resource group (`--no-wait` plus `group wait --deleted`), confirms
   absence and evaluates the inventory proof above. Cleanup runs from an `EXIT` trap, so any
   failure after creation still deletes the sample.

Exit codes: `0` every gate held; `1` a setup or provider failure before the gates (for
example the reaper missing or `Microsoft.DevTestLab` not registered; cleanup still runs); `3` usage; `4` lifetime bound reached; `5` the worker
never published a host key; `6` deletion not proven; `7` a functional gate failed
(storage qualifier, storage negative control, detach independence, deallocate, retention,
guest lifetime bound, and reaper bound when `--prove-reaper` is given)
with evidence journaled; `130` interrupted (cleanup still runs). Any other status is
normalized to `1`. Any failure after the active-phase bound expired reports `4`, and an
unproven deletion always wins: exit `6` replaces any other code. Every blocking `az` and
`ssh` call runs under `timeout` with the remaining bound.

## Journal

Everything lands under `--journal-dir/sample-<n>-<run-id>-<token>/` with mode 0700: the
private client key, `cloud-init.yaml`, `deployment.json`, `deployment-result.json`, `run-command-hostkey.txt`, `qualifier-output.txt`,
`guest-timing.jsonl` (boot, deadline timer armed, bootstrap, disk, registry login, pull,
container create and start stamps from inside the VM), `self-deallocate.txt`, `storage-evidence.txt`, heartbeat counts and
`journal.jsonl` with every controller-side event and millisecond offsets from `T0`
(the resource-group create call). Journals contain the public IP and subscription
scoped resource ids; keep them private and quote only redacted summaries.

## What a passing sample does not prove

A sample proves the measured path for that VM at that time. It does not prove
regional capacity, that a different VM size or region behaves the same, that the
worker's own sessions survive a Stop (they do not; only data does), PC-off task
progress over hours, or the Horizon adapter's own create-once and reconcile
behaviour. Those remain the acceptance items in #474.
