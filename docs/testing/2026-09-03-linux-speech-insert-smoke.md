# Linux Speech Insertion Smoke Test

Temporary validation for desktop dictation into Horizon terminals and apps
that do not expose an AT-SPI editable field (Microsoft Teams / Chromium PWAs).

Run against the exact candidate head. Do not stop or reuse a pre-existing
Horizon process. Use an isolated config and `--ephemeral`. Never log
transcripts, field contents, window titles, or credentials.

## Safety contract

- Direct insertion must not use the clipboard or press Return.
- Password fields must still refuse insertion.
- Scope automation to the exact candidate PID.

## 1. Build

```bash
RUSTFLAGS="-D warnings" cargo test -p horizon-cursor -p horizon-ui --features speech
cargo build --features speech
```

## 2. Horizon panel (PTY path)

1. Launch the candidate with `desktop_injection: true` and hold-mode PTT.
2. Focus a terminal panel in the candidate window.
3. Hold the configured hotkey, speak a short unique marker, release.
4. Confirm the marker appears in the focused panel and no speech-insert error toast is shown.
5. Repeat with the pointer over empty canvas chrome while the same panel remains the board-focused terminal: the marker must still land in that panel, not an AT-SPI error.

## 3. Unclassifiable Chromium / Teams targets

1. Focus a Teams (or other Chromium PWA) message box.
2. Hold PTT from a background Horizon with `desktop_injection: true`.
3. Confirm insertion is refused (no text appears, no Return is sent) because the app does not expose a classifiable AT-SPI text field.
4. Confirm the clipboard is unchanged.

## 4. Refusals

1. Focus a password field and dictate: insertion must fail closed without typing.
2. Select text in a GTK entry that exposes AT-SPI EditableText and dictate: insertion must refuse rather than replace the selection.

## 5. Cleanup

Close only the candidate Horizon window. Remove the isolated config and this
plan after the validation pass unless it is explicitly kept.
