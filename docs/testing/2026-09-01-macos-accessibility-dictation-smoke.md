# macOS Accessibility Dictation Smoke Test

Temporary validation artifact for the macOS direct-dictation pull request. Run
every lane on a logged-in Mac desktop against the exact PR head. Do not merge
until the report ends with `SMOKE-TEST: DONE` and every required lane passes.

## Safety contract

- Do not stop, signal, reuse, or automate a pre-existing Horizon process.
- Launch only the task-owned app bundle with an isolated home, config, and
  session. Record its exact PID and target only that PID/window.
- Do not change the user's default audio source or sink. A monitor/loopback
  input may be selected in the isolated Horizon config when already present.
- Never log transcripts, field contents, clipboard contents, window titles,
  credentials, model paths, or unrelated machine identifiers.
- Never press Enter as part of transcript delivery. Direct insertion must use
  macOS Accessibility only; clipboard writes, synthetic typing, and paste are
  failures.
- Close the task-owned Horizon window normally and verify only its PID exits.

## 1. Exact-head and environment preflight

Use a fresh clone or isolated worktree from the PR branch:

```bash
git fetch origin
git worktree add /tmp/horizon-macos-dictation-smoke <exact-pr-sha>
cd /tmp/horizon-macos-dictation-smoke
git lfs pull
git rev-parse HEAD
git status --short
sw_vers
uname -m
rustc --version
cmake --version
```

Confirm:

- `HEAD` equals the requested PR SHA and the worktree is clean.
- A logged-in console user owns an interactive desktop session.
- Existing Horizon PIDs are recorded and excluded from every later command.
- At least 15 GiB is free for the speech-enabled build.
- `system_profiler SPAudioDataType` reports a usable microphone or an already
  installed loopback/monitor input. If none exists, stop: audio smoke is not
  replaceable with a build-only claim.

## 2. Native build and automated Mac tests

