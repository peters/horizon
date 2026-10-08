---
procedure: cloud-worker-value
feature: Cost and capacity of a project worker
platforms: [linux, macos, windows]
cost: rents compute
destructive: yes
secrets: [provider credentials]
owner: peters
---

# Select a cloud worker by cost per successful task

## 1. Purpose

This procedure compares qualified workers for one declared project task.
Use the cheapest qualified worker by total cost per successful task.
An hourly price alone does not show the cost of a successful task.
One successful worker run does not establish the best worker across all providers.

The procedure applies to other repositories with their own checks and workloads.
Use each repository's instructions instead of the Horizon commands where applicable.

## 2. Applicability

This procedure applies to a fixed project workload on authorized cloud workers.
Use the project's declared platform and provider requirements.
The Linux memory counters below apply only when the worker exposes them.
This procedure does not qualify untested agent protections, GPU workloads or other repositories.

## 3. Safety

> **CAUTION:** RENT ONLY THE RESOURCES THAT THE OPERATOR AUTHORIZED. Failed starts,
> retries and retained storage can produce charges.

> **CAUTION:** KEEP CREDENTIALS AND PRIVATE IDENTIFIERS OUT OF PUBLIC EVIDENCE.
> Public evidence can expose access material.
> Keep provider resource IDs and access details in the private resource ledger.

## 4. Equipment and preconditions

- A fixed source revision, dependency lockfiles and hydrated source assets.
- An immutable image digest and declared software version requirements.
- The project's mandatory checks and a representative task workload.
- A private resource ledger and evidence directory.
- A declared budget, region requirement and maximum task duration.

## 5. Setup

1. Read the repository's instructions, worker profiles and workload requirements.

   Result: The comparison uses the project's mandatory checks and required capabilities.

2. Prepare a private resource ledger and an evidence directory for this comparison.

   Result: Each allocation attempt and measurement has a private record.

## 6. Tasks

Execute CV1 through CV7 in order. Each task has a stable ID for the report.
CV5 cleanup completes before CV6 stops the timer and calculates final cost.

### 6.1 CV1 — Define a comparable task

1. Record the exact source revision and any patch hashes.

   Result: Each worker uses the same project content.

2. Record the image digest and required toolchain and dependency versions.

   Result: Each candidate has the same declared software requirements.

   A repository override can select a different toolchain from the image's installed default.
   Examine the actual versions in CV3 before the task starts.

3. Record all mandatory checks and the representative workload.

   Result: A success means the same completed work on each worker.

4. Record the required platform, region, agent protections and device capabilities.

   Result: A low price cannot replace a required capability.

5. Record the permitted concurrency and maximum task duration.

   Result: Each result states its build jobs, test threads and active workloads.

6. Declare the number of cold and warm samples before the comparison starts.

   Result: Each worker uses the same sample plan, including the rule for failed samples.

For Horizon, use [the cloud development guide](../../../.horizon/README.md)
and `AGENTS.md` for the mandatory validation matrix and applicable native smoke.
Keep the GPU lane separate from the CPU lane.

### 6.2 CV2 — Record current offers

1. Get current offers from each configured provider.

   Result: The record includes the price timestamp, currency, worker type and region.

2. Examine the actual capacity before each allocation.

   Result: Each offer has a capacity result, not only a catalog entry.

3. Record the tax basis, currency conversion and minimum billing period.

   Result: Compared amounts use the same tax basis and conversion timestamp.

4. Record compute, storage, public IP and network charges.

   Result: The estimate includes active and stopped-resource charges where applicable.

5. Record the setup duration and expected idle period.

   Result: The estimate includes time outside the task command.

An advisory capacity result does not guarantee an allocation.
Keep a failed allocation in the comparison, with its reason and any charge.
Do not compare a current offer with an older price without an explicit date.

### 6.3 CV3 — Run the same task

1. Start elapsed-time measurement before allocation.

   Result: The measured period includes setup and every allocation attempt.

> **CAUTION:** ALLOCATE ONLY THE AUTHORIZED WORKER. Allocation, failed starts and retained resources can produce charges.

2. Allocate the authorized worker with the selected type, region and pinned image through the repository's normal provider or Horizon flow.

   Result: The allocation uses the declared comparison candidate and resource budget.

3. Record each allocated resource in the private resource ledger.

   Result: The run can identify and remove only its own resources.

4. Select the normal worker user for the task.

   Result: Tests use the same permissions and agent protections as normal work.

