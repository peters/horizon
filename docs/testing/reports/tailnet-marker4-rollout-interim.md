---
procedure: tailnet-marker4-rollout
candidate_commit: 11463f5a83aa578afbc8bda09577c9e2497426d2
candidate_sha256: 0bde8b13bb9660fa96a31eedb5328a5f83dad9faf57f1514fa93aaee15cefbe4
candidate_kind: frozen cloud_deploy executable
image_source_commit: 64dea7dbcfc476b882e9db12b988b81b4a82ea05
image_digest: sha256:7d337851edd33a790e01ed560f43dd61d83847552178f5a8540113ea3f07a07c
report_stage: first and final executable qualification
date: 2026-10-14
lanes: [published CPU image, local private storage, synthetic GitHub chain, Hetzner CPU None, RunPod storage diagnostic]
issue: https://github.com/peters/horizon/issues/1335
---

# Marker-4 CPU image interim report, 2026-10-14

## 1. Summary

The main-published CPU image passed local image, isolation and synthetic
GitHub-chain checks. The first-head Hetzner CPU run reached Ready with **None**,
passed stop/resume checks and removed its owned provider resources. A fresh run
on the final executable candidate passed the same lifecycle and cleanup checks.
The tested RunPod storage did not enforce private modes; that runtime remained
unsupported. Live tagged enrollment, UI and GPU work were not qualified.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| I01.1–I01.3 publication/image probes | pass | Main workflow [38513468656](https://github.com/peters/horizon/actions/runs/38513468656) published source `64dea7db`; anonymous immutable pull, source label, full 18-marker report, marker check and all-layer host-key check passed. | — |
| I01.4 Rust/image integration tests | pass | All 16 focused quick-start behaviors and the ignored Docker integration test passed. The selected-tailnet path reached Provision without local Docker and canceled before any provider request; the image returned the exact ordered report. | — |
| I02.1–I02.2 local isolation | pass | UID 10001 could not read root-owned `0700`/`0600` state. A writable mount root was refused before private state publication. | — |
| I02.3 GitHub-chain C/R | pass | C1–C8 and R1–R4 passed against exact published-source scripts. The GitHub service ran; UID 10001 could not read the root isolation marker or chain. Refresh, revocation, container recreation, token-free Git/gh callers and cloud/task grant policy passed. | — |
| P01 first-head CPU startup | pass | The frozen candidate deployed the exact pin through public quick start with a Hetzner worker choice and **None**. Ready took 107.1 seconds; GitHub service, marker 4, UID 10001 and private-state isolation passed. | — |
| P01 first-head lifecycle/cleanup | pass | Stop removed the server and retained the same volume. Resume/reconnect reached Ready in 107.2 seconds. Public delete succeeded; exact operation servers, volumes and provider SSH key were absent. | — |
| P01 current-pin rebuild | pass | The rebuild command succeeded as a no-op. A running upgrade from an older pin was not tested. | — |
| P01 final-head fresh CPU run | pass | Candidate `11463f5a` deployed the same frozen executable and exact pin with **None**. Ready took 93.7 seconds; stop retained the volume, and resume/reconnect reached Ready in 115.5 seconds. GitHub service, marker 4, UID 10001 and private-state checks passed before and after resume. Public delete succeeded; exact provider servers, volume and SSH key were absent. | — |
| P02 optional actual selected tailnet | not run | The CPU lane used **None** and enrolled no node. This optional lane remained outside the pin PR; marker and synthetic tests did not substitute for live tagged enrollment. | — |
| N01 tested RunPod persistent storage | fail | Root and child directories remained `0777`; files remained `0666`. Both `fchmod` and anchored path `chmod` returned success but did not enforce private modes. Same-inode checks passed. | [#1335](https://github.com/peters/horizon/issues/1335) |
| GPU profile | not run | The CPU base image and a keyless storage diagnostic did not qualify CUDA, GPU rendering or the declared GPU development profile. | — |

## 3. Defects

- The tested provider storage synthesized permissive modes. Root privilege and
  successful mode calls did not establish confidentiality. Isolated startup on
  that runtime remained unsupported. No mode requirement was weakened.

## 4. Deviations from the procedure

- This report records first and final executable qualification. Its prose-only
  successor reuses those results after comparing all consumed inputs; CI and
  independent review still apply to that successor head.
- The authorized provider substitution used Hetzner CPU. The original RunPod
  quick-start procedure did not pass; CPU success was not reported as RunPod success.
- The CPU run used the public CLI with **None**. It did not qualify the UI or
  actual tailnet enrollment. Rebuild on the current pin was a no-op.
- Lane G with a real GitHub App was not performed; C/R used synthetic tokens.
- The RunPod diagnostic image was diagnostic only and was not the proposed pin.
- No new checkpoint protocol, SDK permission workaround or GPU qualification was added.

## 5. Cleanup

- Local image probe containers exited and were removed. They used private tmpfs
  storage, no network and no real credentials.
- The diagnostic removed its owned entries, restored the original root mode,
  kept the root path/inode unchanged and left the mount empty. The task's private
  resource ledger recorded cleanup of its provider resources.
- Both CPU runs deleted their exact provider server/volume/SSH-key resources;
  provider absence was confirmed. Neither created a tailnet node.
- The synthetic C/R run removed its owned container, volume, overlay image and
  fixture SSH key pair. Cleanup was verified against the recorded Docker daemon.

## 6. Evidence

Private evidence contained publication/job logs, exact image reports, local probes,
C/R receipts and first/final CPU lifecycle/provider-absence receipts. The full
workspace and speech test runs and both required Clippy tiers passed on the
recorded candidate; formatting, maintainability and skill checks had matching
inputs. Advisory pedantic findings remained in two unchanged definitions.

C/R used the image and scripts of published commit `64dea7db`. Their hashes matched
the published bytes. A report-only successor could reuse this result only while
that exact image, helpers, procedure and script inputs remained unchanged.
The final executable candidate was `11463f5a`, with the same frozen CLI hash as
the first candidate. The only later change was this prose report. Rust, Cargo,
assets, helpers, image, procedures and skill inputs stayed identical; the local
matrix and first/final smoke results therefore remain valid for that successor.
Public text omitted provider IDs, endpoints, keys and private operational paths.
Exact-head CI and independent PR review remained pending.
