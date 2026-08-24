# Browser Panel — Rendering & CPU Auto-Research Agenda

Measured baseline (2026-08-24, this machine, real page content, JPEG q60 screencast):

| Stage | Tiled 403×252 | Fullscreen 1840×1000 |
|---|---|---|
| WebSocket + JSON parse | 0.003 ms | 0.010 ms |
| `params` clone | ~0 ms | 0.009 ms |
| base64 decode | 0.002 ms | 0.022 ms |
| JPEG decode (zune-jpeg, reused buffer) | 0.11 ms | 1.96 ms |
| **Driver total** | **~0.15 ms** | **~2.0 ms** |
| Budget at 60 fps | 16.7 ms | 16.7 ms |

Reference points: an idle page has zero frame decode/upload cost
(change-driven screencast), while low-rate manifest polling remains;
zune-jpeg is ~30% faster than system libjpeg on the same frames; texture
upload only happens on `seq` change. Items are ordered by expected CPU/energy
yield per unit of risk, not by novelty.

Method (per repo perf rules): each item ships with a workload script
(Xvfb + Xvfb `import` FPS trace or `tracing` span under
`--features trace-profiling`), the exact page(s) used (static, clock,
CSS-animation, video, scroll-storm), before/after on the same binary
profile, and the span name in the commit message.

## A. Frame pipeline (decode → texture)

### A1 — Skip near-identical frames (driver-side diff gate)  [start here]
Hypothesis: many "active" frames carry almost no visible change
(cursor blink, 1px shimmer, sub-threshold CSS pulse). Decode → compare a
downsampled (e.g. 64×36) luminance grid against the previous frame → if
the diff is below threshold, drop the frame *after* ack, before the
`Frame` event. Saves texture upload + GPU redraw for that class of frame;
cost is one downsample pass (~0.05–0.1 ms) that can reuse the decode
buffer.
Measure: on clock / CSS-animation / video pages, count
frames-sent / frames-decoded / frames-uploaded and driver CPU.
Risk: low (ack is already sent before decode; dropping is safe —
Chrome only cares about acks). Threshold needs a visual check so real
motion is never eaten (fail-open above threshold).

### A2 — Partial (dirty-rect) texture upload  [bigger win, bigger lift]
Hypothesis: for local changes (typing, one updating widget) most of the
texture is unchanged; egui's `TextureHandle::update` re-uploads the whole
panel-sized RGB8 every frame (fullscreen = 5.5 MB ≈ 330 MB/s at 60 fps).
Research: derive merged dirty rects from the A1 diff grid on the driver,
and either (a) push a custom wgpu texture update for those rects around
the egui texture, or (b) split the egui texture into a coarse tile grid
and only `update()` changed tiles.
Measure: upload bytes/frame (wgpu stats or driver-side accounting) and
UI-thread time on the same corpus.
Risk: medium — bypasses or complicates the egui texture path; verify
tile-boundary artifacts and resize/re-attach reset.

### A3 — GPU video composition (H.264 tab capture)  [the "real GPU" bet]
Stop shipping JPEG entirely: Chrome encodes the tab with its GPU encoder
(CDP `TabCapture`/WebRTC loopback), we decode with the hardware video
engine (VAAPI / VideoToolbox / NVDEC) into a wgpu texture. Zero CPU in the
frame path.
Why it's parked: it is a media pipeline (RTP demux, keyframes, jitter,
four platform decode backends, dmabuf/Metal texture import) built to save
~1.7 ms in the fullscreen worst case. Revisit only when the target
workload is 4K/fullscreen video or many simultaneous video panels.
L0 spike first: does the tab-capture loopback survive cross-document
navigation (the case that already breaks the screencast session), and
what is HW-decode + texture-import latency on this box?

### A4 — `deviceScaleFactor` < 1 "performance mode" for large panels
Chrome rasterizes fewer pixels (its own CPU+GPU raster cost drops), we
decode fewer pixels, and the board's GPU upscale is nearly free.
Research: expose a `viewport_scale` (e.g. 0.75/0.5) and measure
Chrome CPU + decode time + perceived text softness on a large panel.
Cheap to try because `Emulation.setDeviceMetricsOverride` is already sent
on viewport change; only the `deviceScaleFactor` field is new.

## B. UI thread

### B1 — One event-list scan per frame, not per panel  [implemented]
Browser events are now snapshotted once per rendered viewport when that
viewport contains a browser panel, then passed as a borrowed slice through
every browser view. Pointer and focused-keyboard handling share that slice;
there is no per-panel event-list clone. Re-measure `horizon::app::update`
with 3 browser panels under a scripted pointer-move storm (xdotool on X11)
if this path changes again.

### B2 — Texture upload off the UI thread (only if A2/A1 aren't enough)
Fullscreen 60 fps = 330 MB/s of synchronous CPU→GPU upload inside the
UI frame. If B1 + A1/A2 leave the UI thread contending, move frame
uploads to a worker owning a wgpu queue with a triple-buffered frame
handoff (the `FrameSlot` double-buffer becomes the handoff protocol).
Measure: `egui::context::pass` and `render_active_view` spans with a
fullscreen video page.

## C. Driver & process CPU

### C1 — Adaptive manifest polling
`tick_signals` reads + JSON-parses the manifest every 250 ms per panel
forever. Research: exponential backoff (250 ms → 2 s) once no owner has
been seen for N seconds (reset on any owner/handoff activity), or a
platform file watch (inotify/FSEvents/ReadDirectoryChangesW).
Measure: driver wakeups/s and file-syscall rate with 5 idle panels.

### C2 — Suspend offscreen/invisible panels
A panel in a hidden workspace or fully scrolled off-screen still gets
every Chrome repaint decoded + uploaded. Research: the board already
knows panel visibility each frame — send `Page.stopScreencast` when a
panel is not visible and restart on return (one CDP roundtrip, ~ms).
Chrome keeps rendering the tab; the win is Horizon-side decode + upload.
Measure: driver CPU + `Frame` event rate for a panel in a background
workspace while the page animates.

### C3 — One Chrome process, many tabs  [architecture, multi-panel]
N browser panels = N Chrome processes (~200–400 MB each + N compositor
pipelines). A shared Chrome with per-panel tabs is one process.
Research: per-panel profile isolation is the current story
(`--user-data-dir` per panel); a shared instance needs a tab-context
story (cookies/storage shared across tabs, or per-tab CDP `Network.setCookie`
scoping), plus lifecycle semantics (panel close = close tab, last tab =
close process) and per-target manifests.
Measure: RSS + CPU of 3-panel static/animating boards, single-process vs
today.

### C4 — Adaptive quality under decode pressure
If driver decode time spikes (heavy 4K-ish content at q60), drop
screencast quality for a cooldown window (screencast restart is a
~50 ms blip, already handled by the restart machinery). Research the
break-even: restart cost vs sustained decode overrun.
Measure: P95 frame-to-frame latency on a heavy video page with the
adaptive policy vs fixed q60.

## D. Deliberately out of scope

- Hardware still-JPEG decode (VideoToolbox/VAAPI/NVJPEG): no portable
  wgpu/Vulkan/Metal API for still JPEG; vendor stacks would save ~1.7 ms
  worst case and fragment the build. Revisit only if A3 lands.
- `params` clone / JSON fast-paths: measured at ≤0.02 ms worst case.
- PNG screencast: strictly more expensive on the wire and to decode.
