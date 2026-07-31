# macOS Speech Preload Smoke Plan

## Purpose

Validate that speech models configured with `preload: true` start loading at
launch, publish the selected backend without a dictation attempt, and surface
load failures promptly in every startup view. Also verify that lazy profiles,
runtime config replacement, and ordinary terminal startup are unchanged.

## Machine And Evidence Requirements

- macOS desktop session with Metal support.
- Debug build from the exact PR commit under test.
- A valid transcribe.cpp GGUF model for the success lane.
- A disposable `HOME`; never modify the tester's normal Horizon config.
- Scope process inspection, screenshots, and automation to the exact Horizon
  PID launched for each lane.
- Save the launch log, one screenshot for each visible failure state, and the
  commit SHA. Remove generated state after reporting.

## Build

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/tmp/horizon-pr257-target
git rev-parse HEAD
cargo build --features speech
```

The binary must be `"$CARGO_TARGET_DIR/debug/horizon"` from the SHA printed
above. Do not substitute a binary from another worktree.

## Isolated Runtime

```bash
export SMOKE_ROOT="$(mktemp -d /tmp/horizon-speech-preload.XXXXXX)"
export SMOKE_HOME="$SMOKE_ROOT/home"
mkdir -p "$SMOKE_HOME/.horizon"
cp /tmp/horizon-speech-fixtures/whisper-tiny-Q5_K_M.gguf \
  "$SMOKE_ROOT/model.gguf"
shasum -a 256 "$SMOKE_ROOT/model.gguf"
```

The expected fixture size is 44,211,616 bytes and its SHA-256 is
`72cfa8ee436a635a5b6fb373cc056a828b9efe96d32d6eb8769ed3cc5b429719`.
Record the source and verified checksum in the report.

## Baseline: Lazy Loading Remains Lazy

Create `"$SMOKE_HOME/.horizon/config.yaml"` with speech enabled and
`preload: false` using an unquoted heredoc so the model path is real:

```bash
cat >"$SMOKE_HOME/.horizon/config.yaml" <<YAML
version: 8
features:
  speech:
    enabled: true
    model: $SMOKE_ROOT/model.gguf
    backend: auto
    hotkey: F9
    preload: false
YAML
```

Launch:

```bash
HOME="$SMOKE_HOME" RUST_LOG=horizon=debug \
  "$CARGO_TARGET_DIR/debug/horizon" \
  >"$SMOKE_ROOT/lazy.log" 2>&1 &
LAZY_PID=$!
```

Verify the exact PID remains alive, the main window is viewable, and no model
load occurs before a dictation attempt. Quit that PID cleanly.

## Successful Preload

Change only `preload` to `true`, then launch a fresh process and capture its
PID. Verify:

1. The main window becomes viewable and stays responsive during model load.
2. The configured model loads without pressing the push-to-talk key.
3. The resolved backend appears in the speech settings after the worker
   reports success.
4. No continuous high-frequency repaint loop remains after preload completes.
5. Opening and closing settings does not load a second copy of the model.
6. A short first dictation, when an input device is available, does not pause
   to initialize the model. If no input device exists, report that limitation
   and verify the expected capture error is shown without a crash.

## Failure During Loading View

The startup loading view normally lasts only a frame on this machine and is not
reliably forceable from a minimal live config. Run the dedicated egui update
test that retains the bootstrap receiver while injecting a preload failure.
Optionally attempt a live lane with an unbound agent panel configured as
`resume: last`, but do not treat failure to capture that transient view as a
product failure.

```bash
cargo test -p horizon-ui --features speech \
  startup_loading_view_drains_and_renders_preload_failure
```

Verify:

1. The preload error appears without any key press, pointer input, resize, or
   unrelated terminal output.
2. The error identifies the speech profile/model failure.
3. The loading view continues progressing.
4. Once the error has been delivered, the test observes no persistent preload
   polling cadence.

## Failure During Startup Chooser

Launch one persistent process with the disposable home, then launch a second
exact binary with the same home and no session-selection flags. Keep the same
nonexistent preloaded model in both processes. The held session lease forces
the second process into the startup chooser.

Verify:

1. The chooser remains usable.
2. The preload error is rendered over the chooser without selecting a session
   or otherwise interacting with the window.
3. Choosing a session proceeds normally after the notice appears.
4. The notice expires normally and is not duplicated on subsequent frames.

## Empty Board And Idle Wake

Launch with an empty board/session, a valid preloaded model, and no terminal
output that could independently repaint the UI.

Verify the backend becomes visible in settings without resizing, moving the
pointer, or creating a panel. Repeat with the nonexistent model and verify the
failure notice also arrives while the board is idle.

## Profiles And Shared Model

Configure two profiles using the same valid model path:

```bash
cat >"$SMOKE_HOME/.horizon/config.yaml" <<YAML
version: 8
features:
  speech:
    enabled: true
    backend: auto
    profiles:
      - name: First
        model: $SMOKE_ROOT/model.gguf
        hotkey: F8
        preload: true
      - name: Second
        model: $SMOKE_ROOT/model.gguf
        hotkey: F9
        preload: true
YAML
```

Verify both profiles finish preload and the UI settles after both completion
events. Current logs do not distinguish a cache hit from the initial shared
load; retain code-review evidence for the cache key and do not claim live proof
of reuse. Then make the second path invalid and verify one success and one
named failure are both delivered.

## Runtime Replacement

While Horizon is running, change the speech config through settings so the
speech system is rebuilt.

Verify the visible behavior:

1. A new `preload: true` profile is polled to completion.
2. A replacement preload failure is surfaced once.
3. No stale success/failure from the retired worker changes the current
   backend label or notice.

Exact worker retirement ordering is not present in the live logs; retain the
worker synchronization tests as the decisive evidence for that invariant.

## Persistence And Regression Checks

1. Quit and relaunch using the same disposable home; `preload` remains set.
2. Confirm ordinary shell panels accept keyboard input and resize correctly.
3. Confirm a build without the `speech` feature still launches with the same
   config and ignores the speech runtime as documented.
4. Confirm no Horizon processes from any lane remain after cleanup.

## Report

Include:

- exact commit SHA and macOS version;
- model source and checksum;
- build and focused test results;
- pass/fail for lazy, success, loading failure, chooser failure, empty-board,
  profiles/shared model, replacement, and persistence lanes;
- exact PID per launch and paths to logs/screenshots;
- dedicated egui test evidence for the transient loading-view lane, code-review
  evidence for shared cache use, and worker tests for retirement invariants;
- any microphone-dependent lane omitted and why.
