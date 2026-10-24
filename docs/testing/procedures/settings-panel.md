---
procedure: settings-panel
feature: Settings panel layout
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Settings panel test procedure

## 1. Purpose

This procedure shows that the Settings panel keeps its width and stays beside
the canvas. It also shows that wide content in the panel scrolls inside the
panel and does not draw over the canvas.

## 2. Applicability

- Candidate: any build of Horizon with a Settings panel.
- Platforms: Linux, macOS and Windows. Step 6.4 also needs X11 to send the key
  and pointer events from a terminal.
- This procedure does not test: the settings themselves, the settings store and
  the export of the configuration. Use
  [the settings store procedure](settings-store.md) for those.

## 3. Safety

None of the steps deletes data, rents compute or changes access.

## 4. Equipment and preconditions

- A Horizon build that starts with `--ephemeral`.
- A synthetic configuration file. Do not use the configuration of a real person:
  a report and a recording of this procedure can become public. Give the file
  `version: 11`, a window of 1600 x 1000 pixels, and at least one line of 100
  characters or more, for example
  `default_workspace: 'a-very-long-workspace-name-a-very-long-workspace-name-...'`
  under `remote_hosts`. Use words that name no real host, person or path.
- A free X display number. This procedure uses `:93`. Step 6.4 needs `Xvfb`,
  `openbox` and `xdotool`.

## 5. Setup

1. Start a private desktop and a private home for the run.

   Result: A display at 1600 x 1000 pixels and a home that holds only the
   synthetic configuration file, with an empty `sessions` folder.

2. Start the candidate with `--ephemeral` and the private home.

   Result: The Horizon window opens at the full size of the display.

## 6. Tasks

### 6.1 S1 — The panel opens beside the canvas

1. Press the Settings shortcut. The default is `Ctrl+Shift+Comma`.

   Result: The Settings panel opens on the right side. The General tab is
   active. The canvas is visible on the left of the panel, with its dot grid
   and its glow.

2. Read the left edge of the panel.

   Result: The panel takes about 30 percent of the window width. The canvas
   keeps the rest.

### 6.2 S2 — The YAML tab keeps the panel width

1. Select the `YAML` tab.

   Result: The panel shows the highlighted YAML of the configuration file. The
   panel keeps the width that it had on the General tab. The canvas stays
   visible on the left.

2. Read the tab row.

   Result: Each tab is in the panel. The panel does not push the tabs off its
   left edge.

### 6.3 S3 — A long line scrolls inside the panel

1. Point at the YAML text and keep `Shift` down while you turn the wheel down
   three or more times.

   Result: The text moves to the left and shows the end of the long lines. A
   horizontal scroll bar shows at the bottom of the editor. The panel width
   does not change and the canvas stays visible.

2. Turn the wheel up without `Shift`.

   Result: The text moves up and down. The panel width does not change.

### 6.4 S4 — The panel follows a resize

1. Resize the window to 1000 x 700 pixels.

   Result: The panel keeps a width between 15 and 50 percent of the window. The
   canvas is visible beside it.

2. Select the `General` tab, set `Height` to a new value, and click `Save` in
   the Settings bar. The value alone is only a live preview.

   Result: The control answers. The bar shows `Revert` while the value is not
   saved, and `Saving` and then `Saved` after the click. The panel width does
   not change.

## 7. Cleanup

1. Close the Horizon window through its normal close path.

   Result: The process exits.

2. Stop the private desktop and remove the private home.

   Result: No fixture process and no fixture file remain.

## 8. Pass criteria

- The panel keeps its width on every tab, with long lines in the editor.
- Wide content scrolls inside the panel in both directions.
- The canvas stays visible beside the panel at every window size.
