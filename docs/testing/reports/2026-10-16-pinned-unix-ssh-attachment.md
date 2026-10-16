---
procedure: pinned-unix-ssh-attachment
candidate_commit: 1d12f7ebf691487c19754d7570509ccffd10e7aa
candidate_sha256: 105faeda2088d3852d81dd88f01595c2802f78c99308e8d489ca6fa639ae0a4c
date: 2026-10-16
lanes: [linux-default, linux-speech, linux-private-attachment, linux-real-tailnet, linux-real-lan]
issue: https://github.com/peters/horizon/issues/1335
---

# Pinned Unix SSH attachment test report, 2026-10-16

## 1. Summary

The default-off attachment passed its local checks and actual tailnet and LAN tasks on one original Hetzner CPU worker. The frozen Core executable used the original pinned SSH connection and returned actual peer bytes after request EOF. Wrong-port and wrong-destination requests failed, and exact close retired each connection. This qualified the Linux transport component; it did not qualify native owner approval, durable admission, public controls or complete M1 sharing.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| A01 | pass | Private Unix ingress, stale-path isolation, source revocation, bounded child input and relay EOF checks passed. | — |
| A01 | pass | A child-process regression passed under umasks 022, 000 and 077. The old production implementation failed the same regression under 022 before the fix. | #1335 (fixed) |
| A02 | pass | Enabled Core coverage totalled 2,241 passes, including fresh host complements. Both enabled required lint tiers passed. | — |
| A02 | pass | Input-matched default coverage totalled 7,039 passes, including host complements; speech coverage totalled 2,416. Workspace required tiers and repository checks passed. | — |
| R01 | pass | One actual tailnet selector passed, with zero failures and zero ignored tests. Actual kernel identities, original worker/pins, peer bytes after EOF, both denied requests and exact close passed together. | — |
| R02 | pass | One actual LAN selector passed, with zero failures and zero ignored tests. Actual route/interface observations, original worker/pins, peer bytes after EOF, both denied requests and exact close passed together. | — |

The candidate used Rust 1.98.1, locked offline dependencies, six build jobs, disabled incremental compilation and `RUSTFLAGS="-D warnings"`. Cargo identified the frozen Core as a test executable with `cloud-workspaces`, `default`, `one-service-gateway` and `tailnet-node-cleanup` features. The normal deployment CLI had SHA-256 `570514c3a3d6310b632d6df9ad8f50aadf88cb3c11c9e873fe81d68330f9bfb3`.

The original A3 worker image received an explicit normal Worker overlay before either task. That ordinary Worker executable had SHA-256 `adef2d955e1e1b6d6f2e53cba6164fb780ed43bd6200c00d63d6b428db2d4f5c` and Cargo `profile.test=false`; its installed bytes matched. The worker bridge was inactive before and after each task. Separate owned kernel guests supplied the desktop source and peer. The paid worker used tailnet None.

The production permission fix and later pure-fixture adjustment changed three leaves behind `one-service-gateway`. Actual default and speech graphs excluded those leaves; their consumed Rust, assets, dependencies, flags and build configuration matched the earlier full matrix. Eighty-five frozen products were rehashed before reuse. The final graph recorded 2,325 source files, including this report. Enabled Core and its affected required tier ran again; the unchanged strict production tier remained input-matched. Both changed CI helper files passed fresh command-selection and failure-propagation checks. Both normal UI products retained all four complete bundled cloud guidance files.

The enabled pedantic advisory retained six unchanged Core findings; the workspace advisory retained existing dependency findings. Neither required tier needed a suppression. Ignored live fixtures in offline coverage were not counted as actual tests.

## 3. Defects

The old private-directory creation depended on the inherited umask because tempfile's default requested mode was 0777. A controlled red regression reproduced refusal under 022. The fix requested 0700 at creation; the same regression then passed under 022, 000 and 077. Both successful actual guests recorded umask 0022. Three pure custody-test directories were also changed to request mode 0700 explicitly; their strict parent checks then passed under the host umask 0002. No permission guard was weakened.

An earlier actual candidate had returned `StartFailed` before attachment or bytes. Its failure and cleanup evidence were retained. That earlier guest's umask was not measured, so this report did not claim a measured cause for that run. Three preceding provider attempts ended before Ready and did not execute either transport task.

## 4. Deviations from the procedure

Actual private attachment ran on Linux only. Non-Unix private attachment remained unsupported. macOS private attachment was not qualified. The documented trusted-parent policy refuses a symlink at `/tmp`. The actual private transport results in this report apply to Linux; enabled portable-engine checks on macOS and Windows are separate CI evidence.

The first successful source, `c3d82353`, passed R01 and R02 with frozen Core `38476fc9`. A later change made three pure custody-test directories explicitly private and added the enabled CI command; production transport code stayed unchanged. Final source `1d12f7eb` then repeated both actual tasks with frozen Core `105faeda`. Each run retained its own source, executable, actual inputs, generated attempts and cleanup proof.

Final report content and its index were committed after transport qualification. They did not change Rust, Cargo, embedded assets, build scripts, feature flags or runtime fixtures. Applicable documentation and Git-aware readiness checks were repeated for the report commit.

## 5. Cleanup

The final normal deployment started at 12:11:17 UTC. R01 passed at 12:16:12 UTC and R02 at 12:16:16 UTC. Cleanup completed normal worker deletion at 12:17:16 UTC, about 359 seconds after deployment started, before the original twenty-minute cleanup deadline. The earlier successful run also completed all owned cleanup after about 400 seconds.

Direct provider reads returned 404 for all five original created resources: server, volume, SSH key and two primary IPs. Seven complete paginated inventories matched the original baseline. Independent reads confirmed the same result. Both owned guests and their network were absent. The task Auth key was revoked before the two captured test nodes were deleted by exact recorded ID; confirmed absence followed. The private saved binding was removed, its catalog was empty, and both original protected devices remained. No original resource or tailnet policy was changed.

The run used the approved CPX22 in fsn1, with two CPU cores, 4 GB RAM and a 10 GB volume, within its USD 3 cap. It used no GPU. No task-owned paid resource remained.

## 6. Evidence

Private evidence retained the original plan, complete resource binding, deployment and provider journals, source and compiler graphs, exact commands and frozen products, actual root-owned input and generated attempt exports, peer EOF/20-byte receipts, original Store/settings/pins/key metadata comparisons and owned cleanup observations. Secret key bytes were neither read nor hashed for fixture proof.

Final R01 stdout had SHA-256 `cb0bf78b1bcf814e7be4d5da103efcae24ce1046e75028ad16ad229e47555f22`. Final R02 stdout had SHA-256 `6f6ecb256762a22c3dadc76f4a1c72f1cec06be12bc627d2fccc9c82cfc5e290`.

See the [procedure](../procedures/pinned-unix-ssh-attachment.md) and [trust boundary](../../architecture/pinned-unix-ssh-attachment.md).
