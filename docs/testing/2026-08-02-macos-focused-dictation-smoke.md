# macOS Focused-Field Dictation Smoke Test

Temporary validation artifact for the focused-field dictation pull request. Run every lane against the exact PR head on a logged-in Mac desktop, report results in the PR, and delete this file only after the final-head rerun passes.

## Scope and safety contract

Validate:

- Accessibility permission and all-or-nothing global registration
- F1 Norwegian transcription, F2 English transcription, and F3 Norwegian-to-English translation
- exact focused-field insertion in Firefox without submitting a draft
- focus, permission, lifecycle, persistence, and registration-conflict safety
- unchanged Horizon-local terminal and mic-button behavior

Never press Enter in the ChatGPT composer. Do not capture private transcript text, field values, window titles, clipboard contents, credentials, or model paths in logs or screenshots.

## Coordinator connectivity gate

From the coordinating machine, restore Tailscale and verify the Mac before claiming the smoke is ready to run:

```bash
tailscale status
MAC_SMOKE_HOST="<configured Mac endpoint>"
ssh -o BatchMode=yes -o ConnectTimeout=8 "$MAC_SMOKE_HOST" \
  'sw_vers; uname -m; xcode-select -p; rustc --version; cmake --version'
```

Use the coordinator's private inventory to fill in the endpoint; do not commit node names or addresses. Do not report smoke readiness until SSH succeeds and a logged-in Mac desktop is available through Tailscale Screen Sharing or local interaction. The Accessibility and Firefox lanes require that interactive desktop session; an SSH-only session is insufficient.

## Record the environment

Create an isolated worktree at the exact PR SHA and keep all runtime state outside the real Horizon home.

```bash
git fetch origin
git worktree add /tmp/horizon-focused-dictation-smoke <exact-pr-sha>
cd /tmp/horizon-focused-dictation-smoke
git lfs pull
git rev-parse HEAD
sw_vers
uname -m
rustc --version
cmake --version
```

Record:

- macOS version and architecture
- Firefox version
- exact Git SHA
- exact Horizon PID for every run
- microphone name used for capture
- whether the top row requires Fn for F1/F2/F3

Confirm a usable external, Continuity, or loopback microphone before testing. A Mac Studio may not expose a built-in input.

## Isolated state and models

Create a temporary home and config. Never edit the Mac's real `~/.horizon/config.yaml`.

```bash
SMOKE_ROOT="$(mktemp -d)"
SMOKE_HOME="$SMOKE_ROOT/home"
SMOKE_CONFIG="$SMOKE_ROOT/config.yaml"
mkdir -p "$SMOKE_HOME"
```

Verify these two model files are available in the temporary smoke environment:

- `nb-whisper-large-Q8_0.gguf`
- `whisper-large-v3-Q8_0.gguf`

If either is absent, check free disk space and copy only those files over Tailscale. Do not add models to Git.

Create `$SMOKE_CONFIG` with Speech Input enabled, `dictate_outside_horizon: true`, hold mode, the selected microphone, and these profiles:

```yaml
features:
  speech:
    enabled: true
    backend: auto
    input_device: "<exact microphone name>"
    hotkey_mode: hold
    dictate_outside_horizon: true
    profiles:
      - name: Norsk
        model: <temporary path>/nb-whisper-large-Q8_0.gguf
        language: "no"
        task: transcribe
        hotkey: F1
      - name: English
        model: <temporary path>/whisper-large-v3-Q8_0.gguf
        language: en
        task: transcribe
        hotkey: F2
      - name: NO to EN
        model: <temporary path>/whisper-large-v3-Q8_0.gguf
        language: "no"
        task: translate
        target_language: en
        hotkey: F3
```

## Build and launch the app bundle

```bash
bash -n packaging/macos/make_app_bundle.sh
cargo build --features speech
./packaging/macos/make_app_bundle.sh \
  --speech \
  --binary target/debug/horizon
cmp target/debug/horizon target/Horizon.app/Contents/MacOS/horizon
plutil -lint target/Horizon.app/Contents/Info.plist
/usr/libexec/PlistBuddy -c 'Print :NSMicrophoneUsageDescription' \
  target/Horizon.app/Contents/Info.plist
```

Confirm `cmp` succeeds, proving that the custom debug speech binary was bundled. Confirm the printed microphone usage text covers both terminal panels and focused text fields. Also package once without `--binary` when a release binary is available and confirm the script retains its `target/release/horizon` default.

