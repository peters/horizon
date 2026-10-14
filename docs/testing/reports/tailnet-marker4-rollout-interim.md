---
procedure: tailnet-marker4-rollout
candidate_commit: 64dea7dbcfc476b882e9db12b988b81b4a82ea05
candidate_sha256: 7d337851edd33a790e01ed560f43dd61d83847552178f5a8540113ea3f07a07c
candidate_kind: published CPU image OCI digest
report_stage: interim
date: 2026-10-14
lanes: [published CPU image, local private storage, RunPod storage diagnostic]
issue: https://github.com/peters/horizon/issues/1335
---

# Marker-4 CPU image interim report, 2026-10-14

## 1. Summary

The main-published CPU image passed bounded local image and isolation probes.
The tested RunPod persistent storage did not enforce private modes, even when
descriptor and path mode operations succeeded on the same inode. This report
did not qualify a new provider deployment, selected-tailnet enrollment or GPU work.
The candidate above names the image, not a frozen desktop executable.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| I01.1–I01.3 publication/image probes | pass | Main workflow [38513468656](https://github.com/peters/horizon/actions/runs/38513468656) published source `64dea7db`; anonymous immutable pull, source label, full 18-marker report, marker check and all-layer host-key check passed. | — |
| I01.4 Rust/image integration tests | pass | All 16 focused quick-start behaviors and the ignored Docker integration test passed. The selected-tailnet path reached Provision without local Docker and canceled before any provider request; the image returned the exact ordered report. | — |
| I02.1–I02.2 local isolation | pass | UID 10001 could not read root-owned `0700`/`0600` state. A writable mount root was refused before private state publication. | — |
| I02.3 GitHub-chain source lane | not run | Real-source service and isolation qualification remains separate. | — |
| P01 exact-candidate CPU provider | not run | Earlier Hetzner CPU/None evidence used a different pin and is historical only. First/final candidate startup, lifecycle and provider cleanup remained pending. | — |
| P02 actual selected tailnet | not run | The local probes and RunPod diagnostic used no tailnet key and enrolled no node. | — |
| N01 tested RunPod persistent storage | fail | Root and child directories remained `0777`; files remained `0666`. Both `fchmod` and anchored path `chmod` returned success but did not enforce private modes. Same-inode checks passed. | [#1335](https://github.com/peters/horizon/issues/1335) |
| GPU profile | not run | The CPU base image and a keyless storage diagnostic did not qualify CUDA, GPU rendering or the declared GPU development profile. | — |

## 3. Defects

- The tested provider storage synthesized permissive modes. Root privilege and
  successful mode calls did not establish confidentiality. Isolated startup on
  that runtime remained unsupported. No mode requirement was weakened.

## 4. Deviations from the procedure

- This was an interim source/image preparation, not final M0b acceptance.
- The original RunPod quick-start procedure did not pass. A compatible CPU lane
  requires explicit provider-scoped evidence; it cannot be reported as RunPod success.
- The RunPod diagnostic image was diagnostic only and was not the proposed pin.
- No new checkpoint protocol, SDK permission workaround or GPU qualification was added.

## 5. Cleanup

- Local image probe containers exited and were removed. They used private tmpfs
  storage, no network and no real credentials.
- The diagnostic removed its owned entries, restored the original root mode,
  kept the root path/inode unchanged and left the mount empty. The task's private
  resource ledger recorded cleanup of its provider resources.
- This preparation created no paid worker or tailnet node.

## 6. Evidence

Private evidence contains the exact publication/job logs, pulled-image labels,
eight image probe logs and the bounded diagnostic receipt. Public text omits
provider IDs, endpoints, keys, private network details and operational paths.
Full matrix, first/final provider evidence, CI and independent review are pending.
