---
procedure: native-app-automate
candidate_commit: 51fef727e604930a7a538e1e88a6fb468d50f6e0
candidate_sha256: 885069b88b5b99a0b6b75baabd6e1e0d9afe2d416d3f78ea12d5af51df096bba
date: 2026-10-06
lanes: [ios-phone-current, ios-phone-older, ios-tablet, android-phone]
issue: https://github.com/peters/horizon/issues/1255
---

# Historical interim native app preflight, 6 October 2026

This dated interim report applies only to its recorded candidate. It does not qualify later commits or authorize merge.

## 1. Summary

The required local checks and packaged build passed on the candidate above using Rust 1.99, matching CI.
The packaged GUI passed a live startup check through the public Device panel.
The packaged MCP interface exposed all 13 tools and refused the recipe-only screenshot action with its typed error.
These results do not qualify the fresh paid device matrix.

## 2. Results

| Task | Result | Evidence |
|---|---|---|
| Local validation | pass | Formatting, maintainability, workspace tests, speech tests, blocking Clippy and strict Clippy passed. |
| Packaged build | pass | The frozen executable hash matches the candidate metadata. |
| Packaged GUI startup | pass | The actual child executable matched the frozen hash. Three displayed-frame inspections advanced through 44, 50 and 59. |
| MCP preflight | pass | Initialization exposed 13 tools. The screenshot action returned `app_screenshot_requires_capture`. Normal parent EOF produced exit code 0. |
| First CLI progress write failure | pass | A closed progress pipe retained `app_run_cancelled`, exited 2 in 3.45 seconds, started no build and left reconciliation empty. |
| Native catalog | pass | The current account offered 105 devices, allowed two lanes, and had zero active or queued sessions. |
| NATIVE-MATRIX | hold | The fresh four-device run needs the declared app artifacts. No paid device was allocated. |
| NATIVE-CANCEL, NATIVE-EOF, NATIVE-CRASH | hold | These checks need real app sessions. The empty MCP preflight does not qualify their cleanup behavior. |
| NATIVE-REMOTE-BUILD | hold | The source and client configuration transfer needs the pending operator approval. |

The three GUI inspections were more than two seconds apart.
The isolated terminal heartbeat changed throughout the observation window.
The candidate did not change the GUI renderer.

## 3. Defects and limits

Missing or empty tunnel-port maps now fail contract validation before execution. The published schema requires the tunnel and between one and sixteen ports, matching runtime validation. Regression tests omit launch arguments so the empty-map refusal cannot depend on URL checks.

Progress writes now await a bounded, per-message acknowledgement. Regression tests verify that the first failed or stalled write cancels the run without another event.
Task retirement retains the parent and child directory descriptors and uses descriptor-relative cleanup. A deterministic replacement-after-check test preserves the replacement and refuses to retire it.

The CI toolchain deprecated the atomic `fetch_update` name. The replacement `try_update` is supported by the declared Rust 1.95 minimum.
The workspace allows the new `assert_is_empty` style preference so empty-resource assertions remain usable without `PartialEq`; the blocking and strict lint tiers pass.

The advisory pedantic tier returned 101 for unchanged cloud code.
This report does not mark that tier as passed.
The historical matrix report records earlier candidates and its own live-view limits.

Automatic approval review refused the source and client configuration transfer to the existing Mac build host.
The operator's answer was pending at this preflight. On 7 October the operator approved the transfer to the trusted build host and the temporary private app/evidence uploads to the device provider. The subsequent remote guardian EOF test and rebuilt unsigned iOS and Android artifacts passed. These later results do not turn this interim report into physical-device qualification of later host commits.
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
Local validation logs live under `/var/tmp/horizon-1313-ci-validation/`.
The report contains no app content, client configuration, credentials or private provider references.
