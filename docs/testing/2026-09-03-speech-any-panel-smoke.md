# Speech Into Any Horizon Panel Smoke Test

Temporary validation for dictation into terminal, editor, and browser panels.

Run against the exact candidate head. Do not stop or reuse a pre-existing
Horizon process. Use an isolated config and `--ephemeral`. Never log
transcripts, field contents, or credentials.

## 1. Build

```bash
RUSTFLAGS="-D warnings" cargo test -p horizon-core -p horizon-ui --features speech
cargo build --features speech
```

## 2. Terminal panel

1. Focus a shell panel.
2. Hold PTT, speak a unique marker, release.
3. Confirm the marker appears in the PTY and Return is not sent.

## 3. Editor panel

1. Open an editor panel in Edit mode, place the caret mid-buffer.
2. Hold PTT or click the title-bar mic, speak a unique marker, release.
3. Confirm the marker is inserted at the caret, not only at the end, and the
   buffer is dirty.

## 4. Browser panel

1. Open a browser panel on a page with a focused text field (no selection).
2. Click the title-bar mic, speak a unique marker, release.
3. Confirm the marker appears in the page field and the page does not submit.

## 5. Non-text panels

1. Focus Git Changes or Usage: no mic, PTT must not insert.
2. Focus Settings or the command palette: PTT must not steal those fields.

## 6. Cleanup

Close only the candidate Horizon window. Remove the isolated config and this
plan after the validation pass unless it is explicitly kept.
