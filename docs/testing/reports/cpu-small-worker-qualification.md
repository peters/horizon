---
procedure: cloud-worker-value
date: 2026-10-08
status: qualified-with-limits
lanes: [cpu]
source_base: b1d47633b7b271da26e507e36fd1730fca247f47
tested_commit: f44c42bff4fb43e2f834ef062219314c3db787ad
tested_patch_sha256: 4a1e38e5ccc112df1ea04a1905f7895dc43593e5dd1b5e3719a1ca1ebe9dbdfd
---

# CPU qualification on a 4 vCPU / 8 GB worker

## 1. Summary

The recorded CPU workload passed every mandatory tier on one 4 vCPU / 8 GB worker.
The pedantic advisory tier failed on unchanged source.
The full command returned exit code 101 because that advisory tier also stops the script.
One worker cannot establish the cheapest qualified offer across providers.
The [worker value procedure](../procedures/cloud-worker-value.md) defines a comparable cost test.

## 2. Results

The following table maps the retained evidence to the procedure's task IDs.
The procedure and this mapping were added after the worker run.
The CPU qualification passed CV3. The complete cost comparison remained blocked.

| Task ID | Result | Note | Defect |
| --- | --- | --- | --- |
| CV1 | blocked | Source, runtime and concurrency were recorded. No complete cold/warm sample plan was declared before this qualification. | — |
| CV2 | blocked | One provider's offer snapshot and allocated capacity were recorded. Comparable offers and complete billing inputs for other providers were not recorded. | — |
| CV3 | pass | The repaired worker's final warm/mixed CPU sample passed every mandatory tier. Earlier failed attempts and environment deviations were preserved. | — |
| CV4 | pass | Memory scopes, limits, swap and OOM results were recorded. The disk peak and continuous end-to-end timing were explicitly unavailable. | — |
| CV5 | pass | Provider 404 responses confirmed deletion. No task-owned resource was retained; copied access material was removed. | — |
| CV6 | blocked | No continuous allocation-to-cleanup timer or full attempt-cost calculation existed. Hourly rates remained estimates, not a completed-task cost. | — |
| CV7 | blocked | One qualified worker did not supply a comparable multi-provider cost ranking or a repeatability result. | — |

The blocked tasks limit the value comparison. They do not change the mandatory CPU check results below.
The startup and fixture defects are described in §5.

## 3. Candidate and worker

| Field | Recorded value |
| --- | --- |
| Tested source commit | `f44c42bff4fb43e2f834ef062219314c3db787ad` |
| Source base | `b1d47633b7b271da26e507e36fd1730fca247f47` |
| Tested five-file patch SHA-256 | `4a1e38e5ccc112df1ea04a1905f7895dc43593e5dd1b5e3719a1ca1ebe9dbdfd` |
| Patch scope | CPU minimum 4 vCPU / 8 GB; two default build jobs; related documentation |
| Source transfer | 1,673 tracked files matched the local candidate by SHA-256 |
| Runtime image | `ghcr.io/peters/horizon-worker-base@sha256:94d85a34632bec1b2819aaf982791b046fbaae70151f7f670e8816be05ed3c18` |
| Provider | Hetzner |
| Actual worker | 4 vCPU, 8 GB memory, 80 GB workspace volume |
| Worker type and region | `cx33`, `hel1` |
| Tested toolchain | Rust and Cargo 1.99.0; Clippy 0.1.99; x86_64 Linux |
| Selected system prerequisites | CMake 3.28.3; NASM 2.16.01; Clang 18 |
| Build jobs and test threads | Two build jobs; one test thread |
| Worker identity | Normal worker user for the final run |
| Cache | Isolated target; warm workspace artifacts; first speech and Clippy artifacts built in the final run |

The patch hash covers the five-file diff against the source base before this report and procedure were added.
The runtime image differed from the repository's development image recipe.
This report qualifies only the recorded workload, runtime and tested toolchain.
Rust 1.98.1 was installed first outside the checkout.

The checkout selected `stable` through `rust-toolchain.toml`.
The actual compiler fingerprint records Rust 1.99.0.
The report and procedure were added after the run and were not copied to the worker.

## 4. Mandatory checks

The final run used the unchanged `.horizon/validate.sh cpu` command.

| Tier | Current result |
| --- | --- |
| Formatting | Passed |
| Maintainability | Passed |
| Workspace tests | Passed; 5,411 passing results and 44 pre-existing ignored tests; 23 min 10 s |
| Speech tests | Passed; 1,736 passing results; 11 min 15 s |
| Blocking Clippy | Passed; 6 min 22 s |
| Strict Clippy | Passed; 40 s |
| Pedantic Clippy | Failed; advisory; 8 s |

The test counts include repeated UI tests in the speech tier.
They are not counts of unique test cases.
The run did not add ignored tests or remove checks.
Stage times use one-second observation intervals.
The GPU lane and paid native mobile tests were outside this CPU run.
This report does not qualify protected agent execution, GPU rendering or an untested repository workload.

## 5. Defects and deviations from the procedure

