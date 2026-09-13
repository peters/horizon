# Azure workspace live acceptance: adapter run record

Permanent record of the first end-to-end run of the merged Azure worker adapter
(`crates/horizon-core/src/cloud_run/azure`) against a real subscription, for
[#474](https://github.com/peters/horizon/issues/474). The driver is the ignored
integration test `crates/horizon-core/tests/azure_live_worker.rs` (Unix only); the
live test is never part of the ordinary test matrix and runs only with the
`HORIZON_AZURE_LIVE_*` variables set, while the failure cleanup's decision tests in
the same file run in every matrix without Azure.

## What one run proves

1. **Create once.** `ensure_worker` creates the task-owned resource group and
   submits the deployment behind the creation fence; a second `ensure_worker`
   reuses it and never creates.
2. **Compute bounded without the controller, from shortly after creation.**
   Before anything is created, the driver resolves the job-schedule link of the spike
   reaper runbook for this subscription and checks that the linked schedule is
   enabled, runs every 15 minutes, is due within the next interval and does not
   expire before the worker's reaper deadline plus one interval (the same facts the
   spike harness requires, plus the expiry); it refuses to create otherwise. Once the
   worker VM exists (about 15 s after creation on the recorded runs) the driver merges
   the reaper's `purpose` and `deadline` tags onto it, keeping the adapter's identity
   tags intact. This is a driver safeguard, not an adapter property: the adapter's
   template carries no operator tags, so a controller lost between deployment
   submission and the tag leaves an untagged VM for the operator to delete by hand,
   and the driver does not verify the reaper's identity or runbook content the way
   the spike harness does.
3. **Ready only with an attested host key.** `reconcile_worker` reports `Ready`
   only when the run-command channel returns the host key the container's SSH
   server serves and the group, VM and address re-prove unchanged. The VM re-proof
   covers the identity tags and the instance identity (`vmId`): a VM deleted and
   recreated under the same name and tags while the key was read, or during a stop
   or start wait, is an identity error, never this worker.
4. **The key is the one on the wire.** A pinned SSH session (no operator
   configuration read, `StrictHostKeyChecking=yes`, the global known-hosts file
   disabled, a `known_hosts` holding only the attested key, the generated client key)
   runs a command as `root` and leaves a marker on the retained data disk.
5. **Inspect from the persisted handle only** reports `Ready` without creating.
6. **Explicit Stop** deallocates and verifies `PowerState/deallocated` within its
   deadline; inspect, a repeated Stop and `ensure_worker` all report the retained
   stop without creating.
   **Check saved Stop** (`observe_worker_stop`, the read-only observation behind the
   shared stop confirmation) is asked at every station with the persisted handle and
   the attested pin: `Pending` while the worker runs and again after the start,
   `RetainedStopped` after each stop (deallocated compute, the `worker-data` disk on
   LUN 0 with `Detach`, the saved address) and `Absent` after the delete; at each
   retained stop a pin naming another address is refused as an identity error.
7. **Delete exactly the owned group**, then prove absence through the adapter
   (`inspect_worker` returns nothing) and through `az group exists` in the exercised
   subscription; a repeated delete is `AlreadyAbsent`. A failure anywhere between the
   first ensure and the adapter's acceptance of the deletion makes the driver request
   deletion of the group on the way out, after proving through the group's identity
   tags that it carries this run's workflow and job identifiers (a same-named group
   that is not ours is left untouched and reported); the outcome of that request is
   reported. Deletion is confirmed only on that successful path; when ownership
   cannot be checked or every deletion request fails, the driver prints a warning that
   names the group and the operator must inspect and delete it by hand, because the
   VM may still be billing until then (bounded by the reaper tag once it is on).

## Run record

Fifteen runs on 2026-09-11/12 in the spike subscription, region `northeurope`, size
`Standard_D2s_v3`, 32 GiB data disk, the worker image published for the #508 spike
(digest `sha256:20cc03ef…`, built from commit 22674061), the persistent pull identity
from `docs/testing/azure-workspace-readiness.md`. Timestamps are seconds after the
driver started, which is about 0.6 s before the first `ensure_worker` call (the
reaper preflight, the client key generation and the client construction come
first). Subscription, group and address values are not recorded.