5. Examine the effective user, home directory, tool paths and cache permissions from the image's normal launcher.

   Result: The task uses the declared worker environment, not an absent default home directory.

6. Record the actual toolchain and dependency versions in that environment.

   Result: The comparison includes the versions selected inside the checkout as the normal worker user.

7. Use an isolated checkout and build cache for each comparison lane.

   Result: A different worker cannot inherit an undisclosed cache advantage.

8. Label the run `cold`, `warm` or `mixed` before the task starts.

   Result: The report states which image, dependency and build caches exist.

9. Start worker memory, swap and disk measurements before the task starts.

   Result: Measurements include the complete task execution.

10. Run one complete validation matrix at a time as the selected normal worker user.

   Result: Unrelated builds do not change the memory or duration measurements.

11. Record setup time, task time and elapsed time so far.
    The allocation timer stays active until CV6, after cleanup.

    Result: The interim record does not claim a cleanup-inclusive total.

12. Record memory, swap and disk use during the task.

    Result: The report includes peaks, limits and out-of-memory events.

13. Preserve every failed attempt before a retry.

    Result: The record includes the failure, elapsed time, cost and retry reason.

14. Repeat the unchanged task according to the declared cold and warm sample counts.

    Result: Cold and warm results remain separate, with every failure recorded.

15. Record the duration range and failures across all samples.

    Result: The report shows variation; one successful warm sample does not establish reliability.

If a repair changes the image or permissions, declare that change before the next run.
A repaired worker does not qualify an untouched cold start of the original image.
If the source changes, start a new comparison with the new revision.
Do not remove a mandatory check to obtain a pass.

### 6.4 CV4 — Interpret memory and task results

Process maximum RSS measures resident memory for a process.
It does not establish the simultaneous physical memory use of all build processes.
Record the measurement tool and its process scope.
Do not add unrelated process peaks and call the sum a measured worker peak.

Also record the worker's physical memory and any cgroup memory limit.
For Linux containers, record `memory.current`, `memory.peak`, `memory.events`
and `memory.stat` where available.
The cgroup total can include file cache.
Report anonymous memory and file cache separately when the measurements permit it.

Record swap use, out-of-memory kills, disk peaks and free disk space.
Include the source assets, image layers and build cache in the disk scope.
State unavailable measurements as unavailable.
Do not treat a short successful command as a full workload qualification.

Classify failures before changing the worker size.
An image defect, wrong home directory or unreadable checkout does not show that the worker needs more memory.
For a timeout, compare the test's source and result with a successful baseline run.
Preserve the first log before a bounded retry.
Report the first failure even if the retry passes.
Repeated retries until success do not establish reliability.

### 6.5 CV5 — Complete cleanup

> **CAUTION:** REMOVE ONLY THE RESOURCES IN THIS RUN'S LEDGER. Other resources can
> contain another person's work.

1. Stop the task-owned commands and close their viewers.

   Result: No task-owned command remains active.

> **CAUTION:** DELETE ONLY PROVIDER RESOURCES RECORDED BY THIS RUN. Other workloads can lose data or access.
> Follow the operator's instruction for retained resources.

2. Delete or retain each owned provider resource under the operator's instruction.

   Result: The report states each resource's final state and continued charge.

3. Examine the provider's final resource state.

   Result: The record distinguishes a deletion request from completed deletion.

> **CAUTION:** REMOVE ONLY CREDENTIAL COPIES RECORDED BY THIS RUN. Other work can lose access if shared credentials are removed.
> Keep shared credentials and credentials used by other work.

4. Remove copied private credentials after the owned worker stops.

   Result: Sanitized results remain available without private access material.

### 6.6 CV6 — Calculate comparable cost

1. Examine the completed CV5 cleanup record.

   Result: Each owned resource has a confirmed final state and a retention instruction.

2. Stop the allocation timer after CV5 completes.

   Result: The end-to-end period includes allocation, failed starts, setup, samples and cleanup.

3. Record final setup, task, cleanup and total elapsed time for that period.

   Result: Final values replace the interim CV3 timings and include cleanup.

4. Record retained-resource charges through the period's end separately from future retention estimates.

   Result: The cost inputs include confirmed resource states and a defined measurement period.

5. Calculate provider cost and cost per successful task for the completed period.

   Result: The calculation includes every attempt and uses the confirmed cleanup and retention record.

Use actual charges when they exist.
Otherwise, label the result as an estimate from timestamped provider rates.
Apply each provider's billing increments, minimum charge and monthly cap where applicable.