The pinned public image had a startup ownership defect.
Its version check created private agent directories as root after the first ownership handoff.
The normal configuration process could not read those directories.
A read-only Rescue inspection identified the cause.

The run changed ownership only on the fresh task-owned directories and kept their modes.
The same worker then returned to its unchanged host image.
A normal coordinator reconnect passed.
This repair does not qualify an untouched cold start of that image.

The first matrix attempt put the checkout below a private worker directory.
Root lost access when the validation script removed filesystem access-override capabilities.
The second attempt used root-owned fixtures that normal worker scripts could not access.
Both failed attempts remain in the private evidence.
The second attempt used cold build caches; later attempts used warm caches.

The third attempt used the normal worker user with the passwd home directory.
The image did not create that directory.
The normal worker launcher selects the volume home directory instead.
Ten UI cloud-creation tests failed because the tailnet store could not use the passwd home directory.
The run preserved all failures.
One unchanged sibling-launch test then passed in 0.76 seconds with the declared worker home.
The fourth full attempt used that home directory without changing its ownership or copying home data.
All ten earlier UI failures passed in the full workspace and speech tiers.

The final attempt used the normal worker user.
Ownership changes covered only the isolated checkout, toolchains and build target.
During that fixture correction, the protected worker home and isolation marker stayed unchanged.

The pedantic tier stopped on `clippy::struct_excessive_bools` in `horizon-cloud/src/provider.rs:55`.
That source file was unchanged by the tested patch.
The mandatory blocking and strict tiers passed.

A separate catalog regression passed on the same worker.
Its explicit worker type remained usable after fallback preferences excluded that type.
The existing worker and volume stayed in use; the test allocated no replacement.

## 6. Measurements and cost

| Measurement | Current result |
| --- | --- |
| Setup time | Initial readiness failed after its 900 s bound; manual recovery and prerequisite installation were separate |
| Final matrix time | 41 min 42.57 s for the command; 41 min 43.12 s for the monitor |
| Final run period | 2026-10-08 14:16:01 to 14:57:44 UTC |
| Total time, including repairs and failed attempts | No continuous end-to-end timer; no comparable task-cost result |
| Maximum RSS | 4,847,176 KiB (4.62 GiB); maximum for one process in the command tree |
| Sampled worker memory estimate | Peak 4,001,337,344 bytes (3.73 GiB); `MemTotal - MemAvailable`, sampled each second |
| Physical memory | 8,127,856,640 bytes (7.57 GiB) visible to the worker |
| CPU and cgroup limits | Four CPUs; no additional cgroup CPU or memory cap |
| Cgroup memory peak | 7,694,069,760 bytes (7.17 GiB); includes file cache and earlier attempts |
| Swap and OOM | No swap device or swaps; cgroup OOM counters and kernel `oom_kill` stayed zero |
| Workspace free space | 46.90 GiB before the final matrix; 40.46 GiB after it |
| Workspace disk peak | Not measured; before/after usage increased by 6.44 GiB |
| Captured price snapshot | 2026-10-08 12:40:43 UTC; EUR; net prices exclude tax |
| Provider compute estimate | EUR 0.0136/hour; monthly cap EUR 8.49 |
| Provider one-hour total estimate | EUR 0.02066849, including 80 GB volume and public IPv4 |
| Provisioned volume rate estimate | EUR 4.576/month for the 80 GB volume; excludes compute and IP |
| Final retained task-owned resources | None; provider deletion confirmed |
| Actual compute, storage, IP, idle and retry cost | No provider invoice collected |
| Total cost per successful task | Not established |
| Developer wait cost | Not included |

The timestamp is the local capture time of the provider offer response.
The response did not include a price publication time.
These rates are estimates. They are not an invoice or a measured cost per completed task.
The hourly estimate adds EUR 0.0136 compute, EUR 0.0008 IPv4 and EUR 4.576/730 for storage.
Storage uses a 730-hour month for comparison. The estimate does not reproduce the provider's invoice calculation.
No measured saving or provider ranking follows from this run.
One successful attempt does not establish repeatability or a cold-cache result.

Process maximum RSS is the maximum for one process in the measured command tree.
It is not the sum of simultaneous processes.
The sampled worker memory estimate is `MemTotal - MemAvailable`.
The cgroup peak includes file cache and earlier attempts.
These measurements have different scopes.

## 7. Cleanup

The final native progress capture completed before cleanup.
The supported deletion harness removed only the recorded task-owned resources.
The provider returned HTTP 404 for the server, volume, temporary SSH key and automatic primary IP addresses.
The diagnostic SSH key returned HTTP 404 before the final run.

The native viewer closed.
The owned local forward and earlier console relay processes are absent.
The native endpoint is unavailable.
The copied credentials and temporary Rescue password response were removed.
Cleanup completed on 2026-10-08 after the final evidence capture.
No provider invoice was collected.

## 8. Evidence

Private logs, source hashes, resource records and failed-attempt evidence remain outside the repository.
This report contains no credentials, private network addresses or provider resource IDs.
