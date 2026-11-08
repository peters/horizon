# Claude panel rendering and scrollback

A Claude panel runs Claude Code in the normal screen buffer. Horizon sets
`CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1` for `PanelKind::Claude` in
`crates/horizon-core/src/panel/spawn.rs`. The test
`claude_uses_horizon_native_scrollback` holds this behavior. A Codex panel and a
shell panel do not get the variable.

Horizon makes this choice so that the panel history, the scrollbar, the history
meter and the board search work on the conversation. An application in the
alternate screen has no host scrollback. Its content is not in the panel history
and the board search cannot find it.

## What the choice costs

The alternate screen is the default of Claude Code. The inline renderer of
Claude Code is the mode with a known defect: a redraw after a width change
leaves a copy of the old frame in the scrollback. The renderer moves the cursor
up by the number of logical lines that it wrote. It does not count the rows that
these lines occupy after a reflow. The terminal limits the cursor movement at
the top of the viewport, so the erase does not reach the rows that moved into
the history. The application writes a new frame below them.

This defect is in Claude Code, not in Horizon. The repository
[anthropics/claude-code](https://github.com/anthropics/claude-code) has more
than one open issue for it. Issue
[84247](https://github.com/anthropics/claude-code/issues/84247) is the most
recent one.

## Measured behavior

Measurements on Claude Code 2.1.319 in a 100 x 30 terminal, with an answer of 60
lines:

| Condition | Duplicated lines | Maximum copies |
|---|---|---|
| No resize | 0 of 60 | 1 |
| Drag resize, 20 steps from 100 to 60 columns | 31 of 60 | 4 |

A resize is the only trigger. An answer that is higher than the panel does not
duplicate its lines without a resize, because Claude Code writes a completed
message one time.

A width increase does not duplicate lines. A width decrease does.

## Options that do not work

Two repairs were measured and discarded. Record them here, because each one
looks correct before measurement.

**Lockstep resize.** Hold the grid at the old geometry until the debounced PTY
resize sends the new one, so that a drag reflows one time for each debounce tick
instead of one time for each frame. This gives no improvement: 6 maximum copies
with the change and 6 without it. The count of copies follows the count of
redraws of the application, and Claude Code limits its own redraw rate. The
reflow rate of the grid does not control it.

**Duplicate removal on read.** Remove the repeated bands when Horizon reads the
scrollback. This does not reach the rendered history. The terminal widget reads
`display_iter` from the alacritty grid, and the scrollbar and the history meter
read `grid.total_lines()`. Only the board search and the cloud progress reader
use `Terminal::full_text_lines`. The field `Grid.raw` and the module
`storage` of `alacritty_terminal` 0.26 are private. The public API has
`clear_history` and `update_history` only, so there is no operation that removes
one band. A removal in `full_text_lines` would make the search count disagree
with the rows on the screen.

## The option that works

Do not send an intermediate geometry to the PTY while a drag is active. Send the
final geometry one time when the drag commits. The UI already has this commit
point: `commit_terminal_resize` in `crates/horizon-ui/src/app/panels/interaction.rs`
calls `resize_immediately`.

Measured with a proxy that models the renderer of Claude Code: 6 maximum copies
before the change and 2 after it, and 428 scrollback lines before and 151 after.
Two copies is the minimum, because the last redraw always leaves one band.

The cost is that the text does not reflow until the person releases the pointer.
Today it reflows one time for each debounce tick of 80 ms. This change was not
made, because the defect is small and the cost is visible at each drag.

## The overridden setting

`CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN=1` has a higher precedence than the key
`tui` in `~/.claude/settings.json`. A person who sets `"tui": "fullscreen"` gets
an inline Claude panel, and Horizon does not report the override. This is the
intended default, because a fullscreen panel loses the panel history, the
scrollbar and the board search. Keep the override in mind when a person asks why
the setting has no effect.
