# Remote browser panel: UI smoke and performance comparison (2026-09-15)

Part of [#628](https://github.com/peters/horizon/issues/628): the cost of a
remote real-device panel to Horizon's UI, measured next to a local Chromium
panel in the same Horizon process, across idle, interaction, a hidden panel
and window resizing, with the provider's own command log as the external
count of screenshot polling. Harness:
`scripts/remote-browser-evidence/ui_perf_smoke.py` (run
`run-1789475185`, started 12:26 UTC).

Setup: Horizon from `main` at 47da2237 (the #685 squash) on the Linux host,
headless on a private Xvfb display with openbox, isolated HOME, one agent panel
that owns the browser panel, and the flow driven only through the `browser_*`
MCP tools plus `xdotool windowsize` for the resizing phase. The remote target
is the hosted grid's iPhone 16 (`ios_phone`); the comparison is the local
Chromium backend (`/snap/bin/chromium`, headless) on the same fixture
(<https://peters.github.io/horizon-mobile-fixture/>). Each panel went through
four phases of 20 s. Every second the Horizon process was sampled from
`/proc`: CPU time of the whole process, of the main thread (egui runs there),
and of the `browser-driver` thread that owns the WebDriver connection, plus
resident memory. CPU percentages are of one core over the phase.

| Phase | Remote iPhone panel: process / main thread / driver thread | Local Chromium panel: process / main thread / driver thread | Remote panel: screenshot requests at the provider |
| --- | --- | --- | --- |
| no panel (baseline, 5 s) | 11.8 % / 11.8 % / 0 % | same process | none |
| idle | 13.4 % / 12.8 % / 0.2 % | 13.7 % / 13.1 % / 0.3 % | 0 in 20 s |
| interaction (`browser_act scroll` every second) | 52.8 % / 47.5 % / 4.9 % | 52.8 % / 49.8 % / 2.7 % | 30 in 20 s (1.49 per second) |
| hidden (`browser_visibility visible=false`) | 14.6 % / 13.7 % / 0.6 % | 14.9 % / 14.0 % / 0.4 % | 4 in 20 s, all around the hide and the show |
| resizing (`xdotool windowsize` every second) | 28.0 % / 27.2 % / 0.4 % | 27.8 % / 27.0 % / 0.4 % | 0 in 20 s |

Resident memory: remote panel 344 MB idle, 383 MB after the interaction phase
(decoded PNG frames), 373 MB after resizing; local panel flat at 346 MB (its
frames arrive as JPEG screencast frames). `browser_create` took 21.7 s for the
remote target (device allocation) and 1.4 s for the local backend. Both panels
closed with `closed: true`; the provider reported the remote session `done`
after 107 s.

## What the numbers show

- **A remote panel costs the UI what a local panel costs.** In every phase the
  main-thread CPU of the remote panel is within two points of the local
  Chromium panel's, and at idle both sit within about a point of the
  no-panel baseline. Idle and hidden are the baseline plus the repaint that
  any panel adds; interaction and resizing are dominated by egui repaints,
  the same for both backends.
- **No network I/O on the UI thread.** The WebDriver connection lives on the
  `browser-driver` thread (`crates/horizon-browser/src/session.rs`, which
  spawns it; `start_remote` in `webdriver/session.rs` runs there and owns the
  HTTP client), and the samples agree: during interaction the driver thread
  carried the screenshot round trips at about 5 % while the main thread's
  share matched the local panel's, and during idle, hidden and resizing the
  driver thread stayed under one percent.
- **Screenshot polling is bounded and stops.** Classic WebDriver has no push
  frames, so Horizon polls `GET /screenshot` adaptively
  (`webdriver/session/frames.rs`: a 33 ms interval inside a 900 ms active
  window after a change, then it stops after three unchanged captures). The
  provider's log confirms the behaviour from outside: zero requests over the
  idle phase, zero over the resizing phase (the page did not change, so
  resizing the window re-fits the last frame without a capture), 30 during
  the interaction phase, where each scroll re-arms the window and the round
  trip to the device limits the rate to about 1.5 per second, and 4 around
  the hide and the show of the hidden phase. No phase approached the 33 ms
  ceiling, and nothing polled while nothing changed.
- **A hidden panel keeps its session and stops costing.** `browser_visibility
  visible=false` left the panel listed and controllable; the phase's CPU
  equals idle and the provider saw no polling while it was hidden.

## Limits

One host, one run of 20 s per phase, software-rendered X display (the
baseline's 12 % is egui repainting on Xvfb, not the browser panel). The
interaction phase drives one scroll per second, which is a light load; the
frame rate under a continuous drag is bounded by the same 33 ms interval and
the device round trip. The local comparison uses the Chromium CDP screencast,
which pushes frames rather than polling, so its "driver thread" figure is
its event loop, not a poller.

## Reproducing

```
Xvfb :99 -screen 0 1600x1000x24 &
DISPLAY=:99 openbox &
python3 scripts/remote-browser-evidence/ui_perf_smoke.py --horizon <horizon> \
  --display :99 --target ios_phone --phase-seconds 20 --out ~/horizon-628-perf
```

Requirements are those of `live_smoke.py` (netrc, Secret Service, Xvfb,
openbox) plus `xdotool`, and a run directory that is not under a hidden
directory such as `~/.cache`, because the snap-packaged Chromium cannot open a
profile there. The report is `report.json` in the run directory, with
`rpc-remote.jsonl`, `rpc-local.jsonl` and `horizon.log`.
