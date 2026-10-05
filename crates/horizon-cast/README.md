# horizon-cast

Experimental MIT implementation of modern Apple TV screen mirroring, with no
Horizon core or UI dependency. The target is Linux and current Apple TVs running
tvOS 27. Synthetic protocol tests cover authentication and transport failures;
hardware playback, frame rate and latency require separate qualification.

The Linux MVP discovers Apple TVs advertising PTP timing, performs type-5 PIN
pairing and pair verification, encrypts control/events/video, and streams H.264.
It reserves one session per receiver IP; separate receivers are independent.
No legacy authentication, audio, desktop capture or buffered media playback is
included. Horizon remembers each TV's long-term pairing identity in private
files under its `cast-pairings` directory (0700 directory, 0600 files). The PIN
and session keys are not stored with the pairing. The host's private, short-lived
MCP request queue carries a submitted PIN until the request is claimed and removed.
New sessions authenticate with the saved
identity without PIN setup. Rejected saved identities fail explicitly; forget
the pairing before retrying. The host must report `PinRequired` immediately.

`CastSession` runs pairing, transport and an external `ffmpeg` process off the UI
thread. Linux needs `ffmpeg` with the `libx264` encoder on PATH. The preset is
15 fps with explicit 720p, 1080p (default), or 4K resolution and aspect-preserving
letterboxing supplied by the host. Portrait swaps the width and height. Encoding
uses CRF 18 on CPU; increasing canvas size cannot restore detail missing from a small,
zoomed-out source. Portrait changes the encoded canvas, not the
physical orientation of the television. The preset is not a guaranteed delivered
frame rate: capture, scaling and encoding can limit throughput. Status counts
frames sent, not frames displayed by the TV. Frame queues are bounded; a source that
stops delivering frames for three seconds fails and releases its encoder.

Optional Linux acceleration uses `cargo build -p horizon-ui --release --features cast-nvenc`
(or `--features nvenc` for this crate). A bounded probe first qualifies source-sized
PAM input, NV12 conversion, CUDA scaling and NVENC at the requested output size.
Each cropped frame carries its dimensions, allowing panel resize/zoom without
restarting the receiver. CUDA preserves aspect ratio; padding stays opaque black.
Capture/cropping, NV12 conversion and final padding remain on CPU. Scaling is
downloaded before padding and NVENC upload; this is not a zero-copy pipeline.

If CUDA scaling is unavailable, a second bounded probe qualifies native RGBA
NVENC with CPU scaling. If neither hardware path works, `libx264` handles encoding.
Neither fallback changes resolution. Cancellation kills the probe; receiver
ownership remains held throughout. Crate selection and benchmark results expose
the actual scaler (`cuda` or `cpu`). Horizon submits source crops when CUDA is
selected, preserving capture isolation and density checks. UI Details and shared
MCP/CLI status report the encoder, scaler and fallback reason.
No GPU dependencies or build-time CUDA toolkit
are added. This first adapter uses the existing FFmpeg subprocess; direct
`moq-nvenc` integration remains deferred until a safe upload/configuration API is
available. The configured capture limit remains 15 fps. `submit` accepts the
fixed output canvas on any backend; hosts may use `uses_source_frames` and
`submit_source` to avoid CPU scaling only when CUDA was actually selected.
Source crops must be nonempty RGBA, at most 8192 pixels per side and 16 megapixels
in total, with both fitted canvas axes at least two pixels before even rounding.
`supports_source_dimensions` checks these bounds for a particular output canvas.
Queues remain bounded to one incoming frame; pause repeats the last
validated crop with its original dimensions. Hosts should letterbox larger or
extremely thin crops on CPU before submitting a fixed canvas, preserving source support and output
resolution; the selected encoder's CUDA filter remains active for that canvas.

Horizon exposes the same operations through its Cast picker and `cast` MCP tool:
`discover`, `sources`, `status`, `start`, `pair`, `stop`, `paired`, and `forget`.
`start` accepts `resolution: "720p" | "1080p" | "4k"` and
`orientation: "landscape" | "portrait"`; status returns both selections. The CLI
plan runner uses the same contract. Settings → Remote Devices lists and forgets
the same persisted pairings, with confirmation and busy protection. Pairing PINs
are removed from durable plans and traces; pairing and PIN-variable reuse cannot
be resumed or replayed.
`paired` returns remembered TV IDs and names, including offline TVs, without
keys. `forget` accepts a receiver ID and removes only that pairing; an active
session must finish first. The private store holds a receiver-specific file
lease through authentication, video and teardown, also rejecting competing
processes that use the same store. MCP requires a live
Horizon agent identity and checks the caller's current workspace. The UI captures
only visible panel/workspace regions in the main window. Hidden, detached,
clipped, deleted or obscured sources stop; overlapping unrelated content is
never intentionally transmitted. Cast controls and unsettled drag/resize geometry
freeze the last safe frame until capture can resume. OS desktop windows are not
captured. There is
no automatic reconnect or restart of a cast after application relaunch.

Standalone hosts opt into persistence with `CastSession::start_remembered` and
an explicit `PairingStore`. `CastSession::start` remains an ephemeral probe.

New dependency rationale: `mdns-sd` performs passive Bonjour discovery; `plist`
encodes binary AirPlay dictionaries; `crypto-bigint` provides constant-time
3072-bit SRP arithmetic. Existing workspace `ring` and `zeroize` supply the other
cryptography and secret-buffer handling. Protocol behavior follows RFC 5054,
HAP pairing framing and AirPlay interoperability documentation. Receiver proofs,
signatures and AEAD tags are checked before accepting data.

Local checks:

```sh
cargo test -p horizon-cast
cargo clippy -p horizon-cast --lib --examples -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo run -p horizon-cast --example pair -- --discover
```

The interactive `pair` example reads the current PIN from stdin. Do not place
real PINs in command arguments, logs, source files or test fixtures. Receiver
information and network endpoints are private evidence. Sustained physical-TV
playback and latency must be checked separately before shipping.

For repeatable CPU, memory and GPU optimization experiments, start with
[`auto/cast/program.md`](../../auto/cast/program.md). Its canonical benchmark
uses this crate's optimized `cast_bench` example and an independent authenticated
loopback receiver, checks decoded frames and image quality, and retains resource
measurements and fingerprints. It never discovers a TV or reads saved pairings.
