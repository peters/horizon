# Linux Apple TV casting: research and handoff

Recorded 2026-10-03. Scope: modern Apple TV, Linux, explicit Horizon sources,
portrait/landscape and 720p/1080p/4K; one live or retiring session per TV.
Multiple TVs retain independent sessions. Audio, arbitrary desktop capture,
legacy receivers and a new rendering engine remain outside the MVP.

## Decisions to preserve

Use the standalone MIT `horizon-cast` crate for discovery, authenticated pairing,
encrypted mirroring and receiver leases. UI, MCP and CLI share the same source,
authorization and session policy. Saved pairings appear in Remote Devices and
can be forgotten after stopping the receiver. Never expose pairing credentials
or restart an active receiver to recover a test.

Keep `cast-nvenc` optional. The implemented GPU path qualifies CUDA scaling plus
NVENC, then NVENC with CPU scaling, then software encoding. Record the actual
encoder, scaler and fallback reason. It adds no Rust dependencies or CUDA
build-tool requirement and preserves `forbid(unsafe_code)`. Direct GPU bindings
remain deferred until a safe integration offers a measured benefit.

The source-sized RGBA path still converts to NV12 on CPU, uploads for CUDA
scaling, downloads for padding and uploads for encoding. It is not zero-copy.
A single pending input/output frame bounds queue growth. Crop validation keeps
thin/oversized inputs from failing the encoder and retains source privacy,
geometry, pixel density and fixed output canvas rules.

The implementation choices follow [FFmpeg's CUDA filter documentation](https://ffmpeg.org/ffmpeg-filters.html#scale_005fcuda)
and [NVIDIA's FFmpeg integration guide](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/ffmpeg-with-nvidia-gpu/index.html).
Those describe GPU facilities; they do not establish Apple TV display latency.

## Measurements and their limits

A short CPU/CUDA/CUDA/CPU comparison used a moving 1152x584 synthetic source,
4K output, a 15 fps target and independent H.264 decoding on one shared GPU
host. Source preparation used the production letterbox function. Lifecycle CPU
includes setup and teardown divided by decoded frames; it is not encoding-only
latency. Peak RSS covers the sender process tree; GPU allocation is attributed
to the encoder process.

| Path | CPU ms/decoded frame | Peak RSS | GPU allocation | Measured sender cadence |
| --- | ---: | ---: | ---: | ---: |
| NVENC + CPU scaling | 46.35 | 602 MiB | 497 MiB | about 15 fps |
| NVENC + CUDA scaling | 28.15 | 444 MiB | 655 MiB | about 15 fps |

This exploratory comparison found about 39% lower CPU and 26% lower RSS against
NVENC with CPU scaling, at an additional 158 MiB of GPU allocation. A separate
software/CUDA/CUDA/software comparison found roughly 70.7 versus 28.9 CPU
ms/decoded frame, but software used about 330 MiB RSS versus 444 MiB for CUDA.
Do not claim that GPU encoding reduces memory against software. Both comparisons
are shared-host observations, not an isolated performance acceptance result.

Independent frame IDs, requested canvas, checker content, black borders and
clean teardown passed. Warmup/startup drops are reported separately from the
measured window. Forced CUDA failure retained NVENC/CPU across all six formats;
forced hardware failure retained software/CPU at the selected canvas. Two
simulated receivers passed independent-stop tests.

Earlier physical sessions sustained approximately 14.5-14.9 transmitted fps at
a configured 15 fps. This is neither a protocol ceiling nor displayed FPS.
No independent source-to-TV latency or receiver-display recording was available.
Only one physical TV is available; simulation does not qualify two physical TVs.
Upscaling a small source to a 4K canvas cannot create missing source detail.

## Delivery boundaries and next work

- #1215 retains physical motion, readability and latency acceptance.
- #1218 retains final host adapter, CPU/GPU physical comparison and display metrics.
- #1238 adds main-window capture with explicit volatile workspace consent. Its
  whole-window autoresearch workload is a separate outcome; do not close the
  issue until that workload and its required measurements are delivered.
- Continue with the native phone controller and phone-driven autoresearch after
  the prerequisite casting work is merged and verified. Keep signing/install
  evidence separate from simulator or build success.

Stop further optimization until a representative workload identifies a bottleneck.
Future experiments must preserve pairing reuse, source authorization, independent
receiver ownership, requested geometry, quality, bounded memory and clean stop.
Keep raw measurements and device identifiers private; publish aggregate results
and generic fixtures only.
