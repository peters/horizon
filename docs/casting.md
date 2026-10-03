# Linux casting

Horizon can mirror a selected panel, workspace or its entire main window to a
modern Apple TV. Casting
is Linux-only. The Cast icon appears beside the recording/microphone controls
on supported builds. It opens source, receiver, orientation and resolution
selection, plus status and stop controls. Each TV allows one session, including
pairing and teardown; different TVs can have independent sessions.

## Runtime and build

Install an FFmpeg build providing `libx264`. Software encoding is available in
the normal Linux build. Optional NVIDIA H.264 encoding uses the driver's NVENC
engine:

```sh
cargo build -p horizon-ui --release --features cast-nvenc
```

This feature adds no Rust GPU dependencies and does not require the CUDA toolkit
on the build machine. At runtime, it needs a usable NVIDIA driver/GPU and FFmpeg
with `h264_nvenc`. A bounded probe checks the requested output dimensions before
streaming. If hardware initialization fails, status reports the CPU fallback;
the requested resolution stays unchanged.

The computer and receiver must be reachable on the local network. Discovery
uses the receiver's advertised AirPlay service. First use requires the fresh PIN
shown on the TV; subsequent starts reuse authenticated credentials stored in
private per-device files. Settings → Remote Devices lists remembered receivers,
including offline devices. Forget removes the selected pairing and refuses to
operate while the TV has an active or retiring session.

## UI, MCP and CLI

The UI and public `cast` MCP tool share the same workspace-scoped operations:
`discover`, `sources`, `start`, `pair`, `status`, `stop`, `paired`, and `forget`.
The CLI plan runner also exposes `cast`. An agent must run from an authorized
Horizon agent panel; another workspace's source IDs are refused. None of these
operations requires Settings to be open.

The **Entire Horizon** source is `{ "kind": "application" }`. It includes the
toolbar, sidebar, canvas and in-window dialogs. A person can start it directly
in the Cast picker. Agent starts require that person to select **Allow this
workspace's agents**; `sources` reports `requires_user_approval` until then.
Approval belongs to one workspace, lasts only for the current application
session, and cannot be granted through MCP or CLI. Revoking it or approving
another workspace stops agent-started application casts while preserving casts
started by the person. Switching application sessions clears approval.

Call `discover`, wait for `status.discovering` to become false, and obtain a
source from `sources`. Pass the returned source object unchanged when starting:

```json
{
  "operation": "start",
  "receiver_id": "<discovered receiver id>",
  "source": {"kind": "panel", "id": "<returned source id>"},
  "orientation": "landscape",
  "resolution": "4k"
}
```

`orientation` supports `landscape` and `portrait`; `resolution` supports `720p`,
`1080p` (default), and `4k`. Portrait swaps the encoded canvas dimensions,
preserving the source aspect ratio with black borders. It does not rotate the
physical TV. Stop the receiver before changing source, orientation or
resolution. A duplicate start cannot replace the existing session.

`status.sessions` reports the state, cumulative successfully sent `frames`,
selected `encoder`, `encoder_fallback`, and any terminal `error`. Sent-frame
counts remain available after stopping or failing. They do not measure displayed
TV frame rate or playback latency. `paired` exposes receiver metadata only,
never credentials. Pairing PINs must not be written to durable plans or logs.

## Capture and performance boundaries

Panels and workspaces must be fully visible and unobscured in Horizon's main
window. Fit the whole source into view first. Hidden, clipped, detached, deleted
or covered panel/workspace sources stop casting. Entire Horizon captures the
main window's rendered content, including its own dialogs, without capturing
the surrounding desktop, other applications or detached windows. A minimized
or unavailable main window cannot start that source. Opening Cast controls or changing source geometry
briefly repeats the last validated image until capture is safe. Desktop and
arbitrary-application capture, audio, HDR, legacy receivers, background rendering
and automatic reconnect are outside this MVP.

Capture is capped at approximately 15 frames per second. Whole-window GPU
readback, CPU crop/scaling, rendering and encoder input transfer can lower the
actual rate. A 4K output setting specifies encoded dimensions; it cannot create
detail absent from the rendered source. NVENC reduces encoding work but does
not move capture or scaling onto the GPU.

A 2026-10-03 physical receiver baseline on implementation `54a5620c`, using a
silent terminal counter and an isolated native VNC desktop, sent approximately
3.9 fps from a 3840×2160 desktop and 8.5 fps from a 1920×1080 desktop with 4K
NVENC output. The larger software run sent approximately 3.9 fps. Sampled
encoder-process lifetime CPU usage was approximately 20% for software versus
9% for NVENC; the application used roughly one CPU core in both large-desktop
runs. These simple-pattern results are not a representative video benchmark or
a measurement of TV display FPS, readability or end-to-end latency.

Preserving the capture schedule across slightly late UI frames and waking the
host when scaled frames become ready raised the same small-desktop counter
scenario to approximately 14.5 fps at 4K landscape, 14.7 fps at 1080p landscape,
14.7 fps at 720p portrait and 14.9 fps at 4K portrait. Each run reused remembered
pairing and stopped cleanly. These remain sender measurements on a simple
pattern; more complex content and larger capture surfaces need separate tests.

A separate 90-frame synthetic FFmpeg scaling experiment from 1280×660 to a 4K
letterboxed canvas took about 1.85 seconds / 5.14 CPU seconds with CPU scaling,
versus 1.55 seconds / 1.51 CPU seconds with CUDA scaling. On the tested FFmpeg
build, the RGBA CUDA route was rejected; the successful route converted to NV12
before upload and downloaded the scaled image for CPU padding. It therefore
requires an input-format and geometry integration, not merely an encoder flag.
The measured benefit must be preserved across actual capture, dynamic geometry,
source isolation and text-quality tests before enabling that path.

[NVIDIA's FFmpeg guide](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/ffmpeg-with-nvidia-gpu/index.html)
and the [FFmpeg filter documentation](https://ffmpeg.org/ffmpeg-filters.html#scale_005fcuda-1)
describe the driver and scaling requirements. A future direct Rust adapter must
provide a safe integration with Horizon's `forbid(unsafe_code)` policy; the
current subprocess adapter avoids importing the low-level GPU bindings.

## Acceptance still requiring physical evidence

The [casting autoresearch program](../auto/cast/program.md) provides separate
CPU, process-tree memory, throughput and GPU-allocation objectives for future
agents. Its canonical benchmark covers every output resolution/orientation using
generated frames and an independent loopback receiver. Decoder, image-quality
and teardown gates prevent broken output from scoring as an improvement. These
development measurements exclude UI capture and the physical display.

Sustained sender sessions, remembered pairing and clean stop have been checked
on one physical receiver. Synthetic receivers qualify protocol rejection,
independent sessions and decoded geometry. Physical displayed motion, text
readability, source-to-TV latency and two simultaneous physical receivers remain
separate acceptance checks in #1215 and #1218. A recording showing a changing
source counter and the TV together can establish displayed-frame progression
and latency; protocol/frame-send success alone cannot.
