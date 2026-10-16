---
procedure: protected-host-ingress
candidate_commit: 4ed8738be664362e976aabefc8a4463edeb3f1c4
candidate_sha256: 933c3999a8645d311c200340bc78c49d840d46ded0194a0395d03b9caaff944f
tested_source_inventory_sha256: d6a785c517bc280bd31f2b29488c269219bca0b342cb8743e4b2b40e2fdb302c
final_source_inventory_excluding_report_sha256: a66284be89a330ebb0256f099b56296c4b3858f0d2c05d9e35c564f3ee1a61b8
date: 2026-10-16
lanes: [local-linux, actual-hetzner-ipv4]
issue: https://github.com/peters/horizon/issues/1335
---

# Protected host ingress test report, 2026-10-16

## 1. Summary

The local matrix and H03 host component passed. One new Hetzner CPU worker ran
with the explicit bootstrap flag, the normal cloud controller and its original
journal. Actual IPv4 forwarding reached the protected root listeners. The agent
could not bind any of the 64 protected ports before, during or after listener
death. The test deleted its cloud resources and preserved the original account
resources.

The header commit identified the base of the uncommitted candidate. The source
inventory identified the original H03 and first-matrix snapshot of 2,282 files
before this report and its index entry. Its hash
covered sorted file SHA-256 values, two spaces, repository-relative paths and a
newline. The candidate hash identified the frozen normal `cloud_deploy` example,
with `profile.test=false`, not a test harness or the earlier helper binary.

The final source inventory hash in the header covers 2,282 other candidate files
after the guidance and index relocations; it excludes this report to avoid a
self-referential hash. Private evidence also retains the complete 2,283-file final
snapshot, including this report. All production inputs and the H03 normal
executable remained byte-identical to the real run.

This component did not qualify production Gate, keeper or Core receipt
consumption, native M1, UI/CLI/MCP exposure operations, IPv6 source preservation,
or complete M2. The bootstrap flag stayed false by default. Existing worker
snapshots still advertised only SSH 22.

## 2. Results

| Task ID | Result | Note | Defect |
|---|---|---|---|
| H01 | pass | SDK passed 488 tests and two doc tests. Default serialization, original bootstrap, Hetzner-only opt-in and the rebuild fence passed. Protocol passed 58 enabled tests and one doc test, including the actual producer encoder contract; its default graph passed 35 tests. | — |
| H02 | pass | Eight pure Python cases passed, including actual fork/held-pipe timeout cleanup. Full local default coverage passed 7,014 tests, including 13 exact host complements, with 55 intentional ignores. Speech passed 2,416 tests, with three intentional ignores. Required blocking/strict tiers, docs, formatting, maintainability and bundled skill checks passed. | — |
| H03 | pass | Actual normal deployment, original receipt/kernel binding, published IPv4 nonce flows, private backend refusal, all-64 agent bind refusal, original child death and root rebind passed. Actual provider cleanup and preservation passed. | — |

The default graph had 63 test roots. After the profile predicate lint correction,
17 roots ran again and 46 reused byte-identical executable inputs. One of the
refreshed roots had an unchanged harness but consumed the changed normal UI
binary. All 13 host complements used the refreshed frozen binaries. All three
speech roots ran on the refreshed graph. The existing opt-in live WebAuthn
harness stayed outside the offline matrix; it was not counted as a pass. The final test-literal correction was
excluded from ordinary graphs by actual Protocol feature and dep-info evidence;
its enabled tests and required tiers ran again. The pure Python source and its
test file matched the earlier eight passing cases. Both normal UI graphs embedded
the complete current paired cloud references.

A final guidance-only relocation changed only the normal UI and UI unit
executables in each graph: 76 of 78 default artifacts and two of four speech
artifacts stayed byte-identical. The current UI unit and normal-UI home-isolation
consumer ran fresh in each graph. The other 61 default roots, all 13 host
complements and the speech font harness reused their exact qualified inputs and
executables. Both current normal UI binaries embedded the full paired references.
Workspace blocking and strict checks ran again and passed. Procedure/report index
placement and this report did not change executable inputs; repository checks
covered the final prose. The total applicable passing coverage stayed 7,014
default and 2,416 speech tests.

Checks used Rust 1.98.1, locked offline dependencies, `RUSTFLAGS=-D warnings`, six
build jobs, no incremental cache and line-table debug information. An initial
workspace build stopped under the 72 GiB shared cache guard. The retry passed
after verified obsolete task packages were removed. This was not a compiler or
product failure.