**Run 1 (adapter as merged through #530): failed at the readiness gate.** The
worker was created in 5 s, cloud-init finished at about 120 s, the container was up
and its host key readable on the VM, yet `reconcile_worker` reported `Provisioning`
for the whole 15-minute bound. Cause: ARM answers a run command with an
`Azure-AsyncOperation` URL that carries a signing certificate in its query string
(3,249 bytes on this run); the transport's 2 KB operation-URL cap refused it, and the
host-key source swallowed the error. Fixed in #533 with a regression test of the live
shape and length. The leftover group was deleted by hand.

**Run 2 (with #533): passed end to end.**

| Step | Seconds after the driver started | Note |
| --- | --- | --- |
| `ensure_worker` returned `Created` | 4.8 | group created, deployment submitted |
| reaper tag merged onto the VM | 16.6 | VM resource existed by then |
| second `ensure_worker` returned `Reused` | 16.8 | no second creation |
| `reconcile_worker` reported `Ready` | 258.2 | attested Ed25519 host key, port 2222, user `root` |
| pinned SSH command succeeded | 258.8 | marker written to `/workspace`, ran as `root` |
| `inspect_worker` from the persisted handle | 275.9 | `Ready` |
| `stop_worker` returned `Stopped` | 308.2 | deallocation verified in 32 s |
| inspect, repeated stop, ensure after stop | 309.5 | all `Stopped`, nothing created |
| `delete_worker` then absence proven | 494.8 | group gone in 185 s, adapter and `az group exists` agree |

**Run 3 (restructured driver, accidentally built from main without #533): failed
at the readiness gate** in the same way as run 1, which confirms the cause: the fix
is the only difference between a run that never becomes ready and one that does.
The leftover group was deleted by hand.

**Run 4 (restructured driver on main with #533, reaper preflight included):
passed end to end.**

| Step | Seconds after the driver started | Note |
| --- | --- | --- |
| reaper preflight | 0.6 | enabled schedule with a future run |
| `ensure_worker` returned `Created` | 6.3 | |
| reaper tag merged onto the VM | 19.5 | |
| second `ensure_worker` returned `Reused` | 19.9 | |
| `reconcile_worker` reported `Ready` | 185.5 | |
| pinned SSH command succeeded | 186.1 | |
| `inspect_worker` from the persisted handle | 203.7 | `Ready` |
| `stop_worker` returned `Stopped` | 220.2 | deallocation verified in 17 s |
| inspect, repeated stop, ensure after stop | 221.1 | all `Stopped`, nothing created |
| `delete_worker` then absence proven | 467.8 | group gone in 247 s |

**Run 5 (bounded CLI calls, settings validated before creation,
subscription-scoped absence check): passed end to end** with readiness at 246.4 s,
pinned SSH at 247.1 s, Stop verified in 32 s (296.3 s) and the group gone in 245 s
(542.7 s).

**Run 6 (additionally the global known-hosts file disabled for the pinned session,
the declared hourly cost configurable next to the size, and a failure cleanup that
requests deletion of the exact group): passed end to end** with readiness at
261.8 s, pinned SSH at 262.4 s, Stop verified in 31 s (311.2 s) and the group gone in
246 s (558.3 s).

**Run 7 (additionally the SSH step bounded like every other external step, and
every fallback deletion gated on the keep flag): passed end to end** with readiness
at 214.1 s, pinned SSH at 214.9 s, Stop verified in 17 s (248.8 s) and the group gone
in 185 s (435.0 s).

**Run 8 (additionally both output pipes of every external command drained
concurrently under a cap): passed end to end** with readiness at 257.0 s, pinned SSH
at 257.6 s, Stop verified in 16 s (291.6 s) and the group gone in 184 s (476.6 s).

**Run 9 (additionally the pipes drained to end of stream keeping a capped prefix,
and the failure cleanup armed before the first ensure and kept armed until the
adapter accepted the deletion): passed end to end**
with readiness at 289.7 s, pinned SSH at 290.5 s, Stop verified in 32 s (340.3 s) and
the group gone in 125 s (466.3 s).

The readiness poll runs every 10 s and the host-key read itself takes about 10 s
through the run-command channel, so each readiness figure carries up to about 20 s
of driver latency on top of the worker's own boot. **Run 10 (additionally the reaper preflight requires the 15-minute cadence with a
run due within the interval, and the tag-failure path leaves deletion to the
cleanup guard): passed end to end** with readiness at 251.2 s, pinned SSH at
252.0 s, Stop verified in 16 s (285.2 s) and the group gone in 245 s (531.2 s).

**Run 11 (additionally the preflight resolves the reaper runbook's job-schedule
link for this subscription and validates that linked schedule): passed end to end**
with readiness at 228.2 s, pinned SSH at 228.8 s, Stop verified in 16 s (262.6 s) and
the group gone in 245 s (509.2 s).

**Run 12 (additionally the preflight requires the linked schedule not to expire
before the worker deadline plus one interval): passed end to end** with readiness at
270.6 s, pinned SSH at 271.4 s, Stop verified in 32 s (321.2 s) and the group gone in
248 s (570.1 s).

**Run 13 (additionally one reaper deadline instant shared by the preflight and the
VM tag, waits that check their deadline before every poll and never sleep past it,
and a format-independent absence check): passed end to end** with readiness at
229.9 s, pinned SSH at 230.7 s, Stop verified in 16 s (264.3 s) and the group gone in
246 s (511.5 s).

**Run 14 (additionally external commands run in their own process group with the
drained streams received under the same deadline and the whole group killed on
expiry, the tag loop capped by its own budget, the SSH session ignoring all operator
configuration, and a non-string expiry counted as expired): passed end to end** with
readiness at 234.0 s, pinned SSH at 234.6 s, Stop verified in 32 s (283.2 s) and the
group gone in 247 s (531.0 s).

**Run 15 (the driver as committed here: additionally the failure cleanup takes an
injectable command runner and its decisions are covered by five deterministic tests
in the ordinary matrix, and the driver is gated to Unix): passed end to end** with
readiness at 264.8 s, pinned SSH at 265.6 s, Stop verified in 32 s (316.1 s) and the
group gone in 246 s (563.0 s). A passing run never exercises the failure cleanup's
decisions; those are proven by the deterministic tests (owned group deleted once
and the acceptance checked; rejected or hung deletion retried then reported;
foreign, untagged and absent groups left untouched; unverifiable ownership never
deletes; disarmed or kept guards do nothing).

The thirteen passing runs give
258 s, 186 s, 246 s, 262 s, 214 s, 257 s, 290 s, 251 s, 228 s, 271 s, 230 s, 234 s
and 265 s: the spread is Azure's (image pull and VM start). All sat above the
180-second target that applied when they were recorded, and all fit the 300-second
Azure startup target approved on 2026-09-12 (see `azure-workspace-readiness.md`);
they are evidence, not a guarantee for future runs. Deletion of the group
took 185 s, 247 s, 245 s, 246 s, 185 s, 184 s, 125 s, 245 s, 245 s, 248 s, 246 s,
247 s and 246 s; Stop 32 s, 17 s, 32 s, 31 s, 17 s, 16 s, 32 s, 16 s, 16 s, 32 s,
16 s, 32 s and 32 s.

## Stop, explicit compute start, pinned reattach (run 16, 2026-09-12)

With the optional compute-start capability (`InteractiveWorkerStartProvider`,
implemented by the Azure client) the driver adds a phase between the first stop and
the delete: `start_worker`, wait for `Ready` through the persisted handle, a pinned
SSH session with the same host key reading the marker written before the stop, a
second `start_worker` that must find the worker running, then a second stop. Run 16
passed end to end, same fixture and settings as above:

| Step | Seconds after the driver started | Note |
| --- | --- | --- |
| ready with attested host key | 260.4 | |
| pinned SSH, marker written | 261.0 | |
| stop verified | 309.7 | 32 s |
| `start_worker` returned `Started` | 390.9 | running again 80 s after the call, already observed `Ready` |
| `Ready` through the persisted handle after start | 409.0 | same address, port and attested host key as before the stop |
| pinned reattach read the marker | 409.8 | data on the retained disk survived the stop |
| second `start_worker` returned `AlreadyRunning` | 427.8 | nothing posted |
| second stop verified | 459.4 | 32 s |
| deleted and gone | 705.3 | 245 s |

Run 17, with the wait polling until its absolute deadline instead of a finite
schedule, repeated the sequence: ready at 291.4 s, stop 32 s, `Started` 80 s after
the call, `Ready` with the same endpoint 97 s after it, marker read, second start
`AlreadyRunning`, second stop 32 s, gone in 247 s.

What this proves: an explicit, authorized start of the exact worker brings back the
same identity (address and host key) and the retained data, is idempotent on a
running worker, and never allocates. What it does not prove: anything about
processes that were running before the stop, which do not survive a deallocation;
and nothing here is the saved Shell task Start, which is a separate operation.

## Check saved Stop (run 18, 2026-09-13)

With the Azure `InteractiveWorkerStopObserver` (the read-only observation behind the
shared "Check saved Stop" confirmation) the driver asks `observe_worker_stop` with the
persisted handle and the attested pin at every station of the sequence above, and at
each retained stop also with a pin naming another address. Run 18 passed end to end,
same fixture and settings as above:

| Step | Seconds after the driver started | Observation |
| --- | --- | --- |
| ready with attested host key, pinned SSH | 213.9, 214.7 | |
| check while running | 233.1 | `Pending`; nothing mutated |
| stop verified | 249.7 | 17 s |
| check after the stop | 251.7 | `RetainedStopped`; a pin with another address refused as an identity error |
| `start_worker` returned `Started`, `Ready` again | 330.9, 348.8 | 79 s and 97 s after the call, same endpoint and host key |
| check after the start | 367.4 | `Pending` |
| second stop verified | 383.8 | 16 s |
| check after the second stop | 385.7 | `RetainedStopped`; moved-address pin refused |
| deleted and gone | 631.7 | 246 s |
| check after the delete | 632.4 | `Absent` |

What this proves: on the real control plane a deallocated worker carries exactly the
retained-disk shape the observer requires (`worker-data` on LUN 0 with `Detach`, under
the worker's own group) and the saved address, so a saved Stop is confirmed only
there; running, starting and deleted workers are reported as pending or absent, never
as a retained stop; and the observation issues no mutation at any station. What it
does not prove: the shared confirmation path calling this observer for Azure, which
is wired by the lead lane.

## What this run does not prove

- Resuming in-memory work after a stop: compute start brings back the disk and the
  SSH identity (proven in run 16), not the processes that ran before the stop.
- Closing Horizon or powering off its controller while a worker runs: that must leave
  the worker running and is a separate live step that involves neither Stop nor start.
- Repository handoff on the worker (depends on the PAT setup owned in #470).
- Three panels on one worker and multi-day retention.
- Bounded compute in the first seconds after creation (see item 2 above) and an
  independent verification of the reaper's identity and runbook; both belong to the
  spike harness today.
