# Markdown Todo Preview Smoke Test

## Goal

Validate that Horizon's markdown panel behaves like a todo-first viewer with a full rendered preview, clickable task list items, and no split/diff-style preview mode.

## Environment

- Build from the current worktree.
- Launch Horizon with a config that opens a file-backed markdown editor panel on startup.
- Use a markdown file that contains:
  - unchecked and checked task list items
  - nested task list items
  - headings
  - emphasis and strikethrough
  - a link
  - a table

## Baseline

1. Launch Horizon and confirm the window maps successfully.
2. Confirm the startup workspace is visible and the markdown panel opens without a crash.
3. Confirm the panel shows only `Edit` and `Preview` controls in the mode bar.
4. Confirm the initial preview fills the full panel body instead of rendering a split source/preview layout.

## Primary Flows

1. In preview mode, click an unchecked task item and confirm:
   - the checkbox becomes checked
   - the markdown source is updated
   - the panel remains interactive
2. Click the same task item again and confirm it becomes unchecked.
3. Toggle a nested task item and confirm nested list structure remains visually correct.
4. Switch to `Edit`, verify the markdown source reflects the toggles, then switch back to `Preview`.
5. If the panel is file-backed, restart Horizon and confirm the toggled state persists on disk.

## Edge Cases

1. Verify a markdown file with no task list items still renders normally in preview.
2. Verify an empty scratch markdown panel still opens and can switch to `Edit`.
3. Confirm a clicked link still renders as a link and does not break the preview state.
4. Confirm tables render as tables rather than collapsing into plain paragraph text.

## Resize And Visual Regression

1. Capture a screenshot immediately after launch.
2. Resize the markdown panel larger and smaller and confirm:
   - layout reflows without clipping or overlapping content
   - checkboxes remain aligned with their text
   - headings, lists, tables, and code blocks remain readable
3. Capture a second screenshot after the resize pass.

## Regression Check

1. Run workspace tests and the standard workspace validation commands used for code changes.
2. Confirm no new warnings or build failures were introduced by the markdown renderer swap.
