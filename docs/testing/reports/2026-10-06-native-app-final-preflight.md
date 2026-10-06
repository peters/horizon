---
procedure: native-app-automate
candidate_commit: fead1545547f6af48716ab6e34fa5a6df85d3be4
candidate_sha256: 100de1b2b927be7d79880179b4ad52d5007e6b0da9d6818547fbdce08e8ab7db
date: 2026-10-06
lanes: [ios-phone-current, ios-phone-older, ios-tablet, android-phone]
issue: https://github.com/peters/horizon/issues/1255
---

# Final native app preflight, 6 October 2026

## 1. Summary

The required local checks and packaged build passed on the candidate above.
The packaged GUI passed a live startup check through the public Device panel.
The packaged MCP interface exposed all 13 tools and refused the recipe-only screenshot action with its typed error.
These results do not qualify the fresh paid device matrix.

## 2. Results

| Task | Result | Evidence |
|---|---|---|
| Local validation | pass | Formatting, maintainability, workspace tests, speech tests, blocking Clippy and strict Clippy passed. |
| Packaged build | pass | The frozen executable hash matches the candidate metadata. |
| Packaged GUI startup | pass | The actual child executable matched the frozen hash. Three displayed-frame inspections advanced through 73, 81 and 89. |
| MCP preflight | pass | Initialization exposed 13 tools. The screenshot action returned `app_screenshot_requires_capture`. Normal parent EOF produced exit code 0. |
| Native catalog | pass | The current account offered 105 devices, allowed two lanes, and had zero active or queued sessions. |
| NATIVE-MATRIX | hold | The fresh four-device run needs the declared app artifacts. No paid device was allocated. |
| NATIVE-CANCEL, NATIVE-EOF, NATIVE-CRASH | hold | These checks need real app sessions. The empty MCP preflight does not qualify their cleanup behavior. |
| NATIVE-REMOTE-BUILD | hold | The source and client configuration transfer needs the pending operator approval. |

The three GUI inspections were more than two seconds apart.
The isolated terminal heartbeat changed throughout the observation window.
The candidate did not change the GUI renderer.

## 3. Defects and limits

The advisory pedantic tier returned 101 for unchanged cloud code.
This report does not mark that tier as passed.
The historical matrix report records earlier candidates and its own live-view limits.

Automatic approval review refused the source and client configuration transfer to the existing Mac build host.
The operator's answer remains pending.
The existing artifacts did not provide enough source evidence to replace that build.
The fresh paid matrix and its media, backend requests and cleanup remain unqualified.

## 4. Cleanup

The task closed its public Device panel and stopped all recorded GUI fixture processes.
Exact native reconciliation returned no pending operations.
The preflight made zero paid allocations.
Shared application checkouts, desktops and development services remained intact.

## 5. Evidence

Private evidence lives under `/var/tmp/horizon-1313-smoke/` on the controller.
It includes the frozen executable, source identity, public panel observations, private GUI capture and MCP receipts.
Local validation logs live under `/var/tmp/horizon-1313-final-validation/`.
The report contains no app content, client configuration, credentials or private provider references.
