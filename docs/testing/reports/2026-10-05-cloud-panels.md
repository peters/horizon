---
procedure: cloud-panels
candidate_commit: 1e0c6ebfe8a7f41be6453623541ee60454e4dae0
candidate_sha256: f0344ead0698ccaf6c1854ce8571a103f066330467018757ab8a4b4f8055e27f
date: 2026-10-05
lanes: [hetzner, runpod]
issue: https://github.com/peters/horizon/issues/1264
---

# Cloud panels test report, 2026-10-05

This report records the first run of the
[cloud panels test procedure](../procedures/cloud-panels.md). The run continues.
A result of `not run` means that the run did not do the test yet.

## 1. Summary

The fixture, the first setup tasks, the repository configuration and most price
catalog tests passed. A Hetzner cloud and a RunPod cloud deployed on the test
tailnet and reached each other. Stop and resume on Hetzner kept the volume data,
the host key and the tailnet node ID. The run did not do the other tests yet.

Three worker picker tests and the tailnet name test failed. The Claude sign-in
test failed because the device `type` action changed the typed key. A01 failed
because the Cloud settings dialog moved after it opened. Three tests that need
the PC were blocked, because the PC was not on the test tailnet.

Two retests used commit `73267151fbc8d1e9ed6433f2b3213fdf021fa8e3`, which contains
the fixes for #1292 and #1293. The SHA-256 of that frozen candidate was
`323fa5fc2496b1c84281772d4033a828e2e1c9eb29d4767eae755a36016426c1`.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| [S01](../procedures/cloud-panels/s-test-fixture.md) | pass | Debug build of `origin/main`. The evidence records the SHA-256. | — |
| [S02](../procedures/cloud-panels/s-test-fixture.md) | pass | Persistent launcher with a private home and no `--ephemeral`. | — |
| [S03](../procedures/cloud-panels/s-test-fixture.md) | pass | Three inspections showed a connected Device panel with frames that advanced. | — |
| [S04](../procedures/cloud-panels/s-test-fixture.md) | pass | The SHA-256 of the running child was the same as the frozen SHA-256. | — |
| [S05](../procedures/cloud-panels/s-test-fixture.md) | pass | A private keyring on the fixture D-Bus was unlocked with a synthetic password. | — |
| [A01](../procedures/cloud-panels/a-machine-setup.md) | fail | The dialog opened and showed all saved keys, but it moved about 1 second after it opened. #1298 fixes this. A retest is not done yet. | [#1297](https://github.com/peters/horizon/issues/1297) |
| [A02](../procedures/cloud-panels/a-machine-setup.md) | pass | Retest after #1294: **Keep saved key** was directly below the field. | — |
| [A03](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [A04](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [A05](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [A06](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [A07](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [A08](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [A09](../procedures/cloud-panels/a-machine-setup.md) | not run | — | — |
| [B01](../procedures/cloud-panels/b-repository-configuration.md) | pass | The dialog offered a Codex or Claude setup agent. | — |
| [B02](../procedures/cloud-panels/b-repository-configuration.md) | pass | The committed configuration loaded a Hetzner CPU and a RunPod CPU profile. | — |
| [B03](../procedures/cloud-panels/b-repository-configuration.md) | pass | Invalid YAML showed a clear error and cleared the profiles. | — |
| [B04](../procedures/cloud-panels/b-repository-configuration.md) | pass | Image-only mode refused build profiles with a clear message. | — |
| [B05](../procedures/cloud-panels/b-repository-configuration.md) | not run | — | — |
| [C01](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C02](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C03](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C04](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | RunPod and Hetzner showed together with the three starting points. | — |
| [C05](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | Prices showed in USD with a dated ECB rate and a run-length estimate. | — |
| [C06](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | The provider filter and **In stock only** worked. | — |
| [C07](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | Retest after #1295 with real keys: 30 samples, no change of layout. | — |
| [C08](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C09](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | The chooser listed **None** and the test tailnet. | — |
| [C10](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C11](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | **Start cloud** stayed disabled until a title was entered. | — |
| [C12](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C13](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C14](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C15](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C16](../procedures/cloud-panels/c-new-cloud-dialog.md) | fail | **In stock only** hid Hetzner rows that the provider marks as unlisted. | [#1302](https://github.com/peters/horizon/issues/1302) |
| [C17](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C18](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C19](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C20](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C21](../procedures/cloud-panels/c-new-cloud-dialog.md) | fail | The list showed RunPod rows first, then Hetzner rows. It was not in the order of the estimated total. | [#1303](https://github.com/peters/horizon/issues/1303) |
| [C22](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C23](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C24](../procedures/cloud-panels/c-new-cloud-dialog.md) | pass | The list showed each data center. Data centers that cannot hold the volume showed **Storage unavailable**. | — |
| [C25](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C26](../procedures/cloud-panels/c-new-cloud-dialog.md) | fail | A region without storage showed that no worker is in stock, not **Storage unavailable**. | [#1304](https://github.com/peters/horizon/issues/1304) |
| [C27](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C28](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C29](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C30](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | — | — |
| [C31](../procedures/cloud-panels/c-new-cloud-dialog.md) | not run | The Hetzner worker ran in the selected location. The comparison of the RunPod data center is not done yet. | — |
| [D01](../procedures/cloud-panels/d-deployment.md) | pass | A Hetzner CPU cloud on the test tailnet reached Ready in about 2 minutes. | — |
| [D02](../procedures/cloud-panels/d-deployment.md) | pass | A RunPod CPU cloud on a network volume and the test tailnet became Ready. | — |
| [D03](../procedures/cloud-panels/d-deployment.md) | not run | — | — |
| [D04](../procedures/cloud-panels/d-deployment.md) | pass | The checkout had the expected commit, no local changes and the agent user as owner. | — |
| [D05](../procedures/cloud-panels/d-deployment.md) | pass | The timeline on the card matched the stages that the run saw. | — |
| [E01](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E02](../procedures/cloud-panels/e-panels.md) | fail | The Claude panel started, but sign-in failed. The device `type` action changed the typed key. | [#1301](https://github.com/peters/horizon/issues/1301) |
| [E03](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E04](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E05](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E06](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E07](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E08](../procedures/cloud-panels/e-panels.md) | not run | — | — |
| [E09](../procedures/cloud-panels/e-panels.md) | pass | `cloud_deploy endpoint` gave the endpoint. Root SSH worked with the pinned host key. | — |
| [L01](../procedures/cloud-panels/l-lifecycle.md) | pass | The stop took 12 seconds and deleted the server. The resume took 89 seconds on a new server. The marker file stayed. | — |
| [L02](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L03](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L04](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L05](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L06](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L07](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L08](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [L09](../procedures/cloud-panels/l-lifecycle.md) | pass | The resume made a new server. The volume data and the host key stayed. Pinned SSH worked. | — |
| [L10](../procedures/cloud-panels/l-lifecycle.md) | not run | — | — |
| [T01](../procedures/cloud-panels/t-tailnets.md) | pass | A short key kept **Save tailnet** disabled. The settings JSON had no auth key. | — |
| [T02](../procedures/cloud-panels/t-tailnets.md) | not run | — | — |
| [T03](../procedures/cloud-panels/t-tailnets.md) | pass | The card showed the test tailnet and **Selected at provisioning**. | — |
| [T04](../procedures/cloud-panels/t-tailnets.md) | pass | `tailscaled` ran in userspace mode with the SOCKS5 proxy on `127.0.0.1:1055` and its state on `/workspace`. | — |
| [T05](../procedures/cloud-panels/t-tailnets.md) | blocked | The worker listed itself as online with its tailnet addresses. The PC was not on the test tailnet, so step 3 was not possible. | — |
| [T06](../procedures/cloud-panels/t-tailnets.md) | blocked | The run used a separate test tailnet. The PC was not on it. | — |
| [T07](../procedures/cloud-panels/t-tailnets.md) | blocked | The run used a separate test tailnet. The PC was not on it. | — |
| [T08](../procedures/cloud-panels/t-tailnets.md) | pass | A random value crossed the tailnet in both directions between two clouds. | — |
| [T09](../procedures/cloud-panels/t-tailnets.md) | pass | A Hetzner cloud and a RunPod cloud reached each other over the tailnet. This was the first live RunPod tailnet test. | — |
| [T10](../procedures/cloud-panels/t-tailnets.md) | fail | The node ID and the tailnet IP address stayed, but the tailnet name changed after the resume. | [#1310](https://github.com/peters/horizon/issues/1310) |
| [T11](../procedures/cloud-panels/t-tailnets.md) | not run | — | — |
| [T12](../procedures/cloud-panels/t-tailnets.md) | not run | — | — |
| [T13](../procedures/cloud-panels/t-tailnets.md) | not run | — | — |
| [T14](../procedures/cloud-panels/t-tailnets.md) | pass | The agent user reached the other cloud by tailnet name through SOCKS5 and by IP address through the HTTP proxy. | — |
| [G01](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G02](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G03](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G04](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G05](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G06](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G07](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G08](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G09](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G10](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G11](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [G12](../procedures/cloud-panels/g-companions.md) | not run | — | — |
| [O01](../procedures/cloud-panels/o-offers.md) | pass | The host MCP listed RunPod offers, Hetzner in `other_providers` and a complete USD comparison. | — |
| [O02](../procedures/cloud-panels/o-offers.md) | not run | — | — |
| [O03](../procedures/cloud-panels/o-offers.md) | not run | — | — |
| [N01](../procedures/cloud-panels/n-local-network-bridge.md) | not run | — | — |
| [N02](../procedures/cloud-panels/n-local-network-bridge.md) | not run | — | — |
| [N03](../procedures/cloud-panels/n-local-network-bridge.md) | not run | — | — |
| [N04](../procedures/cloud-panels/n-local-network-bridge.md) | not run | — | — |
| [N05](../procedures/cloud-panels/n-local-network-bridge.md) | not run | — | — |
| [X01](../procedures/cloud-panels/x-teardown.md) | not run | — | — |
| [X02](../procedures/cloud-panels/x-teardown.md) | not run | — | — |
| [X03](../procedures/cloud-panels/x-teardown.md) | not run | — | — |
| [X04](../procedures/cloud-panels/x-teardown.md) | not run | — | — |
| [X05](../procedures/cloud-panels/x-teardown.md) | not run | — | — |

## 3. Defects

- [#1292](https://github.com/peters/horizon/issues/1292): **Replace** left a large empty area above **Keep saved key**. Fixed by #1294.
- [#1293](https://github.com/peters/horizon/issues/1293): a background price refresh showed **Comparison incomplete** and moved the worker list. Fixed by #1295.
- [#1297](https://github.com/peters/horizon/issues/1297): the Cloud settings dialog moved about 1 second after it opened. Fixed by #1298.
- [#1299](https://github.com/peters/horizon/issues/1299): the New cloud dialog in one column kept a small scroll area. Fixed by #1300.
- [#1302](https://github.com/peters/horizon/issues/1302): **In stock only** hides Hetzner offers that the provider marks as unlisted.
- [#1303](https://github.com/peters/horizon/issues/1303): the worker list is not in the order of the estimated total across providers.
- [#1304](https://github.com/peters/horizon/issues/1304): a region chip says that no worker is in stock when no data center can hold the volume.
- [#1305](https://github.com/peters/horizon/issues/1305): the parser of RunPod stock and the picker search need stronger checks.
- [#1310](https://github.com/peters/horizon/issues/1310): the tailnet name of a Hetzner cloud changed after a stop and a resume.
- [#1301](https://github.com/peters/horizon/issues/1301): device `type` actions lost characters at action boundaries. This is a defect of the test tool.
- [#1296](https://github.com/peters/horizon/issues/1296): a flaky terminal test can stop a CI shard until the time limit.

## 4. Deviations from the procedure

- The procedure was written during the run. The run used the same steps, but
  not in the final text.
- A02 and C07 were done again on a later candidate that contains the fixes. A01 was not.
- The run used other cloud titles than the planned clouds of the procedure.
- In E02, a device `type` action entered the real agent key. The procedure now
  requires that the operator enters each real secret.
- T06 and T07 were not done, and T05 was done only in part, because the PC was not on the test tailnet.

## 5. Cleanup

- Not done yet. The X area records the deletion of each cloud in the resource ledger.

## 6. Evidence

The private evidence of the operator contains the screenshots, the inspections,
the hashes and the resource ledger. This file contains no secrets, provider IDs
or tailnet addresses.
