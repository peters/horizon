# Codex Alt-Screen Smoke Test Plan

## Purpose

Validate the Codex render fixes that remove stale sparse alt-screen reuse and keep fullscreen Codex layouts synchronized during repeated resize.

## Setup

1. Build the investigation worktree:
   - `cd /home/peters/github/horizon-codex-render-investigation`
   - `cargo run --release`
2. Ensure the Horizon session can launch both:
   - a default `Codex` preset
   - a custom Codex panel without `--no-alt-screen`, for example:
     - command: `codex`
     - args: `-c tui.alternate_screen=always`
3. Keep one non-Codex fullscreen TUI available for regression comparison:
   - `fzf`
   - `sk`
   - `vim`
   - `less`

## Baseline Checks

1. Launch Horizon and confirm the window paints correctly at startup.
2. Open a normal shell panel and verify text rendering, cursor rendering, and scrollback are unchanged.
3. Open the default `Codex` preset and confirm it still starts in inline mode because the preset keeps `--no-alt-screen`.
4. Confirm the default Codex preset preserves scrollback and does not regress from current behavior.

## Primary Flow

1. Open a Codex panel with alternate screen enabled.
2. Watch the first 5 to 10 seconds of startup.
3. Verify the panel does not show stale previous dense content while Codex is transitioning into its fullscreen UI.
4. Send a prompt and watch for redraw artifacts during:
   - initial response rendering
   - status/header updates
   - tool output insertion
   - focus changes between panel and other UI
5. Resize the Codex panel slowly and then rapidly.
6. While Codex shows its bottom composer/status area, keep resizing from alternating edges and corners for 5 to 10 seconds.
7. Verify the bottom composer or status bar stays visually attached to the active content instead of leaving a large stale blank band above it.
8. Confirm newly typed characters still appear on the active prompt line after the repeated resize sequence.
9. Toggle panel fullscreen if available and repeat prompt/response rendering.
10. Capture a screenshot after launch and another during active redraw.

## Regression Comparison

1. Open `fzf` or `sk` in another panel and trigger rapid filtering on a large candidate set.
2. Verify the original sparse-frame protection still works for non-Codex panels:
   - no blank middle-viewport flash
   - candidate list stays visually stable during rapid typing
3. Open `vim` or `less` in fullscreen and confirm normal alternate-screen rendering remains intact.
4. Open Claude Code and confirm its fullscreen rendering still behaves as before.

## Edge Cases

1. With Codex alt-screen enabled, switch focus repeatedly between panels and the sidebar/settings UI.
2. Drag the Codex panel while it is actively repainting.
3. Hover and drag the scrollbar during active Codex output.
4. Select terminal text in the Codex panel and confirm selection rendering still works.
5. Let Codex sit idle, then wake it with input and confirm there is no stale cached frame.
6. Compare a normal shell panel against the alt-screen Codex panel during rapid resize and confirm the shell still avoids resize-flood regressions while Codex reflows immediately.

## Persistence And Resume

1. Save or persist a session state that includes:
   - one default Codex panel
   - one alt-screen Codex panel
   - one non-Codex fullscreen TUI panel
2. Restart Horizon.
3. Confirm all panels restore as expected.
4. Verify the default Codex preset still resumes in inline mode and the alt-screen Codex panel still uses the intended command/config.

## Visual Regression Checklist

1. No stale previous-frame content remains visible in Codex after fullscreen transitions.
2. No new blank-viewport flashes appear in `fzf`/`sk`.
3. Codex does not leave a large blank gap between the conversation/output area and the bottom composer after repeated resizes.
4. Cursor, colors, and scrollbar remain aligned after repeated resizes.
5. Panel chrome and canvas interactions remain unchanged outside the terminal body.

## Evidence To Record

1. Horizon commit SHA under test.
2. OS and compositor/window manager.
3. Codex CLI version.
4. Whether the test used the default preset or explicit `tui.alternate_screen=always`.
5. Screenshots for:
   - launch
   - active redraw
   - repeated resize with bottom composer visible
   - non-Codex regression comparison