Launch the bundled binary, not the unbundled Cargo output, so macOS permission identity is correct:

```bash
HOME="$SMOKE_HOME" \
RUST_LOG=horizon=debug \
target/Horizon.app/Contents/MacOS/horizon \
  --config "$SMOKE_CONFIG" \
  --ephemeral \
  --blank
```

Record the PID with `pgrep`/Activity Monitor and scope later inspection and automation to that PID.

## Lane 1: permission and registration

1. Start with Accessibility denied for this exact `Horizon.app` identity.
2. Open Settings → General → Features → Speech Input.
3. Confirm status says Accessibility is required and offers **Grant Accessibility…** and **Retry**.
4. Confirm merely opening Settings and parsing YAML does not show a permission prompt.
5. With Firefox foreground, press F1 and F3. Confirm Firefox receives its normal Help/Find behavior and Horizon does not start capture.
6. Confirm Horizon-local dictation and its mic button remain available.
7. Click **Grant Accessibility…** and grant Accessibility to this exact app bundle.
8. Grant Microphone permission when capture is explicitly started.
9. Confirm macOS never asks for Input Monitoring.
10. Return to Settings and click **Retry** if needed.
11. Confirm status lists all three registered bindings: F1, F2, and F3.
12. If the top row controls brightness/media, enable standard function keys or record that Fn+F1/F2/F3 is required.

Capture a privacy-safe screenshot of the final registration status.

### Unsupported binding is all-or-nothing

1. Temporarily change one saved profile hotkey to F35, the v1 adapter's intentionally unsupported function-key case, while leaving the other two profiles on supported keys.
2. Save and confirm Settings reports **Unsupported binding**, including the exact F35 binding and reason.
3. With Firefox foreground, exercise every configured profile key. Confirm none is captured by Horizon; the browser receives the supported F-keys normally and there is no partial registration.
4. Confirm Horizon-local speech and the mic button remain available despite the global-registration refusal.
5. Restore F1/F2/F3, Save, and use **Retry** if needed. Confirm all three bindings register together before continuing.

## Lane 2: Horizon regression

With a terminal-backed panel focused:

1. Hold F1, speak Norwegian, and release. Confirm one transcript plus its trailing space enters that panel.
2. Repeat with F2 in English and F3 Norwegian-to-English translation.
3. Confirm no function-key escape sequence reaches the terminal.
4. Confirm the title-bar mic button starts/stops dictation and uses the expected last profile.
5. Start dictation, press and release Escape, and confirm both Escape events are consumed without terminal leakage.
6. Open Settings, Search, command palette, panel rename, and workspace rename in turn. Confirm their existing hotkey gates still prevent an unintended local start.
7. Click **Rebind…**. Confirm status reports a temporary pause and Firefox receives the globally released keys during capture.
8. Cancel with Escape and confirm the saved F1/F2/F3 registration set returns after the captured release.
9. Capture a new binding but close Settings without Save. Confirm the saved runtime set returns.
10. Repeat while holding the captured F-key for longer than the three-second lost-release guard. Confirm timeout re-registration does not treat native repeats as a fresh dictation press; release it, then confirm a new press works.
11. Save a valid new set. Confirm the complete new set replaces the old set atomically, then restore F1/F2/F3 for the remaining lanes.
12. Exercise the root window and a detached window without broadening the existing detached-terminal hotkey policy.

## Lane 3: Firefox ChatGPT draft, without submission

Focus the ChatGPT composer with Horizon unfocused or hidden.

1. F1: speak a Norwegian phrase containing `æ`, `ø`, and `å`. Confirm exactly one insertion at the caret with a trailing space and no Firefox Help.
2. F2: select part of an unsent draft, dictate English, and confirm only the selection is replaced; surrounding text remains byte-for-byte unchanged.
3. F3: speak Norwegian and confirm English output replaces/inserts at the caret; Firefox Find must not open.
4. Run two consecutive dictations and confirm one normal spacing boundary with no duplicate insertion.
5. Use Command-Z after each insertion and confirm normal undo behavior.
6. Seed the clipboard with mixed/rich content before dictation. Compare all clipboard flavors afterward and confirm none changed.
7. Confirm the draft was never submitted and no synthetic Enter occurred.

Capture a privacy-safe screenshot of an unsent draft and a short `screencapture -V` recording showing one complete press/record/release/insert interaction.

## Lane 4: exact-focus and target refusal

