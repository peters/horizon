---
procedure: protected-ingress-reader
candidate_commit: 00906c624497837ada69d6f07130855f27ea35de
candidate_sha256: 7b3889916a1664c43654c1460ac69d52781f19cf9b625af4f441edb9c39c606a
date: 2026-10-16
lanes: [linux-original-host-root-docker]
issue: https://github.com/peters/horizon/issues/1335
---

# Protected host receipt reader test report, 2026-10-16

## 1. Summary

The default-off Reader passed its pure checks, applicable local matrix and final actual-root readback on one owned Hetzner CPU worker. The frozen Reader test executable read the genuine host-produced receipt and retained live file and namespace custody. The result qualified the Reader component only. It did not qualify Gate activation, peer-source preservation, named or tailnet admission, public interfaces or complete M2 delivery.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| H01 | pass | Five pure refusal tests passed. The actual-root test remained ignored in offline runs. | — |
| H02 | pass | Current default coverage totalled 7,015 passing tests, including host complements; speech coverage totalled 2,416. Required blocking and strict tiers passed. | — |
| H02 | pass | The combined enabled Worker passed 239 tests. Both frozen normal UI builds contained all four complete current bundled guidance files. | — |
| H03 | pass | The exact ignored selector ran one actual test with zero failures and zero ignored tests on the original worker. Its allocation and all reported receipt facts matched the original provider journal and host observations. | — |
| H03 | pass | The original receipt bytes and file/directory metadata stayed unchanged across the test. | — |

The candidate used Rust 1.98.1, locked offline dependencies, six build jobs, disabled incremental compilation and `RUSTFLAGS="-D warnings"`. Cargo identified the frozen candidate as a test executable with exactly the `protected-ingress` feature. The normal deployment CLI had SHA-256 `a984716a9f35bedf6d98540a82022b48d3d965f85ee211b3ada9a5bb5393b01f`.

Fresh default tests covered the changed UI and Worker roots. Reused results covered 61 unchanged default roots, two unchanged speech roots and the unchanged Core host complements. Actual executable bytes, compiler graph features/profiles, consumed source and runtime fixture inputs matched before reuse. The unchanged Board timing test passed its single ordinary identical-executable confirmation after concurrent load ended; its initial failure remained in the private evidence.

The enabled Worker advisory check passed. The workspace advisory check retained two unchanged excessive-boolean findings and the existing unknown-lint diagnostic. The required tiers passed without suppressions.

## 3. Defects

No defect was found in the current Reader candidate.

## 4. Deviations from the procedure

None. H03 used the original normal deployment, pinned SSH endpoint, genuine fixed host receipt and frozen test executable. No startup capture, copied receipt or replacement worker supplied authority. The test executable was the Reader SUT; this run did not claim a normal Worker runtime route.

The retained report and index were added after qualification. Their input comparison kept all Rust source, Cargo files, embedded assets, build scripts, toolchain and feature inputs unchanged. The workspace build script did not consume the report, index or Git identity. Applicable repository and Git-aware readiness checks were repeated for the report commit; the matching Rust and actual-root results were retained.

## 5. Cleanup

The normal deployment started at 08:59:52 UTC. H03 passed at 09:01:59 UTC, and normal deletion started immediately afterward. Deletion exited zero at 09:02:10 UTC, about 139 seconds after deployment started.

At 09:02:12 UTC, direct provider reads returned 404 for the original server, volume, SSH key and two primary IPs. Seven complete paginated inventories matched the original baseline IDs and showed no unclassified live resource. All pre-existing resources were preserved. The run used the approved CPX22 in fsn1 with a 10 GB volume, no GPU and tailnet disabled, within the USD 3 cap.

## 6. Evidence

Private evidence retained the original deployment and provider journals, fresh offer and plan, immutable source and compiler inputs, exact command arguments, upload SHA-256/mode checks, actual-root stdout/stderr, original receipt comparisons and positive cleanup observations. The actual-root stdout SHA-256 was `37e0b4162de906bd4dc23a0a0e9b997ba0aa3a2e3d1759253c6d6a8b8bf96f28`.

See the [procedure](../procedures/protected-ingress-reader.md) and [trust boundary](../../architecture/protected-ingress-reader.md).