The real run used one CPX22 in fsn1, with two CPUs, 4 GB RAM and a 10 GB volume.
The normal CLI resolved the fresh repository's exact plan and selected worker
through the existing machine policy. Tailscale stayed off. No credential was
copied to the repository, no image was published, and no Ready state was copied.
The immutable worker image was
`ghcr.io/peters/horizon-worker-base@sha256:a3abdf1800d95adccdf93438208a206d2353f9a6fe69c4529faea791f73230f7`.
The embedded production script SHA-256 was
`fde770ffff61d897dc35c03cbebd88129b1ba3f92bfcd2cbc6a99e02b72bd6f0`.

The original normal endpoint supplied the pinned worker-root SSH connection.
The read-only generation receipt matched the actual journal allocation, immutable
image, PID1 start ticks, kernel network namespace and effective sysctl 1024. The
actual root process had `CAP_NET_BIND_SERVICE`. The receipt mount was read-only;
the receipt was root-owned, mode 0400 and nlink 1. Its actual publication set
contained SSH 22 and all 32 fixed TCP mappings in both listed wildcard families,
with no backend publication. Listing both families did not qualify IPv6 traffic.

Desktop IPv4 nonce flows reached external 20000→physical 600 and
external 20031→physical 631. Each actual accepted peer matched the same original
direct pinned SSH peer. Both backend ports were unreachable while the original
root backend listeners were live. The agent child had UID/GID 10001, no
supplementary groups, all five capability sets zero and no-new-privileges. All 64
bind attempts returned permission denied in each phase. The exact original root
listener child was killed and joined. Root then bound all 64 ports successfully.
The original receipt bytes and stable PID1/kernel binding matched afterward.

The quote was EUR 0.0312 net per server hour, with volume/IP charges separate.
The run used a USD 3 cap, a 20-minute cleanup deadline and a 30-minute hard bound.
Deployment started at 07:27:17 UTC. Normal cleanup returned zero at 07:31:55 UTC.
A fresh provider read confirmed absence and preservation at 07:32:42 UTC.
The run did not retrieve an invoice or establish the remaining epic budget.

## 3. Defects

No blocking or strict failure remained. Two introduced lint findings were fixed:
the bool serialization predicate used the existing standard-library predicate,
and the enabled contract test removed unnecessary raw-string delimiters.

Advisory pedantic Clippy exited 101 on unchanged Main excessive-bool findings in
provider `Description` and browser `GroupState`. The existing unknown-lint warning
also remained. A fresh UI-only advisory check reached the UI after excluding
dependency linting and reported 12 existing excessive-bool declarations; every
reported UI source matched Main exactly. No suppression was added. This was not
a full pedantic pass.

## 4. Deviations from the procedure

The host's exact sent user-data bytes and their hash were not captured. The normal
SDK generated user data after the real volume supplied its device path. The run
froze the normal executable, profile, immutable image and embedded script before
effects, then recorded actual allocation, volume, receipt and kernel observations.
No reconstructed bytes were presented as wire evidence. The host Docker CLI
version was not captured or inferred from the worker's CLI.

An initial result reporter compared all `/proc/1/stat` bytes and reported failure
after successful socket equipment. Field 39 changed from one processor number to
another. PID1, start ticks, namespace, root UID/GID, capability and sysctl stayed
unchanged, and the immutable receipt stayed byte-identical. The saved worker and
desktop commands both returned zero. The result was reconciled from those raw
observations. The failed reporter record remained; no socket task was rerun and
no product source was changed.

`SO_REUSEADDR` was used only by the disposable root test listeners and their
positive rebind control to avoid TIME_WAIT as an equipment failure. It did not
change production listeners or agent privileges.

## 5. Cleanup

- The normal controller deleted the original task server, volume and SSH key.
- Fresh same-account paginated inventories and direct original-ID 404 responses
  confirmed that the server, volume, key and both primary IP resources were absent.
- Every pre-existing server, volume, key, IP and shared network remained present.
  No new network, firewall or floating IP was created.
- Both original local image-check containers were absent from a healthy daemon
  and its exhaustive container inventory. No broad Docker cleanup was used.
- The original socket child was killed and joined. Its SSH wrapper returned zero.
- The normal deployment/cleanup wrapper and deadline watchdog terminated.
- All five local unit fixtures were absent. No paid worker remained from this run.

## 6. Evidence

Private task evidence retained current source/product inventories, the normal
compiler artifact, profile/choice/plan hashes, exact commands, original journal
and account IDs, receipt/mount/kernel observations, worker/desktop raw output,
failed and reconciled reporter records, and positive cleanup receipts. This report
omitted account names, addresses, private paths and operational identifiers.

The [procedure](../procedures/protected-host-ingress.md) and
[trust boundary](../../architecture/protected-host-ingress.md) defined this host
component. Actual protected Worker/Gate/keeper/Core consumers and final exposure
UI/CLI/MCP acceptance remained required for the epic. Independent review and
exact-head GitHub checks remained separate delivery gates.
