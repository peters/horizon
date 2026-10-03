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
and requires the selected output canvas, advancing embedded frame IDs, image
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
