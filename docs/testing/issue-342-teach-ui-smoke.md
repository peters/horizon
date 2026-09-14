# Teach-mode UI smoke (temporary)

Candidate: the exact `horizon` binary from this PR branch (`target/debug/horizon`).
Use `--config <temp-config> --ephemeral` and unset `HORIZON` for the child.

## Baseline

1. Launch Horizon. Open or create a browser panel.
2. Confirm the usual chrome (back/forward/reload, URL bar, backend picker) still renders.
3. Confirm no Teach banner is visible before Teach is started.

## Start / banner

1. Right-click the browser panel titlebar and choose **Teach routine**.
2. A cyan Teach banner appears with status **Teaching**, a routine name field (default `untitled`), **Pause**, **Stop**, and **Discard**.
3. The agent ownership chip, if present, still renders; the Teach banner does not replace the URL bar.

## Record / preview

1. With Teach active, left-click a clearly labeled control on a deterministic local page (or `https://example.com`).
2. The banner step preview shows a `click …` line using the accessible name when one exists.
3. Pointer movement alone does not add steps.
4. A failed capture (if you click a cross-origin iframe) shows a red error on the banner and does not append a step.

## Pause / resume / stop / discard

1. **Pause** changes the status to **Teach paused** and further clicks do not append steps.
2. **Resume** returns to **Teaching** and a later click appends another step.
3. **Stop** changes the status to **Teach stopped**, shows the outcome picker, and further clicks do not append steps. Resume from Stop is a no-op.
4. **Discard** removes the banner. Starting Teach again from the context menu creates a fresh session.

## Outcome picker

1. After **Stop**, if the page has a title, the **Outcome: &lt;title&gt;** checkbox is available.
2. The optional **Heading** field accepts extra completion text and rejects nothing visually except by staying empty when unused.
3. Uncheck the title outcome; the heading field still works.

## Persistence / recovery

1. Start Teach, record one click, Pause (draft is saved under the private routine store).
2. Discard and confirm the banner is gone.
3. Do not inspect other panels’ profiles; discarding Teach must not close the browser panel.

## Visual / regression

1. Screenshot: launch, Teach banner while recording, paused, stopped with outcome picker.
2. Resize the panel while Teaching; the banner stays a single-line strip and does not cover the page frame the way a wrapping handoff banner used to.
3. Detached browser window: Teach menu still starts a banner on that panel only.

Delete this file after the validation pass unless asked to keep it.
