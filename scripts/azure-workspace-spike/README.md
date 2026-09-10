# Azure Linux VM workspace spike

Bounded, journaled, self-cleaning spike harness for the Azure CPU lane of
[#474](https://github.com/peters/horizon/issues/474). It creates **one exact
task-owned resource group per sample**, measures the timings #474 asks for, checks
the on-worker storage and retention behaviour, deletes the sample and proves the
subscription inventory is unchanged. Run
[`scripts/azure-workspace-preflight`](../azure-workspace-preflight/README.md) first.

Paid creation is bounded by `--max-minutes` (default 120); when the bound is
reached the harness deletes the sample and exits 4. It never registers a provider,
never publishes an image and touches nothing outside the sample resource group.

## Prerequisites

- `az` logged in to the target subscription; `ssh`, `ssh-keygen`, `ssh-keyscan`, `nc`, `jq`, `curl`.
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
  --sample 1 [--region northeurope] [--vm-size Standard_D4s_v3] [--max-minutes 120] \
  [--allow-ssh-from <cidr>] [--dry-run] [--keep]
```

`--dry-run` renders the cloud-init and exits without creating anything. `--keep`
leaves the sample resource group for manual inspection; delete it yourself.

## What one sample does

1. Snapshots the subscription resource inventory (`az resource list`).
2. Generates a fresh Ed25519 client key used only for this sample.
3. Creates the resource group, then a Linux VM (`Canonical:ubuntu-24_04-lts:server`,
   key-only SSH, Standard static public IP, no default NSG rules, the pull identity
   assigned, a 32 GiB data disk) with cloud-init that: formats and mounts the data
   disk as ext4 at `/mnt/horizon-workspace`; installs Docker; exchanges the identity's
   IMDS token for a registry refresh token and logs in with the documented
   `00000000-0000-0000-0000-000000000000` user; pulls the image by digest; creates and
   starts the worker container with the data disk bound at `/workspace` and the client
   public key in `HORIZON_SSH_PUBLIC_KEY`, publishing container port 22 on VM port 2222.
4. Opens VM port 2222 only from the caller's egress address (or `--allow-ssh-from`).
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
   captured out of band; the phase is bounded to 10 minutes.
9. Deletes the resource group (`--no-wait` plus `group wait --deleted`), confirms
   absence and compares the inventory. Cleanup runs from an `EXIT` trap, so any
   failure after creation still deletes the sample.

Exit codes: `0` every gate held; `3` usage; `4` lifetime bound reached; `5` the worker
never published a host key; `6` deletion not proven; `7` a functional gate failed
(storage qualifier, storage negative control, detach independence, deallocate, retention)
with evidence journaled. An unproven deletion always wins: exit `6` replaces any other code.
Every blocking `az` call runs under `timeout` with the remaining lifetime bound; cleanup has
its own fixed 25-minute bound.

## Journal

Everything lands under `--journal-dir/sample-<n>-<run-id>/` with mode 0700: the
private client key, `cloud-init.yaml`, `vm-create.json`, `run-command-hostkey.txt`, `qualifier-output.txt`,
`guest-timing.jsonl` (boot, bootstrap, disk, registry login, pull, container create
and start stamps from inside the VM), `storage-evidence.txt`, heartbeat counts and
`journal.jsonl` with every controller-side event and millisecond offsets from `T0`
(the resource-group create call). Journals contain the public IP and subscription
scoped resource ids; keep them private and quote only redacted summaries.

## What a passing sample does not prove

A sample proves the measured path for that VM at that time. It does not prove
regional capacity, that a different VM size or region behaves the same, that the
worker's own sessions survive a Stop (they do not; only data does), PC-off task
progress over hours, or the Horizon adapter's own create-once and reconcile
behaviour. Those remain the acceptance items in #474.
