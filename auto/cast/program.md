# Casting autoresearch

Use the small hypothesis → baseline → edit → measure → keep/discard loop from
[autoresearch](https://github.com/karpathy/autoresearch) and the adjacent NativeSDK
repository. This is development tooling for `horizon-cast`, not a background
service or a permission to operate a real TV.

## Start

Read `AGENTS.md`, this file, the complete `results.tsv`, and recent casting
optimization commits. Use a clean isolated worktree from current `origin/main`.
Make one bounded hypothesis per experiment. Preserve failed experiments in the
append-only log so a future agent does not repeat them.

```sh
python3 -m venv /tmp/cast-bench-venv
/tmp/cast-bench-venv/bin/pip install -r auto/cast/requirements.txt
CAST_BENCH_PYTHON=/tmp/cast-bench-venv/bin/python bash auto/cast/bench.sh --backend cpu
CAST_BENCH_PYTHON=/tmp/cast-bench-venv/bin/python bash auto/cast/bench.sh --backend gpu
```

Linux needs Python 3.12 or newer, Rust, FFmpeg with `libx264`, FFprobe, and GNU time. GPU runs additionally need usable NVENC and
`nvidia-smi`. Missing GPU support fails the GPU lane; software fallback cannot
qualify it. No CUDA toolkit is needed to compile this benchmark. No new Rust
dependency is added. Never install or change host GPU drivers for an experiment.

## Measurement contract

The benchmark compiles an optimized load generator against the actual crate,
starts an independent authenticated receiver on numeric loopback, sends only
generated grayscale frames, verifies encryption, decodes the received H.264,
and requires the selected output canvas, at least two decoded frames with strictly
advancing embedded frame IDs, image
quality and clean teardown. There is no mDNS advertisement, real-device discovery,
saved pairing access, TV interaction or UI capture.

Default coverage is 720p, 1080p and 4K, portrait and landscape. A two-second
in-process warmup precedes each fixed measurement window. Select one objective
before the campaign: `--objective cpu` (sender/encoder lifecycle CPU milliseconds
per independently decoded frame, including startup/warmup/teardown),
`memory` (peak aggregate process-tree RSS), `throughput` (wall milliseconds per
measured frame), or `gpu-memory` (observed allocation for benchmark GPU processes).
All scores are lower-is-better. Other metrics remain regression guards. GPU
allocation is sampled, not a driver-wide memory total or a peak-allocation proof.

These are encoder/queue/encrypted-transport measurements. They exclude Horizon
screen capture, source cropping, physical-TV display rate, glass-to-glass latency
and real text readability. The fixed canvas uses the production selected backend;
it does not claim GPU crop/scaling coverage. Keep the hardware acceptance gates
in #1215/#1218 separate. Never report sender counters as displayed FPS.

Every result records the actual `scaler` (`cpu` or `cuda`). Usable NVENC with CPU
scaling still qualifies the encoder-only GPU lane. Use `--scaler cuda` or
`--scaler cpu` when a campaign requires a particular path; a mismatch fails.
Mixed scaler paths cannot share an aggregate score. Match the actual scaler in
each baseline/candidate case and re-baseline after changing this measurement
contract; scores from different scaler paths are not directly comparable.

Retain raw logs, binary and source fingerprints, command parameters, independently
decoded frame totals and quality checks in the returned evidence directory.
The committed ledger starts with a header: establish and record a real baseline
on your target machine instead of copying another machine's numbers.
CI runs the benchmark's failure-injection tests in an isolated Python environment;
hardware benchmarks remain explicit local runs. Run the same tests before a
campaign with `python -m unittest discover -s auto/cast -p test_bench.py`.

## Optimize and compare

In scope: frame ownership/allocation, bounded queues, encoder input/output,
packet framing/encryption, and measured GPU use within `horizon-cast`. UI capture
work is a separate experiment and still requires the native VNC smoke workflow.
Do not alter authentication, nonce uniqueness, source isolation, per-TV leases,
requested resolutions/orientations, cancellation, or teardown to improve a score.
Do not change benchmark fixtures, input pacing, quality thresholds, encoder
quality settings, expected frame coverage or measurement code during a campaign.
Fix measurement bugs separately, version the benchmark, and re-baseline.

Use the same machine, backend, parameters and immutable benchmark for baseline
and candidate. Serialize campaigns; do not compete with CI or active GPU work.
Repeat warm runs and interleave baseline/candidate in A/B/B/A order. A shared
desktop result is exploratory until reproduced under comparable load. Choose
an improvement larger than the observed run-to-run variation and reject CPU,
memory, throughput or quality regressions outside an explicitly agreed budget.

Append each attempt to `results.tsv`: real short commit for a landed `keep`,
`0000000` for an unlanded `discard`, finite score and measured diagnostics,
reason/hypothesis, and evidence path. A baseline is measured, not inferred.
Failed/blocked runs retain logs and cannot become a numeric zero or a win.
Do not edit older rows, automate destructive resets, or merge experimental
changes without the repository's validation/review/merge gates.

## Whole-window exploratory workload

`--workload whole-window` is a separate versioned contract:
`whole-window-v1-exploratory`. It runs the actual isolated Horizon root window,
including capture, crop/scaling, encoding and encrypted loopback transport. Its
`score` is always `null`; do not append these diagnostics to the encoder-v1
numeric ledger or use them to declare an optimization win. Preserve each
experiment's `summary.json` or failure file and all raw evidence. This version
prepares the end-to-end workload for future CPU, memory and GPU research; it is
not a complete scoring contract.

Linux additionally needs the existing native smoke prerequisites: bubblewrap,
Xvfb, Openbox, x11vnc, D-Bus and xdotool and xwininfo, plus built `horizon` and `horizon-device`
binaries. `--tools PATH` accepts the existing unpacked smoke-tool directory.
There is no new Rust or Python dependency. Build the candidate in its exact
isolated worktree, then use the same source and immutable benchmark throughout
the run. Preparation freezes both executables and records the actual application
child PID, start identity and executable SHA-256, source/benchmark fingerprints,
source commit, kernel, Python and FFmpeg version. A later tooling-only commit may
have a different commit hash from the candidate; its Rust source fingerprint
must still match the qualified build. Compilation provenance needs the retained
build log in addition to the executable hash.

```sh
# This process stays running until the run phase completes; OUTPUT must be new.
CAST_BENCH_PYTHON=/tmp/cast-bench-venv/bin/python bash auto/cast/bench.sh \
  --workload whole-window prepare --horizon target/debug/horizon \
  --horizon-device target/debug/horizon-device --output /tmp/window-baseline

# After the native viewer and real UI consent steps below, in another terminal:
CAST_BENCH_PYTHON=/tmp/cast-bench-venv/bin/python bash auto/cast/bench.sh \
  --workload whole-window run --prepared /tmp/window-baseline \
  --viewer-evidence /tmp/window-baseline/viewer-observations.json \
  --seconds 10 --resolution 1080p --orientation landscape --backend cpu
```

Preparation creates a task-owned X11 desktop, private application state, synthetic
terminal pixels, and two real agent controllers in different workspaces. Unlike
encoder-v1, this workload advertises exactly one owned synthetic AirPlay receiver
with a numeric loopback endpoint, because public discovery is required. Discovery
can observe other devices but the controllers filter them before writing evidence;
no physical receiver is selected, contacted or saved. Only the synthetic receiver
is paired. Its remembered controller signatures are checked independently and
its configuration-only tail is retained as metadata until an authenticated picture
arrives, rather than fed to the decoder without a picture.

Use the public `device_panel` operation to connect the reported `vnc_address` in
the calling agent's current workspace. Retain at least three public status
responses in a JSON array as `viewer-observations.json`: they must identify the
same owned connection, span at least two seconds, show connected/received/displayed
images and advancing frame sequences, and be fresh when `run` starts. Follow the
native viewer health and recording rules in `AGENTS.md`; a receipt does not replace
watching the interactive flow. Through the explicitly scoped `horizon-device`
target, open the real Cast controls and grant **Entire Horizon** in the owning
workspace. Do not modify configuration, forge agent identity or use test-only grants.

`run` verifies the grant and cross-workspace denial, starts through public casting
MCP, checks duplicate-start exclusion, warms for two seconds and samples the fixed
requested interval. An independent device screenshot of the exact PID's root
client window is taken before measurement. Decoding verifies every received frame,
requested canvas/configuration dimensions, large grayscale guards and checker
quality, non-reversing content IDs, and root chrome outside the terminal fixture
against that reference. It separately requires decoded content to advance within
the fixed interval selected by independent receiver monotonic timestamps; duplicate
encoded frames are reported. Resource endpoints are frozen before waiting for GPU
queries, and sender-counter polling is recorded as a separate interval.
Sender throughput is not content-update rate, displayed TV FPS or latency.

When `revoke-required.json` appears, revoke the owning workspace's application
capture grant through its real Cast UI. The benchmark requires capture to stop,
restart to be denied, authenticated receiver events, exact sender/receiver frame
accounting, TEARDOWN, normal window close and owned-process cleanup. A missing gate
fails the run and retains logs/partial diagnostics; it cannot produce a pass or a
numeric zero. Cleanup targets only the task-owned desktop and processes.

CPU is the application process-tree endpoint counter over the fixed interval,
including reaped child time; RSS is sampled aggregate tree memory. Horizon-hosted fixture terminals, controllers and their public MCP children are
included; the external X/VNC fixture and resource sampler are excluded. Endpoint sampling can miss a child that
reparents away, so CPU evidence remains exploratory. GPU allocation is sampled
from NVIDIA graphics and compute records attributed to actual owned PID identities;
unavailable allocation is `null` with a reason. Software fallback fails a requested
GPU lane. `--scaler cpu|cuda` requires matching public status; older candidates
without scaler metadata report `null` and cannot qualify a requested scaler lane.
Mixed scaling includes CPU work and must not be described as a fully GPU pipeline.

Compare only identical contracts, machine/load, renderer, resolution/orientation,
backend/scaler and immutable fixtures, with interleaved warm runs. Preserve quality,
authentication and lifecycle gates. A future phone-controller workload must get its
own versioned contract after the native phone interface exists; these synthetic
whole-window diagnostics do not establish phone or physical-TV performance.
Run failure tests with `python -m unittest discover -s auto/cast -p 'test_*.py'`.
