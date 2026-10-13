---
procedure: toolbar-menu
feature: The toolbar New cloud and New workspace buttons and Menu
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Toolbar New cloud, New workspace and Menu test procedure

## 1. Purpose

This procedure makes sure that the toolbar shows three controls at the right:
**+ New cloud**, **+ Workspace** and **Menu**. It also makes sure that each
button does one action without a menu, that each Menu row shows a title and a
one-line description, that each row does its action, and that the controls stay
in the toolbar in a narrow window.

## 2. Applicability

- Candidate: a build with the toolbar **Menu** button.
- Platforms: Linux. The toolbar is the same on macOS and Windows.
- This procedure does not test these functions:
  - The New cloud dialog. The
    [New cloud dialog procedure](cloud-panels/c-new-cloud-dialog.md) tests it.
  - The machine and hourly price in the tooltip of **+ New cloud**. They need
    a cloud account.
  - A build or computer where cloud workspaces are not ready. Then
    **+ New cloud** is dimmed, or not in a build without cloud workspaces. The
    dimmed button takes no click and no keyboard focus, and its tooltip says why.
    The unit tests of the toolbar cover it.
  - **New Cloud** and **New Workspace** on the empty canvas. The canvas shows
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
- No cloud account in the private state of the fixture.

## 5. Setup

1. Start the fixture with the candidate. Open a Device panel for its
   `vnc_address`.

   Result: The Device panel shows the fixture desktop. The Horizon window shows
   the toolbar with **Horizon**, the search field, the FPS meter,
   **+ New cloud**, **+ Workspace** and **Menu**.

2. Set the Horizon window to 1600 × 1000 pixels or the size of the fixture
   desktop.

   Result: The toolbar also shows the tagline after **Horizon**.

## 6. Tasks

Give each result to the report with the task ID.

### 6.1 NEW — New cloud and New workspace

1. Move the pointer onto **+ New cloud**.

   Result: The button becomes lighter. A tooltip says that the new workspace
   gets a cloud worker.

2. Click **+ New cloud**.

   Result: No menu opens. The New cloud dialog opens for a new workspace.

3. Click **Cancel**.

   Result: The dialog closes and the new workspace goes again.

4. Move the pointer onto **+ Workspace**.

   Result: The button becomes lighter. A tooltip says that the new workspace is
   empty and runs on this computer.

5. Click **+ Workspace**.

   Result: No menu and no dialog open. A new empty workspace shows in the
   sidebar under **This PC**.

6. Click **New** in the sidebar header.

   Result: No menu opens. One more empty workspace shows under **This PC**.

### 6.2 MENU — The toolbar Menu

1. Click **Menu**.

   Result: A menu opens under the button. It shows these captions and rows, each
   row with a one-line description:
   - **GO TO**: **Quick Nav**, **Sessions** and **Remote Hosts**.
   - **CLOUD**: **Add cloud here**, **Cloud settings** and **Fit all clouds**.
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

7. Open **Menu** and click **Add cloud here**.

   Result: The New cloud dialog opens for the current workspace. Click
   **Cancel**.

### 6.3 NARROW — A narrow window

1. Set the Horizon window to 800 × 900 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The tagline goes. The FPS meter, **+ New cloud**, **+ Workspace**
   and **Menu** stay. The search field stays between the brand and the controls. No control
   overlaps the search field.

2. Open **Menu**.

   Result: The menu opens in the window and shows all its rows. Press Escape.

3. Set the window back to its first size.

   Result: The toolbar shows the tagline again.

### 6.4 KEYS — Keyboard focus

1. Click an empty part of the toolbar.

   Result: No control has the focus.

2. Press Tab.

   Result: A focus ring shows around **+ New cloud** only.

3. Press Tab.

   Result: The focus ring moves to **+ Workspace** only.

4. Press Tab.

   Result: The focus ring moves to **Menu**.

5. Press Enter.

   Result: The Menu opens. Press Escape.

### 6.5 LIGHT — Light theme

1. Open **Menu › Settings**. Under **Appearance**, choose **Light**.

   Result: The board uses the light theme. The text on **+ New cloud** stays
   readable on the accent color. The text on **+ Workspace** is readable.

2. Open **Menu**.

   Result: The menu uses light colors. All titles, descriptions and shortcut
   chips are readable.

3. Under **Appearance**, choose the first theme again. Close Settings.

   Result: The board uses the first theme.

## 7. Pass criteria

- NEW opens New cloud for a new workspace from **+ New cloud**, and makes an
  empty workspace on this computer from **+ Workspace** and from **New** in the
  sidebar. No button opens a menu.
- MENU opens each action from its row and shows the captions, descriptions and
  shortcuts as written.
- KEYS moves a focus ring over each button and **Menu**, and Enter opens the
  Menu.
- LIGHT shows readable controls and menus in the light theme.
- NARROW keeps **+ New cloud**, **+ Workspace** and **Menu** in the toolbar in
  a narrow window.

## 8. Cleanup

1. Close the Horizon window of the fixture. Stop the fixture with SIGTERM on the
   process ID of its `serve.py`. Close the Device panel.

   Result: The fixture processes stop. Your own Horizon and terminals are not
   changed.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
