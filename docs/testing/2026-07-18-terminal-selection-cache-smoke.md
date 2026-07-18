# Terminal Selection Cache Smoke Test

## Goal

Verify that copying or cutting a terminal selection never leaves a stale
selection highlight on screen. Before this fix the copy frame stored
selection-highlighted shapes in the terminal grid cache, the copy handler then
cleared the model selection after rendering, and the next quiet frame replayed
the highlighted shapes from the cache until output, scrolling, or a resize
invalidated it.

## Fix Under Test

`crates/horizon-ui/src/terminal_widget/render.rs`: `render_grid` no longer
stores shapes built while a selection is active; it invalidates the cache
instead, so the first frame after the selection clears rebuilds from live
terminal content.

## Setup

- Build the debug binary: `cargo build -p horizon-ui`.
- Launch `target/debug/horizon` with an isolated `HOME` and isolated runtime
  state.
- Open one terminal panel, run a command that prints a few lines (`ls -la`),
  and let the terminal go fully idle so the grid cache is eligible.

## Primary Flow

- Select a word with the mouse; confirm the highlight renders.
- Press the platform copy shortcut (Cmd+C on macOS, Ctrl+Shift+C on Linux)
  while the pointer is still over the terminal.
- The highlight must disappear on the next frame, with the terminal otherwise
  idle. Confirm the copied text is on the clipboard.
- Repeat, but after selecting, move the pointer completely off the panel (onto
  empty canvas) before pressing copy while the panel keeps focus. The
  highlight must still clear immediately. This variant was the hardest failure
  path before the fix.

## Interaction Edge Cases

- Cut path: select text, press Ctrl+Shift+X / Cmd+X; highlight must clear (the
  terminal also receives the cut control byte, which is expected).
- Select text, then type a character: selection clears and the echoed output
  refreshes the grid; no ghost highlight.
- Select text, then paste: selection clears; pasted input renders normally.
- Select text, then click elsewhere in the terminal: highlight clears.
- Select text, copy, then immediately press copy again: nothing new is copied
  (selection already cleared) and no highlight reappears.
- Select text spanning scrollback, scroll away and back: no ghost highlight at
  either position.
- Select in one terminal, focus a second terminal, copy: the first terminal's
  highlight must not stick.

## Persistence

- No persisted state is involved; relaunch once and confirm terminals render
  normally.

## Visual and Performance Regression Checks

- With an idle terminal and the pointer parked over it, confirm no continuous
  repaint or CPU increase: cache reuse must still engage on quiet frames
  without a selection.
- Confirm selected-cell colors themselves are unchanged while a selection is
  active (this fix only changes what the cache retains).
- Stream output (`yes | head -c 100000`), then let it settle and re-run the
  primary flow; the first quiet frame after settling must show live content.

## Cleanup

- Stop only the Horizon PID launched for this test and remove the isolated
  home. Delete this temporary smoke-test plan after the validation pass.