```text
provider cost = compute + storage + public IP + network + other provider charges
cost per successful task = total provider cost of all attempts / completed tasks
```

The total includes setup, failed attempts, retries, idle time and retained resources
within the declared measurement period.
Use the same measurement period and currency for every amount in a cost calculation.
State the shared-cost allocation rule for reused images, caches or workers.
If no task succeeds, report a failure and total cost; do not divide by zero.

Developer wait cost is an optional separate estimate:

```text
developer wait cost = measured blocked hours * declared hourly value
combined total estimate = total provider cost + total developer wait cost
combined cost per successful task = combined total estimate / completed tasks
```

Record the hourly value and who supplied it.
Do not count unattended elapsed time as blocked developer time.
Keep provider cost and developer wait cost visible as separate amounts.
Do not claim a financial return without measured benefits and a declared cost basis.

### 6.7 CV7 — Record the comparison

Copy this table into the report for the same task and revision.
Use one row per worker, cache state and concurrency setting.
Use the final CV6 measurements and calculations. Do not use interim CV3 totals.

| Field | Worker A | Worker B |
| --- | --- | --- |
| Source revision and patch hashes | Pending | Pending |
| Image digest and toolchains | Pending | Pending |
| Provider, worker type and region | Pending | Pending |
| Price timestamp, currency and tax basis | Pending | Pending |
| Capacity result and allocation timestamp | Pending | Pending |
| CPU, memory, cgroup limit and storage | Pending | Pending |
| Build jobs, test threads and active workloads | Pending | Pending |
| Cold, warm or mixed caches | Pending | Pending |
| Declared cold/warm sample counts and duration range | Pending | Pending |
| Required checks and workload result | Pending | Pending |
| Successes, failures and retry reasons | Pending | Pending |
| Setup, task and total elapsed time | Pending | Pending |
| Maximum RSS and measurement scope | Pending | Pending |
| Worker/cgroup peak, cache, swap and OOM events | Pending | Pending |
| Disk peak and free space | Pending | Pending |
| Compute, storage, IP, network and idle charges | Pending | Pending |
| Billing minimum, tax basis and shared costs | Pending | Pending |
| Total provider cost and cost per successful task | Pending | Pending |
| Optional developer wait cost | Not included | Not included |
| Optional combined total and cost per successful task | Not included | Not included |
| Cleanup result and retained-resource cost | Pending | Pending |

1. Exclude workers that fail a mandatory requirement or the task duration limit.

   Result: Only qualified results enter the cost ranking.

2. Select the lowest measured cost per successful task among comparable qualified results.

   Result: The selection states its scope, sample count and remaining uncertainty.

3. Report untested providers and unavailable capacity separately.

   Result: The report does not claim an optimum across untested offers.

## 7. Pass criteria

A complete cost comparison passes only when all tasks meet these criteria:

- CV1: The source, image, software requirements, capabilities, budget, time limit and cold/warm sample plan are recorded.
- CV2: Current offers, capacity, currencies, tax basis and applicable billing rules are recorded for each compared provider.
- CV3: Each sample marked qualified passed the mandatory checks. The report records all sample outcomes, failures, concurrency and actual environment versions.
- CV4: Measurement tools, limits, process scopes and unavailable fields are explicit. No missing measurement is reported as a measured zero.
- CV5: Every owned resource has a confirmed final state. Retained resources have an operator instruction, and shared resources and credentials remain intact.
- CV6: Final timing and cost use one declared period that includes cleanup. Every charge or estimate has a basis, and the successful-task denominator is explicit.
- CV7: The comparison uses qualified, comparable samples and states its provider coverage and uncertainty.

A worker qualification can pass CV3 without a complete cost comparison.
Record incomplete tasks as blocked and explain the missing evidence.
One successful warm sample does not establish repeatability or the cheapest provider.

## 8. Cleanup

Complete CV5 before CV6. Record a deletion request and confirmed deletion separately.
If an owned resource remains, record its state, retention instruction and ongoing charge.
Do not report a final timing or cost while an owned resource has an unknown state.

## 9. Record of results

For a retained run, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). Otherwise, put the results in the pull request.
Map CV1 through CV7 to the report's Task ID, Result, Note and Defect columns.
Use `pass`, `fail` or `blocked` in a final report. Use `not run` only in an interim report.
Keep the private resource ledger, raw logs and secret-bearing evidence outside the repository.
Include the comparison table, final cleanup states and links to sanitized supporting evidence.
