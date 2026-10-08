---
date: 2026-10-08
status: prepared; not run on CI
candidate_base: 5dec1ccf7d662bf2347105d2e1a403e9caa09797
lanes: [linux-speech]
---

# Temporary terminal test diagnosis

This branch starts from the exact catalog candidate that failed on CI.
It is a temporary diagnostic branch, not a product pull request or a merge candidate.
No CI dispatch or result follows from this document.

The Linux speech shard alone uses the watchdog.
The shard commands, assertions and test concurrency stay unchanged.
The watchdog captures evidence after five minutes without a completed test.
Compilation alone does not start that idle timer.
A separate deadline reserves time before the job's 35-minute limit.
Stack capture has a total budget of 60 seconds.
The evidence records each process that lacks time for a stack capture.

The watchdog records process identities through their parent chain and retains Linux pidfds.
Signals apply only to recorded identities.
The watchdog closes descriptors for dead or changed identities during each observation.
If identity records fail, the watchdog reports a failure and stops the known owned processes.
The evidence includes regular thread backtraces, child states, tool versions and CPU and memory counters.
GDB disables automatic scripts and prints no frame arguments or entry values.
The watchdog reads no process environment or command arguments.
Backtraces print no argument or local values. The watchdog takes no memory dump or core file.

The ordinary Actions log retains shard output.
The artifact contains only the diagnostic counters and thread stacks.
Raw output files do not enter the artifact.
The watchdog reports a timeout as a failure; it does not convert a failed check into success.

Synthetic fixtures test progress, quiet compilation, capture order and owned cleanup.
An unrelated process must survive the timeout fixture.
An identity change must prevent a signal.

Local reproduction on the unmodified candidate used Rust 1.98.0.
The cache pair passed, and five parallel terminal-family runs each passed 81 tests.
The full speech UI suite passed 1,734 tests with both 32 and two test threads.
Those results did not reproduce or resolve the CI hang on Rust 1.99.
