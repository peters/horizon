# Browser performance acceptance

Recorded for [#324](https://github.com/peters/horizon/issues/324).

Issue #324 required either:

1. a literal five-minute **public WebSocket** capture on Chromium and Firefox,
   with zero unreported loss, monotonic sequence accounting, bounded warm
   CPU/RSS, reconnect without duplicate summaries, and a 30-second rolling
   summary from browser-observed data; or
2. an **explicit approval** of the existing combined oracle as the replacement
   criterion.

This document selects **option 2**.

## Decision

The accepted performance-acceptance criterion is the **combined oracle**
already specified in [`docs/testing/browser-panel-gate.md`](../testing/browser-panel-gate.md):

| Lane | What it proves | What it is not |
| --- | --- | --- |
| G1 `websocket.html` | High-rate native WebSocket capture (4,096-frame burst), 17-frame reconnect, gap/loss/truncation/drop accounting, URL redaction, bounded NDJSON | Not a public site; not five minutes |
| G1b E24 `--observation-seconds 300` | Five-minute public-site capture on Chromium and Firefox (Linux and macOS), 30-second summaries, cursor-only `browser_network_watch`, capture health, DOM/feed match | E24 `/bors` is HTTP/JSONP, not a WebSocket; the runner reloads at each interval |
| G3 | Workload-matched CPU/RSS and frame latency on the deterministic fixture | Not a public WebSocket |

Together these lanes cover high-rate WebSocket correctness, public-site
duration and summaries, and resource bounds. They do **not** claim a
five-minute capture of a third-party public WebSocket.

## Why not a literal public WebSocket

E24, the issue's named public live-data probe, does not expose a WebSocket on
`/bors`. A substitute public stream (exchange tickers, echo servers) would be a
new external dependency: markup and feed drift would be indistinguishable from
browser regressions, and the gate already forbids treating that class of
failure as an engine bug.

The loopback fixture is the deterministic pass/fail oracle for WebSocket
frames. The E24 five-minute mode is the public-site duration and summary
oracle. That split is the intended replacement for one combined public
WebSocket.

## How to rerun

Deterministic WebSocket (G1), Chromium and Firefox:

```bash
python3 scripts/browser-smoke/run.py --backend chromium --horizon target/debug/horizon
python3 scripts/browser-smoke/run.py --backend firefox --horizon target/debug/horizon
```

The MCP contract in that runner includes `websocket.html` (4,096-frame burst
plus reconnect). Use `--allow-dirty` only while iterating.

Public five-minute summaries (G1b), Oslo market hours, clean exact-head
checkout:

```bash
python3 scripts/browser-smoke/e24_smoke.py --backend firefox --horizon target/debug/horizon \
  --observation-seconds 300 --summary-interval-seconds 30
python3 scripts/browser-smoke/e24_smoke.py --backend chromium --horizon target/debug/horizon \
  --observation-seconds 300 --summary-interval-seconds 30
```

Resource bounds (G3) stay on the deterministic fixture with a workload-matched
base build on the same host.

## Non-goals

- Publishing crates (see [`browser-packaging.md`](browser-packaging.md)).
- Treating E24 markup or feed outages as Horizon failures.
- Adding a second browser-control API beside MCP.