Run the repository matrix on the Mac exact head:

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace
RUSTFLAGS="-D warnings" cargo test --workspace --features speech
cargo clippy --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
```

Do not use the ignored libtest hotkey smoke on macOS. Carbon hotkey delivery
depends on the main application event loop, while Rust's test harness runs test
bodies on worker threads. Instead, run the main-thread native hotkey smoke while
the test key is free, then press and release F9 once within its ten-second
window:

```bash
cargo run -p horizon-cursor --example macos_global_hotkey_smoke
```

This low-level harness supplements rather than replaces native delivery through
the task-owned Horizon bundle in sections 3, 4, and 6.

The macOS-only unit tests must cover target validation, F-key translation,
native-repeat suppression, and release delivery.

## 3. Isolated app bundle and permissions

Create task-owned runtime paths outside the repository. Never edit the real
`~/.horizon/config.yaml`:

```bash
SMOKE_ROOT="$(mktemp -d /tmp/horizon-macos-dictation-runtime.XXXXXX)"
SMOKE_HOME="$SMOKE_ROOT/home"
SMOKE_CONFIG="$SMOKE_ROOT/config.yaml"
mkdir -p "$SMOKE_HOME"
cargo build --release --features speech
./packaging/macos/make_app_bundle.sh --speech
plutil -lint target/Horizon.app/Contents/Info.plist
/usr/libexec/PlistBuddy -c 'Print :NSMicrophoneUsageDescription' target/Horizon.app/Contents/Info.plist
```

The permission text must cover terminal panels and focused fields in other
apps. Configure `features.speech.desktop_injection: true`, hold mode, the
selected input device, an available local model, and an unused F-key profile.

Launch the bundled executable with `HOME="$SMOKE_HOME"`, `--config
"$SMOKE_CONFIG"`, and `--ephemeral`. Confirm the new PID differs from every
preflight PID. In Settings -> General -> Speech Input:

1. Inspect the status at normal and narrow widths.
2. If Accessibility is denied, confirm the status is explicit and the
   **Grant Accessibility...** button is visible without clipping.
3. Click the button, grant the exact task app in macOS System Settings, return
   to Horizon, and confirm the status changes to **Accessibility granted**.
4. Confirm macOS never requests Input Monitoring.
5. Start speech explicitly and grant Microphone permission if prompted.

Capture privacy-safe screenshots at launch, normal settings width, narrow
settings width, after a native window resize, and after the Fit interaction.
Screenshots must show no unrelated applications, terminal contents, paths, or
host identifiers.

## 4. Real hotkey, audio, and direct insertion

Use a disposable TextEdit document with a unique, non-sensitive marker. Seed
the clipboard with a sentinel and record clipboard flavor metadata plus a hash
of its textual representation without printing the content.

1. Focus an empty caret in TextEdit with Horizon in the background.
2. Hold the configured global F-key, play/speak a deterministic 3-5 second
   phrase through the selected input, wait for playback to finish, and release.
3. Confirm the full transcript, including a unique tail marker, appears once
   at the caret with one trailing space.
4. Confirm no Return/Enter is inserted and no action is submitted.
5. Repeat with an 8-10 second phrase and assert the unique tail marker; a
   beginning-only transcript is a failure.
6. Run two consecutive dictations and confirm neither is duplicated or fused.
7. Confirm Command-Z treats each insertion as a normal editable operation.
8. Recheck clipboard flavors and hash. They must be byte-for-byte unchanged.

Correlate the result with privacy-safe logs for input-device name, negotiated
sample rate/channel count, submitted duration, backend, profile number, and
success/refusal only. Logs must not contain the transcript, field value, or
window title.

## 5. Focus-change and unsafe-target refusals

Every refusal must leave every field unchanged and must not fall back to the
clipboard or synthetic keys.

### Refuse before microphone capture

- Select non-empty text in TextEdit, then press the F-key. Confirm a clear
  refusal occurs before capture and the selection remains unchanged.
- Focus a hidden-answer AppleScript dialog (`display dialog` with a hidden
  default answer), then press the F-key. Confirm the secure field is refused
  before capture.
- Focus a non-editable Finder surface and press the F-key. Confirm no recording
  starts and no text appears anywhere.

### Discard after capture

For each case, start a 3-5 second utterance in TextEdit, change focus before
release, then release and wait for transcription:

- another field/document;
- another TextEdit window;
- another application;
- close the target document;
- switch A -> B -> A before release (the away-and-back race).

No transcript may appear in the original target, the new target, or anywhere
else. Refocus a safe empty caret and confirm the next dictation succeeds once,
proving cleanup did not wedge target ownership.

## 6. Hotkey and lifecycle regression

1. With Horizon focused, confirm the same profile hotkey still dictates into
   the focused terminal panel and does not route to another app.
2. Confirm F-key native repeats create one press and one release only.
3. Confirm rapid tap and quick re-press do not duplicate or wedge recording.
4. Cancel external recording and transcription with Escape; no late insertion
   may occur.
5. Change/reload speech config during external recording. The target must be
   released and no late insertion may occur.
6. Disable `desktop_injection`; the external F-key must return to the foreground
   app while Horizon-local mic-button behavior remains available.
7. Relaunch the task bundle with the same isolated config and verify permission
   status, registration, and one successful insertion again.
8. Close the exact task-owned window through the normal window close path,
   verify its PID exits, and confirm every pre-existing Horizon PID remains.

## 7. Visual review

Inspect every captured screenshot at original resolution. Pass only if:

- status dot, label, and button align with existing Speech Input controls;
- copy wraps naturally without widening the settings panel;
- normal and narrow layouts have no clipping, overlap, or orphaned controls;
- resize and Fit leave panel chrome, spacing, and text crisp;
- permission-denied and permission-granted states are both visually clear;
- there is no passive animation or continuous repaint while idle.

Record screenshot filenames and image dimensions in the report. Do not commit
screenshots or runtime artifacts to the repository.

## 8. PR report and final-head rule

Before execution, post:

```text
SMOKE-TEST REQUEST Mac Studio/macOS — plan: docs/testing/2026-09-01-macos-accessibility-dictation-smoke.md — scope: native build, Accessibility/global hotkey, real speech insertion, focus/privacy refusals, lifecycle, and visual QA
```

If a behavior change is pushed, rerun every affected lane on the new SHA. After
all lanes pass, remove this temporary plan in a documentation-only commit, run
the repository matrix, and rerun the exact-head native build, decisive direct
insertion, away-and-back refusal, clipboard sentinel, and visual screenshots on
that final SHA.

Post the final report as:

```text
SMOKE-TEST REPORT (Mac Studio/macOS <version>, <arch>, <final-sha>)
- exact-head/native build: pass | fail — ...
- permissions/global hotkey: pass | fail — ...
- real audio/direct insertion: pass | fail — ...
- focus and unsafe-target refusal: pass | fail — ...
- clipboard and lifecycle: pass | fail — ...
- visual launch/resize/Fit: pass | fail — ...
- pre-existing process isolation: pass | fail — ...
Summary: <fixes, evidence locations, and remaining concerns>
SMOKE-TEST: DONE
```
