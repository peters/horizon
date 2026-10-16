---
procedure: cloud-close
candidate_commit: 1f3d69967b06423e4917352431f70c25f4f55d14
candidate_tree: 795f4cc8e8c315c507c70478c2c010af84044289
candidate_sha256: 11d3c713b01517e12219408e12cfc64220ebe07b7cc4da5f415fda7628e2a76c
date: 2026-10-15
lanes: [linux-native, unix-fifo-regression]
issue: https://github.com/peters/horizon/issues/1335
---

# Cloud close background checks, 2026-10-15

## 1. Summary

The final Linux native run passed C01–C04 and C07. The automated checks passed C06 with real FIFO reads. The candidate included the source changes in the tree above on the stated base commit. The run used synthetic records and sent no provider request.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| C01 | pass | Cancel and Escape kept the cloud and its panel. The first dialog offered Delete and Cancel. | — |
| C02 | pass | Failed deletion showed the worker and storage warning. Cancel and reopen reset the override. A retry followed by explicit override removed only the selected cloud. | — |
| C03 | pass | A never-started cloud offered Remove. Removal closed its card and panel. No deployment record was created. | — |
| C04 | pass | Failed deletion showed a storage-only warning. Cancel kept the cloud. Explicit override removed its card and panel. | — |
| C06 | pass | Real FIFO tests checked blocked reads, cancellation, session reset, stale results and worker lock custody. A blocked refresh kept the settled controls. A final removal checked the changed record and refused removal. | — |
| C07 | pass | The window became 1252 × 827 pixels before the last cloud was removed. Fit ran at this size. The open dialog and its Delete and Cancel controls were readable. Cancel, restore and Fit passed. | — |

The two original deployment records and the workspace-volume marker kept their original bytes after the explicit overrides. The never-started cloud had no deployment record after removal. Its operation lock remained on disk.

## 3. Defects

The initial periodic-refresh regression failed because the refresh withdrew the settled Remove control. The final candidate kept the control. A fresh final check still refused removal after the synthetic record acquired resources. The regression passed on the final candidate.

## 4. Deviations from the procedure

- C05 had no native run in this qualification. The automated close tests covered missing and unreadable records. This report claims no native C05 result.
- C06 used automated real FIFO tests. The native run did not block its filesystem.
- The fixture had no provider settings. Deletion failed locally so that the override flow needed no provider credential or paid resource.
- An observer used a separate workspace and the ordinary MCP transport. Its workspace-scoped list did not prove a global cloud-count transition.
- The first native run used an earlier source tree. Its small-window check occurred after all clouds were removed. The final run checked the small dialog before the last removal.

## 5. Cleanup

- The candidate window closed normally. Its process exited with code 0.
- The owned services, collectors and child processes stopped. The control target expired.
- Both recordings finalized and were copied before the owned Device viewer closed.
- The owned relay stopped and its original process exited with code 0.
- The exact fixture container was removed. A separate check against the same healthy Docker daemon confirmed its absence.
- The private records, logs and recordings were kept as evidence. No real cloud or tailnet resource was created.

## 6. Evidence

The private evidence set was named `cloud-removal-async-final-main1f3d-795f-8abf`. It contained the frozen-product manifest, process hashes, live viewer observations, input receipts, screenshots, record hashes, cleanup receipts and both finalized recordings. The actual application and its ordinary MCP child matched the candidate hash above.

The PR GIF showed both recordings in order at four times normal speed. It used a two-pass palette at 6 frames per second and 1000-pixel width. Its 271 decoded frames lasted 45.17 seconds. The file had 5,265,034 bytes and SHA-256 `13864ac5e70efcb5badd0754ec50c93a849470698c07044d5237414507ec4659`. Representative decoded frames and the final frame were inspected. The content was synthetic.
