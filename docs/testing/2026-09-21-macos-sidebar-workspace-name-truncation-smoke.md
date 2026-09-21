# macOS sidebar workspace name truncation smoke test

Validation record for the sidebar workspace-name truncation fix. Before the
fix, a long workspace name in the flat (non-accordion) sidebar was rendered
with no width cap and ran past the sidebar edge, while the panel rows beneath
it truncated correctly.

## Safety contract

- Never touch a pre-existing Horizon process or `~/github/horizon`; use a
  task-owned worktree and binary only.
- Launch with `--config <board>.yaml --ephemeral` so the session store is
  never written.

## Board

```yaml
version: 11
workspaces:
  - name: Cloud MVP — temporary viewer with a very long workspace name
    terminals:
      - name: VNC verification controller notes
        kind: editor
        position: [80, 80]
        size: [520, 340]
      - name: Device 127.0.0.1:40003
        kind: editor
        position: [80, 460]
        size: [520, 300]
  - name: Short
    terminals:
      - name: notes-b
        kind: editor
        position: [700, 120]
        size: [560, 380]
```

## Lanes

1. Flat sidebar (default). Expect the long workspace name to end in an
   ellipsis inside the row background and never cross the sidebar edge.
   Expect `Short` to stay flush left after the color bar, not centered.
2. Detached workspace. Right-click a workspace row, choose "Open in New
   Window". Expect the truncated name to leave room for the `NEW WINDOW`
   badge on the same row.
3. Accordion sidebar (Settings, sidebar accordion on). Expect the name to
   truncate before the panel count badge exactly as before the fix.
4. Clicking the truncated name still selects the workspace.

## Linux headless record (2026-09-21)

Xvfb `:97` with openbox, `target/debug/horizon --config board.yaml
--ephemeral`, driven with xdotool. Baseline (main, release binary): the long
name overflowed the sidebar edge.

- Lane 1 (flat): the name truncated to `Cloud MVP — temporary ...` inside the
  row background and `Short` stayed flush left. PASS.
- Lane 3 (accordion via `features.sidebar_accordion: true`): the name
  truncated before the panel-count badge (`Cloud MVP — tempo... 2`) and
  `Short 1` stayed flush left. PASS.
- Lane 4 (click): a left click on the truncated `Short` name made it the
  active workspace, focused `notes-b`, and panned the canvas to it. PASS.
- Lane 2 (detached): the row context menu opened but was painted behind the
  sidebar on this headless display, so the `Open in New Window` click could
  not be delivered. Needs a real desktop.

SMOKE-TEST: PENDING (macOS lane 2)
