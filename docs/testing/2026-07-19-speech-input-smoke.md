# Speech Input Smoke-Test Plan (2026-07-19)

Validates the opt-in speech feature: mic button on panel title bars, F9
push-to-talk into the focused panel, config surface, and the transcribe.cpp
integration. Executable without extra context; follow the lane for your
machine and report per `AGENTS.md` → Cross-Machine Smoke-Test Handoff.

Machine lanes:

- **Lane A — macOS arm64 (Metal)**: steps A1–A13. Fully automatable except
  A9 (live mic; a Mac Studio has no built-in microphone — A8 covers the
  no-device error path instead, which is equally load-bearing).
- **Lane B — Linux x86_64 + NVIDIA (CUDA)**: headless build/link/pipeline
  verification only (B1–B2), run by the originating agent after Lane A
  reports `SMOKE-TEST: DONE`. Agent-driven GUI smoke testing happens ONLY
  on macOS — agents must not launch Horizon instances on the Linux
  desktop; the remaining desktop checks (B3–B5) are user-driven.

## Shared setup (both lanes)

```sh
git fetch origin && git checkout feat/speech-input
# Test fixtures (~45 MB): tiny multilingual model + a known 16 kHz sample.
mkdir -p /tmp/horizon-speech-fixtures && cd /tmp/horizon-speech-fixtures
curl -sLO https://huggingface.co/handy-computer/whisper-tiny-gguf/resolve/main/whisper-tiny-Q8_0.gguf
curl -sLO https://github.com/handy-computer/transcribe.cpp/raw/main/samples/jfk.wav
cd -
```

## Lane A — macOS (Metal)

- **A1 — stub build unchanged**: `cargo build --release` succeeds;
  `cargo clippy` reports no warnings from `speech`/`panel`/`config` files.
- **A2 — stub tests**: `cargo test` all green.
- **A3 — speech build**: `cargo build --release --features speech` succeeds
  (compiles the vendored C++ tree; needs CMake + clang only).
- **A4 — speech clippy/tests**: `cargo clippy --features speech` has no
  warnings; `cargo test --features speech` all green (includes resampler
  unit tests and config serde round-trip).
- **A5 — real transcription (Metal)**:

  ```sh
  HORIZON_SPEECH_TEST_MODEL=/tmp/horizon-speech-fixtures/whisper-tiny-Q8_0.gguf \
  HORIZON_SPEECH_TEST_WAV=/tmp/horizon-speech-fixtures/jfk.wav \
    cargo test --features speech -p horizon-ui --bin horizon speech_pipeline -- --nocapture
  ```

  Expect `backend: Metal` (auto-detected) and a transcript mentioning
  "fellow Americans" (whisper-tiny may vary slightly; non-empty ≈ pass,
  and note the text in the report).
- **A6 — config surface**: with `features.speech.enabled: true`, `model:`
  pointing at the fixture, `hotkey: "F9"` in `~/.horizon/config.yaml`
  (back up the original first, restore after), launch
  `target/release/horizon`: a mic glyph appears left of the close button
  on every terminal-backed panel title bar (Editor/Git Changes/Usage
  panels have no PTY and show none). Screenshot it. With `enabled: false` (or a
  build without the feature) the glyph is absent and the settings pane
  shows the rebuild hint under Features → Speech Input.
- **A7 — layout regression**: with speech enabled, panel titles, the
  history meter, SSH badge, and attention badge must not overlap the mic
  glyph (badges shift left of it). Screenshot a panel with a long title.
- **A8 — no-microphone error path**: on a machine with no input device,
  click the mic button: the app must not crash; a
  `speech input error`/`no microphone found` warning appears in logs and
  the mic returns to idle.
- **A9 — live mic (only if an input device exists)**: click mic → speak →
  click again; text is inserted into that panel's prompt with a trailing
  space and no newline. Hold F9 → speak → release; text lands in the
  *focused* panel. Escape while recording cancels. F9 press/release must
  not leak `~`/escape sequences into the terminal.
- **A10 — Settings workflows (GUI)**: open Settings → General → Features →
  Speech Input with a model configured. Verify: the **spoken-language**
  dropdown lists the model's declared languages (not free-form) and the
  metadata line shows the language count + translate capability; the
  **Output** picker offers Transcribe and the model's declared translate
  targets, and switching the spoken language resets an out-of-range
  target; the **backend** picker only lists backends compiled into this
  build and shows `active: <backend>` after a dictation; **Rebind…**
  captures a pressed chord, rejects a chord that overlaps a global
  shortcut (or another profile) with an inline error, and rejects bare
  keys/clipboard chords; changing **task/mode/backend/hotkey** and saving
  applies live with no restart.
- **A11 — profiles**: configure `features.speech.profiles` with two
  profiles (distinct models/languages and hotkeys F1/F2). Verify each
  hotkey dictates with its own profile, the mic button reuses the
  last-used profile, the mic tooltip lists both keys, and the Settings
  summary shows per-profile rows. A profile with an invalid/duplicate
  hotkey (or a non-first profile with no hotkey) is rejected on save.
- **A12 — persistence after relaunch**: quit and relaunch; the saved
  speech config (profiles, language, task, backend, hotkey) is restored
  and dictation still works without re-editing.
- **A13 — packaged bundle (macOS)**: build the `.app` via
  `packaging/macos/make_app_bundle.sh` and launch that bundle (not the raw
  binary). First dictation must trigger the microphone permission prompt
  (the bundle's `Info.plist` carries `NSMicrophoneUsageDescription`);
  granting it lets dictation proceed. The raw binary cannot exercise this
  TCC path.

## Lane B — Linux + NVIDIA (CUDA)

Agent-run (headless only):

- **B1 — CUDA build**: `cargo build --release --features speech-cuda`.
- **B2 — pipeline on GPU**: A5's command but with
  `--release --features speech-cuda` (running it verbatim would rebuild
  and test the CPU feature set) — expect `backend: CUDA0`-style output
  and a sane transcript.

User-driven (agents must not run GUI smoke on the Linux desktop):

- **B3 — launch + mic button**: as A6, verified by the user.
- **B4 — live dictation (RØDE mic present)**: as A9, in Norwegian with the
  NB-Whisper model, `language: "no"`; verify dialect speech lands as
  bokmål text in the focused panel. Also verify `task: translate` +
  whisper-large-v3 inserts English.
- **B5 — backend fallback**: same config with `backend: cpu` still works.

## Reporting

Per `AGENTS.md`: push fixes to `feat/speech-input`, then post
`SMOKE-TEST REPORT (<lane>)` on the PR with per-step pass/fail and finish
with the literal marker line `SMOKE-TEST: DONE`. If a step cannot run
(e.g. A9 without a mic), mark it `skipped — <reason>` rather than pass.