Use a temporary local page with a normal text input, textarea, read-only field, disabled field, and password field.

Before recording:

1. Normal input and textarea are eligible.
2. Read-only, disabled, and password/secure fields refuse before microphone capture.
3. A non-editable page element refuses before capture.

During recording and again during transcription, independently test each change:

- another editable field in the same Firefox tab
- another Firefox tab
- another Firefox window
- another application
- closing the target element/tab/window

For every change, the result must appear nowhere. Refocus the original/another eligible field and confirm the next dictation succeeds, proving cancellation did not wedge the system.

Explicitly distinguish the two focus-loss contracts:

1. Start terminal dictation, then move focus away from every Horizon window. Confirm terminal dictation cancels and no late transcript is delivered.
2. Start external dictation in a valid Firefox field while Horizon is already in the background. Keep that exact Firefox field focused and confirm Horizon remaining unfocused does not cancel it; global key release stops hold mode and the result inserts once.

## Lane 5: lifecycle and failure recovery

1. Hold a profile key through focus changes and confirm its background release stops only its own hold recording.
2. Exercise quick repeat presses and simultaneous F1/F2/F3 presses. Confirm no duplicate or double start.
3. Switch temporarily to toggle mode and confirm one press starts, the next stops, and repeats are ignored.
4. Test first-use cold model loading, cancellation during load, and a successful retry.
5. Reload the config while recording and while transcribing. Confirm active target ownership is cleared and late results never insert.
6. Switch Horizon sessions and confirm all speech target ownership clears.
7. Quit Horizon during recording and during transcription. Confirm browser F-key behavior returns immediately and no late insertion occurs.
8. Revoke Accessibility during active external dictation. Confirm capture/transcription cancels or is discarded, all global registrations are removed, and no fallback insertion occurs.
9. Re-enable permission and use **Retry**. Confirm all three bindings return together.
10. Disable `dictate_outside_horizon`. Confirm browser F-key behavior returns while Horizon-local speech and the mic button still work.
11. Start a second Horizon instance with the same bindings. Confirm it reports the exact conflicting binding and registers none of the set.
12. Stop the owning instance and click **Retry** in the other. Confirm all three register successfully.
13. Save, relaunch, minimize/hide, and sleep/wake. Confirm registration state persists and no press/release state remains stuck.

## Lane 6: visual and idle-performance checks

1. Inspect the Speech Input settings at normal and narrow window sizes. Status text, exact binding, reason, and buttons must wrap without widening or clipping the settings panel.
2. Confirm unsupported/conflict errors remain readable with long reasons.
3. If a simulated or observed unregister failure reports that Horizon must restart to release a system key, confirm **Retry** is absent and the restart instruction remains readable.
4. With global dictation disabled, observe idle CPU/repaint behavior and confirm no new continuous repaint loop.
5. With it enabled but idle, confirm there is no repeated Accessibility work or polling-driven repaint.
6. During active dictation, confirm the existing bounded speech polling cadence remains responsive.

## Automated validation on exact head

Run from the exact worktree that will be pushed:

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace
RUSTFLAGS="-D warnings" cargo test --workspace --features speech
cargo clippy --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
```

Save command output with the exact SHA. Logs may include target kind, profile, generation, binding, and refusal reason, but never transcript text, field content, clipboard data, or window title.

## PR handoff

Before Mac execution, post:

```text
SMOKE-TEST REQUEST Mac Studio/macOS — plan: docs/testing/2026-08-02-macos-focused-dictation-smoke.md — scope: Accessibility permission, global F1/F2/F3, Firefox ChatGPT insertion, focus safety, persistence, and Horizon regression
```

If the executor fixes anything, push it to the PR branch, rerun repository validation, and repeat every affected lane on the new exact head.

After all lanes pass, delete this temporary plan, commit that cleanup, and rerun the decisive Firefox, focus-safety, and automated-validation lanes on the final SHA. Post:

```text
SMOKE-TEST REPORT (Mac Studio/macOS <version>, <arch>, <final-sha>)
- preflight/build: pass | fail — ...
- permissions/registration: pass | fail — ...
- Horizon regression: pass | fail — ...
- Firefox ChatGPT: pass | fail — ...
- focus/privacy/lifecycle: pass | fail — ...
- evidence/final-head verification: pass | fail — ...
Summary: <fixes, remaining concerns, and evidence locations>
SMOKE-TEST: DONE
```
