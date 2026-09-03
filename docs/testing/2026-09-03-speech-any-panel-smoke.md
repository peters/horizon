# Speech Into Any Horizon Panel Smoke Test

Temporary validation for dictation into terminal, editor, and browser panels
on the exact candidate head.

Do not stop or reuse a pre-existing Horizon process. Identify windows by PID,
not application name. Use an isolated config and `--ephemeral`. Unset `HORIZON`
for the child. Never log transcripts, field contents, or credentials. Do not
change the user's default PipeWire/Pulse sink or source.

## 1. Build

```bash
RUSTFLAGS="-D warnings" cargo test -p horizon-core -p horizon-ui --features speech
cargo build --features speech
```

Prefer `target/debug/horizon` unless release behavior is under test.

## 2. Isolated launch

1. Pick an unused X display (`ls /tmp/.X11-unix`; e.g. `:21`).
2. Start `Xvfb :21 -screen 0 1280x800x24` and a nested window manager (`openbox`).
3. Create a temporary PipeWire loopback **without** changing defaults:
   - sink node `horizon-smoke-sink` / description `Horizon smoke sink`
   - source node `horizon-smoke-mic` / description `Horizon smoke mic`
   - quote the descriptions so Pulse/CPAL see the full name
4. Confirm `wpctl` still stars the user's original default sink/source.
5. Write a config outside the repo with:
   - `features.speech.enabled: true`
   - `backend: cpu` (debug CUDA preload is not required)
   - `input_device: Horizon smoke mic`
   - `hotkey: F9`, `hotkey_mode: hold`, `preload: true`
   - `desktop_injection: false`
   - a shell panel running a raw-mode stdin capture (cbreak; no Return)
   - an editor panel on a temp markdown file containing `ab cd`
6. Copy the Pulse cookie into the isolated `HOME` and set
   `PULSE_SERVER=unix:${XDG_RUNTIME_DIR}/pulse/native`.
7. Launch:
   `HORIZON= HOME=<isolated> RUST_LOG=info target/debug/horizon --config <temp> --ephemeral`
8. Wait for `speech model preloaded` and `using configured speech input device ... device=Horizon smoke mic`. Abort if the log falls back to the system default microphone.

Hold F9 on the candidate window, play a WAV into `horizon-smoke-sink` with `pw-play --target=horizon-smoke-sink`, then release F9. Correlate each lane with logs: selected device, negotiated rate/channels, submitted duration, and on-screen insert.

## 3. Baseline

- Candidate window maps (`IsViewable`) and shows the shell + editor.
- Mic buttons appear on terminal, editor, and live browser panels only.
- Git Changes / Usage have no mic.
- User Horizon PID (if any) is untouched.

## 4. Terminal panel

Unique tail marker required on every completed utterance (e.g. `zephyr`).

| Lane | Procedure | Expect |
|------|-----------|--------|
| Short | ~0.5 s WAV | Insert, no Return |
| Normal 3–5 s | Phrase ending in the tail marker | Tail present, no command submit |
| 8–10 s | Longer phrase, same tail | Tail present (not only the start) |
| Rapid release | Key down, <0.3 s of audio, key up | No crash; empty or partial is OK |
| Cancel | Key down, start WAV, Escape, key up | No insert from that utterance |

## 5. Editor panel

1. Focus the editor (sidebar row). Seed `ab cd`.
2. Dictate unique marker A, then unique marker B without moving the caret.
3. Confirm both appear **in order** after the original text, Edit mode is on
   (`PreviewMode::Split`/`Preview` must switch to Edit), buffer is dirty.
4. A second dictation must not land before the first (stale `TextEdit` caret).

## 6. Browser panel

1. Open a live browser panel on a page with a focused text field and no selection.
2. Title-bar mic, unique marker, release.
3. Marker appears in the page field; the page does not submit.
4. Stopped/error browser: no mic / PTT does not silently drop — show a notice
   that the browser is not running; clipboard is not used.

## 7. Non-text and overlays

1. Focus Git Changes or Usage: no mic, PTT must not insert.
2. Settings, command palette, search, rename: PTT must not steal those fields.
3. After starting dictation on an editor, move focus to another panel before
   the transcript arrives: the editor must not reclaim keyboard focus.

## 8. Persistence / visual

- Screenshot at launch, after terminal insert, and after consecutive editor
  insert.
- Editor dirty indicator (`*`) after dictation.
- Isolated config is not written into the user's `~/.horizon`.

## 9. Cleanup

Close only the candidate window through the normal WM close path. Confirm the
candidate PID exited. Stop only the Xvfb, window manager, and loopback created
for this test. Remove the temporary config, virtual mic, and this plan unless
it is explicitly kept. Confirm the user's default sink/source and any
pre-existing Horizon PID are unchanged.
