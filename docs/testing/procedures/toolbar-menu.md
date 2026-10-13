---
procedure: toolbar-menu
feature: The toolbar New workspace button and Menu
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Toolbar New workspace and Menu test procedure

## 1. Purpose

This procedure makes sure that the toolbar shows two controls at the right:
**+ New workspace** with a place part (**Cloud ▾** or **PC ▾**), and **Menu**.
It also makes sure that each menu row shows a title and a one-line description,
that each row does its action, and that the controls stay in the toolbar in a
narrow window.

## 2. Applicability

- Candidate: a build with the toolbar **Menu** button.
- Platforms: Linux. The toolbar is the same on macOS and Windows.
- This procedure does not test these functions:
  - The New cloud dialog. The
    [New cloud dialog procedure](cloud-panels/c-new-cloud-dialog.md) tests it.
  - The machine and hourly price under **Cloud**. Task C33 of the New cloud
    dialog procedure tests them with a cloud account.
  - A build or computer where cloud workspaces are not ready. Then the button
    shows **PC ▾**, makes a workspace on this computer, and the **Cloud** row is
    dimmed. The unit tests of the place menu cover it.
  - The words that a screen reader speaks. The unit tests of the menu rows
    check that a row gives its title, description, note and shortcut.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC CONTENT AND GENERIC WORKSPACE NAMES.
> Screenshots and recordings show the toolbar, the sidebar and the panels.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`, and a Horizon host
  with a Device panel for the fixture.
- `xdotool` and `import` (ImageMagick) for the fixture display.
- No cloud account in the private state of the fixture.

## 5. Setup

1. Start the fixture with the candidate. Open a Device panel for its
   `vnc_address`.

   Result: The Device panel shows the fixture desktop. The Horizon window shows
   the toolbar with **Horizon**, the search field, the FPS meter,
   **+ New workspace** with **Cloud ▾** and **Menu**.

2. Set the Horizon window to 1600 × 1000 pixels or the size of the fixture
   desktop.

   Result: The toolbar also shows the tagline after **Horizon**.

## 6. Tasks

Give each result to the report with the task ID.

### 6.1 NEW — New workspace and its place menu

1. Move the pointer onto **+ New workspace**.

   Result: The left part becomes lighter. A tooltip starts with
   **New workspace:** and the description of the default place.

2. Click **Cloud ▾**.

   Result: A menu opens under the button with the caption **NEW WORKSPACE** and
   two rows: **Cloud** with a cloud glyph and **This PC** with a screen glyph.
   Each row shows a one-line description. **Cloud GPU** is not in the menu.

3. Move the pointer over each row.

   Result: The row under the pointer gets a tinted background.

4. Click **This PC**.

   Result: The menu closes. A new empty workspace shows in the sidebar under
   **This PC**. No dialog opens.

5. Click **+ New workspace**.

   Result: The New cloud dialog opens for a new workspace. Click **Cancel**.
   The dialog closes and the new workspace goes again.

6. Click **Cloud ▾**, then press Escape.

   Result: The menu closes. No workspace is made.

### 6.2 MENU — The toolbar Menu

1. Click **Menu**.

   Result: A menu opens under the button. It shows these captions and rows, each
   row with a one-line description:
   - **GO TO**: **Quick Nav**, **Sessions** and **Remote Hosts**.
   - **CLOUD**: **New cloud**, **Cloud settings** and **Fit all clouds**.
   - **HORIZON**: **Settings**.

   The rows of Quick Nav, Sessions, Remote Hosts and Settings show their
   keyboard shortcut at the right. **Fit all clouds** is dimmed when the board
   has no cloud.

2. Click **Quick Nav**.

   Result: The menu closes and Quick Nav opens. Press Escape.

3. Open **Menu** and click **Sessions**.

   Result: The Sessions window opens. Close it.

4. Open **Menu** and click **Remote Hosts**.

   Result: The Remote Hosts chooser opens. Press Escape.

5. Open **Menu** and click **Settings**.

   Result: The Settings window opens. Close it.

6. Open **Menu** and click **Cloud settings**.

   Result: The cloud settings open. Close them.

7. Open **Menu** and click **New cloud**.

   Result: The New cloud dialog opens for the current workspace. Click
   **Cancel**.

### 6.3 NARROW — A narrow window

1. Set the Horizon window to 800 × 900 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The tagline goes. The FPS meter, **+ New workspace** and **Menu**
   stay. The search field stays between the brand and the controls. No control
   overlaps the search field.

2. Open **Menu** and the place menu again.

   Result: Both menus open in the window and show all their rows.

3. Set the window back to its first size.

   Result: The toolbar shows the tagline again.

### 6.4 KEYS — Keyboard focus

1. Click **Cloud ▾**, then press Escape.

   Result: The place menu closes.

2. Press Tab.

   Result: A focus ring shows around **+ New workspace** only.

3. Press Tab.

   Result: The focus ring moves to **Cloud ▾** only.

4. Press Tab.

   Result: The focus ring moves to **Menu**.

5. Press Enter.

   Result: The Menu opens. Press Escape.

### 6.5 LIGHT — Light theme

1. Open **Menu › Settings**. Under **Appearance**, choose **Light**.

   Result: The board uses the light theme. The text on **+ New workspace** stays
   readable on the accent color.

2. Open **Menu**, then the place menu.

   Result: Both menus use light colors. All titles, descriptions and shortcut
   chips are readable.

3. Under **Appearance**, choose the first theme again. Close Settings.

   Result: The board uses the first theme.

## 7. Pass criteria

- NEW makes a workspace on this computer from **This PC**, opens New cloud from
  **+ New workspace** when cloud workspaces are ready, and shows a description
  on each row.
- MENU opens each action from its row and shows the captions, descriptions and
  shortcuts as written.
- KEYS moves a focus ring over each part of the button and **Menu**, and Enter
  opens the focused menu.
- LIGHT shows readable controls and menus in the light theme.
- NARROW keeps **+ New workspace** and **Menu** in the toolbar in a narrow
  window.

## 8. Cleanup

1. Close the Horizon window of the fixture. Stop the fixture with SIGTERM on the
   process ID of its `serve.py`. Close the Device panel.

   Result: The fixture processes stop. Your own Horizon and terminals are not
   changed.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
