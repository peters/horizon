---
procedure: settings-panel
feature: Settings panel layout
platforms: [linux, macos]
cost: none
destructive: no
secrets: [a marker value of the test only]
owner: peters
---

# Settings panel test procedure

## 1. Purpose

This procedure shows that the Settings panel keeps its width and stays beside
the canvas. It also shows that wide content in the panel scrolls inside the
panel and does not draw over the canvas. It shows that no tab paints over the
panel and that the canvas stops at the panel.

## 2. Applicability

- Candidate: any build of Horizon with a Settings panel.
- Platforms: Linux and macOS. Step 6.4 also needs X11 to send the key and
  pointer events from a terminal.
- This procedure does not test: the settings themselves and the settings store.
  Use [the settings store procedure](settings-store.md) for those.

## 3. Safety

None of the steps deletes data, rents compute or changes access.

## 4. Equipment and preconditions

- A Horizon build that starts with `--ephemeral`.
- A synthetic settings file. Do not use the settings of a real person: a report
  and a recording of this procedure can become public. Give the file
  `version: 11`, a window of 1600 x 1000 pixels, and a value of 100 characters or
  more, for example a `remote_hosts.default_workspace` of
  `a-very-long-workspace-name-a-very-long-workspace-name-...`. Use words that name
  no real host, person or path.
- A free X display number. This procedure uses `:93`. Step 6.4 needs `Xvfb`,
  `openbox` and `xdotool`.

## 5. Setup

1. Start a private desktop and a private home for the run.

   Result: A display at 1600 x 1000 pixels and a home that holds only the
   synthetic settings file, with an empty `sessions` folder.

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

### 6.2 S2 — Every tab keeps the panel width

1. Select each tab in turn: General, Keys, Presets, Browser, Remote, Secrets,
   Server and Speech.

   Result: The panel keeps the width that it had on the General tab. The canvas
   stays visible on the left. There is no tab of a text format.

2. Read the tab row.

   Result: The eight tabs are on one row. No tab label wraps onto a second line,
   and the panel does not push the tabs off its left edge.

3. Read the left edge of the panel and the right edge of the canvas on each
   tab. Read the first character of the tab row, which is `General`.

   Result: The left edge of the panel is at the same place on all six tabs. The
   canvas stops at that edge. `General` shows in full, and no strip of the
   canvas covers the panel.

4. Select the `Secrets` tab. Read the heading.

   Result: The heading reads `Secrets for agents` in full. No strip of the
   canvas covers it.

### 6.2b S2b — A row of the Secrets tab fits the panel

> **CAUTION:** THIS TASK STORES TWO MARKER VALUES. Use the private home of this
> run only, and use values that are not a key of a provider.

1. In the `Secrets` tab, type `HZ_DEMO_TOKEN` in the `NAME` box and
   `marker-value-not-a-key` in the value box. Click `Add`.

   Result: One row appears, with the name on the left and `Stored on this
   computer` on the same line. The two buttons `Replace value` and `Delete`
   are on the line under the name and the state.

2. Read the row that adds a name. Read the `Add` button.

   Result: The `NAME` box, the value box and `Add` are on one line. All three
   stay inside the panel, and `Add` is at the right end of the row.

3. Add a second name of 64 characters: `AAAA` ten times, then `AAAA` six times.
   Use the same marker value.

   Result: The second row appears. Its name is cut with an ellipsis, and a
   pointer on the name shows the whole name. The state text stays on its line,
   and the two buttons stay on the line under it. The panel width does not
   change, and the canvas stays visible on the left.

4. Put the pointer on the state text of a row, then on the name of the second
   row.

   Result: A cut text shows its whole text. A text that the panel does not cut
   shows no tooltip.

### 6.2a S2a — A port for one host names its field

1. Select the `General` tab and read **Remote hosts** → the row under
   **A port for one host, by its label or its SSH host name:**.

   Result: The host box of the empty row is about three times as wide as its
   port box, and its hint says `Host label or SSH host name`.

2. Type `smoke-node` in the host box and `5997` in its port box.

   Result: The host name is readable in the box. `Remove` appears at the end of
   the row, and the row stays inside the panel.

### 6.3 S3 — Long content scrolls inside the panel

1. In General, write the long name of the synthetic settings into **Remote
   hosts** → **Workspace**, and keep `Shift` down while you turn the wheel down
   three or more times.

   Result: The field shows the end of the long value, and the panel scrolls.
   The panel width does not change and the canvas stays visible. No field makes
   the panel wider.

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

- The panel keeps its width on every tab, with long lines in the editor. The
  tabs are General, Keys, Presets, Browser, Remote, Secrets, Server and Speech.
- The canvas stops at the left edge of the panel on every tab. No strip of the
  canvas covers the tab row, a heading, a row of the Secrets tab, or a field.
- Wide content scrolls inside the panel in both directions.
- The canvas stays visible beside the panel at every window size.
