---
procedure: rent-a-cloud-on-every-provider
candidate_commit: 884e818b41bcc6e4847df60fdb5961125daf47b2
candidate_sha256: c0a0f78747dbec0dd4b38d016621d279192469018eb80b78dce8def35bd7f6ab
date: 2026-10-30
lanes: [azure-new-cloud, rent-a-cloud-on-every-provider]
issue: https://github.com/peters/horizon/issues/1774
---

# Azure completion rule test report, 2026-10-30

## 1. Summary

The run observed the four outcomes of
[ADR-1774](../../architecture/adding-a-cloud-provider.md) on one Azure cloud that
a person made. The person picked Azure in the **New cloud** heading, rented the
cloud, ran a command on the worker, and deleted the cloud. The delete proof read
the subscription.

All four outcomes passed. The run then did the mandatory cross-provider rent pass
on RunPod and on Hetzner. Both passed.

The run found one new defect, in the creation path that every provider shares:
a later choice of a profile chip replaces the provider that the person chose in
the heading. Section 3 gives it.

## 2. Results

The candidate was built from `884e818b4` and frozen as
`c0a0f78747dbec0dd4b38d016621d279192469018eb80b78dce8def35bd7f6ab`. The run
examined `/proc/<pid>/exe` of the application child and compared its digest with
the frozen file. The fixture ran on its own X display, with its own private home
and a loopback VNC server. The person watched the run in a native Device panel.

| Task ID | Result | Note | Defect |
|---|---|---|---|
| ANC-1 | pass | Azure was the provider of the profile `azure-build`. The heading listed RunPod, Hetzner and Azure after the repository path was entered. | — |
| ADR pick | pass | A click on the **Azure** entry moved the cloud to Azure. The summary read Azure · `azure-build` · `northeurope` · **VM size** `Standard_D4s_v3` · USD 0.21/h. The `azure` settings section decided the size and the region. | — |
| ADR rent | pass | **Start cloud** made the group `horizon-ws-5485aecf-…` and the VM `worker`. The card reached **Ready** in 3 m 31 s. Validate took 1 m 07 s, build and push were skipped, provision 35 s, readiness 1 m 41 s, register 6.0 s. | — |
| ADR run | pass | The SSH probe answered on port 22. The marker `h1774-ui-smoke` came back from `/workspace/checkout/marker.txt`. The worker named `5e7115788d35` and `Ubuntu 24.04.5 LTS`. `findmnt` named `/workspace` on `/dev/sdc[/workspace]` (ext4). `/etc/horizon-worker` held `agent-versions.json` and `capabilities.json`. The repository arrived in `/workspace/checkout`. A Claude Code panel ran on the worker. | — |
| ADR delete | pass | The card read **Worker deleted** and **Workspace storage cleaned up** in 3 m 03 s. | — |
| Delete proof | pass | `az group exists` was `false`. The group list held no `horizon-ws-*` group. The `issue=1774` tag sweep was empty. A name sweep for the cloud ID was empty. The subscription held 389 resources and none of them was from this run. | — |
| RENT-RP | pass | The pod `4de78455zae0d3` reached **Ready** in 1 m 54 s. The marker passed. `findmnt` named a network volume on `ap-jp-1.runpod.net`. `docker images` printed `docker: command not found`. The delete took 0 m 07 s. The pod answered HTTP 404 and no volume carried the cloud ID. | — |
| RENT-HZ | pass | The server `173542857` on `cx33` in `fsn1` reached **Ready** in 1 m 53 s. The marker passed. `findmnt` named `/workspace` on `/dev/sdb[/workspace]` (ext4). The delete took 0 m 10 s. The project then held no server and no volume of this run. | — |
| Cross-provider order | pass | One provider ran at a time. The run deleted each cloud and proved its absence before the next provider started. | — |

## 3. Defects

- The heading choice does not survive a later profile choice. The run picked
  **RunPod** in the heading, then selected the `runpod-cpu` profile chip. The
  placement changed to the cheapest offer across all providers, which was a
  Hetzner `cx43`, and the card then read `Hetzner · runpod-cpu`. The provision
  request went to Hetzner. This is the reason the Hetzner attempt in Section 4
  happened before the RunPod pass.
- The account limit of the Hetzner project is unchanged. A request for 8
  shared-core vCPU failed in 3.8 s with `Provider rejected the request or
  capacity is unavailable`. The 4 vCPU shape `cx33` succeeded. The
  2026-10-25 finding 1 stays open as an account condition, not a defect.

## 4. Deviations from the procedure

- The first attempt stopped at Validate with
  `Cloud state I/O failed: Read-only file system (os error 30) at path
  "/var/tmp/.tmpqDEOK"`. The operator shell exported `TMPDIR=/var/tmp`, and the
  fixture leaves `/var/tmp` read-only. The run started the fixture again with
  `TMPDIR` unset. No cloud resource was made before the stop. This is a
  condition of the host and not a product defect.
- The fixture has no Docker socket. The run started one rootless Docker daemon
  for the lane, whose socket was the private runtime folder of the fixture, and
  it pulled the worker image one time (8470 MB). The later two lanes used the
  same image from the same data folder and did not pull it again.
- The run used one fixture home for the three lanes. The procedure asks for one
  private home for each provider. The run deleted each cloud and proved its
  absence before the next lane, so no lane changed the settings of another.
- The first Hetzner attempt used the `azure-build` profile at 4 vCPU. That
  profile requires 4 vCPU and 16 GB, and Hetzner offered `cx43` for it, which the
  account refused. The run then set the 2 vCPU profile `hetzner-cpu` as the
  default profile of the test repository. The dialog then placed `cx33`.
- The recording of the Azure lane has a gap of about 3.5 minutes between the two
  segments, across the ready state. The report GIF holds the recorded flow and
  puts the ready state and the deleted state in it as still frames.

## 5. Cleanup

- Azure: the resource group `horizon-ws-5485aecf-eee5-4bdd-8cf9-f9a94bb00de1`
  was deleted through the card. The group was absent 160 s later. The VM, its
  disks, its address, its NIC and its NSG were in that group.
- RunPod: the pod `4de78455zae0d3` and its network volume were deleted through
  the card. The pod answered HTTP 404.
- Hetzner: the server `173542857` and its volume were deleted through the card.
  The project held no server afterwards.
- Kept, and not from this run: 2 Hetzner volumes with 4 SSH keys, made between
  2026-10-10 and 2026-10-28, and 6 RunPod network volumes. The run did not
  delete them because no run of this task made them. They bill their storage
  until someone deletes them.
- The Docker daemon of the lane, its data folder and the two fixture homes were
  stopped and removed. The Docker daemon of the operator was not part of the
  run.

## 6. Evidence

The private evidence folder holds the screenshots of each state and the three
recorded segments. The pull request holds the GIF of the Azure lane and the
screenshots that show the ready state and the delete proof. The evidence folder
is outside the repository and holds references only, no credential value.
