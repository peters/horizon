---
procedure: toolbar-menu
feature: The toolbar Menu and the sidebar New button
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Toolbar Menu test procedure

## 1. Purpose

This procedure makes sure that the toolbar shows **Menu** at the right. It also
makes sure that each Menu row shows a title and a one-line description, that
each row does its action, and that the control stays in the toolbar in a narrow
window.

## 2. Applicability

- Candidate: a build with the toolbar **Menu** button.
- Platforms: Linux. The toolbar is the same on macOS.
- This procedure does not test these functions:
  - **New Workspace** on the empty canvas. The canvas shows
    them only on a board without workspaces, and the fixture always has a
    workspace. The unit tests of the empty canvas cover them.
  - The words that a screen reader speaks. The unit tests of the menu rows
    check that a row gives its title, description and shortcut.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC CONTENT AND GENERIC WORKSPACE NAMES.
> Screenshots and recordings show the toolbar, the sidebar and the panels.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`, and a Horizon host
  with a Device panel for the fixture.
- `xdotool` and `import` (ImageMagick) for the fixture display.

## 5. Setup

1. Start the fixture with the candidate. Open a Device panel for its
   `vnc_address`.

   Result: The Device panel shows the fixture desktop. The Horizon window shows
   the toolbar with **Horizon**, the search field, the FPS meter and **Menu**.

2. Set the Horizon window to 1600 × 1000 pixels or the size of the fixture
   desktop.

   Result: The toolbar also shows the tagline after **Horizon**.

## 6. Tasks

Give each result to the report with the task ID.

### 6.1 NEW — New workspace
1. Click **New** in the sidebar header.

   Result: No menu opens. One more empty workspace shows under **This PC**.
   The toolbar has no button for an empty workspace.

2. Make a panel of the first workspace wide: drag its right edge to about three
   times the width of a panel, then click **New** again.

   Result: The new workspace stands on free canvas space beside the frame of the
   first workspace. Its name is not over a panel of the first workspace, and no
   panel of the two workspaces overlaps.

### 6.2 MENU — The toolbar Menu

1. Click **Menu**.

   Result: A menu opens under the button. It shows these captions and rows, each
   row with a one-line description:
   - **GO TO**: **Quick Nav**, **Sessions** and **Remote Hosts**.
   - **HORIZON**: **Settings**.

   The rows of Quick Nav, Sessions, Remote Hosts and Settings show their
   keyboard shortcut at the right.

2. Click **Quick Nav**.

   Result: The menu closes and Quick Nav opens. Press Escape.

3. Open **Menu** and click **Sessions**.

   Result: The Sessions window opens. Close it.

4. Open **Menu** and click **Remote Hosts**.

   Result: The Remote Hosts chooser opens. Press Escape.

5. Open **Menu** and click **Settings**.

   Result: The Settings window opens. Close it.

### 6.3 NARROW — A narrow window

1. Set the Horizon window to its minimum width of 800 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The tagline stays after **Horizon**. The FPS meter and **Menu**
   stay. The search field gets narrower and cuts its placeholder text. No
   control overlaps the search field.

2. Open **Menu**.

   Result: The menu opens in the window and shows all its rows. Press Escape.

3. Set the window back to its first size.

   Result: The toolbar shows the tagline again.

### 6.4 KEYS — Keyboard focus

1. Click an empty part of the toolbar.

   Result: No control has the focus.

2. Press Tab.

   Result: A focus ring shows around **Menu** only.

3. Press Enter.

   Result: The Menu opens. Press Escape.

### 6.5 LIGHT — Light theme

1. Open **Menu › Settings**. Under **Appearance**, choose **Light**.

   Result: The board uses the light theme. The **Menu** label is readable.

2. Open **Menu**.

   Result: The menu uses light colors. All titles, descriptions and shortcut
   chips are readable.

3. Under **Appearance**, choose the first theme again. Close Settings.

   Result: The board uses the first theme.

## 7. Pass criteria

- NEW makes an empty workspace on this computer from **New** in the sidebar.
  The control opens no menu, and the new workspace stands beside the panels of
  every workspace that is already there.
- MENU opens each action from its row and shows the captions, descriptions and
  shortcuts as written.
- KEYS moves a focus ring over **Menu**, and Enter opens the Menu.
- LIGHT shows readable controls and menus in the light theme.
- NARROW keeps **Menu** in the toolbar in a narrow window.

## 8. Cleanup

1. Close the Horizon window of the fixture. Stop the fixture with SIGTERM on the
   process ID of its `serve.py`. Close the Device panel.

   Result: The fixture processes stop. Your own Horizon and terminals are not
   changed.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
