---
procedure: toolbar-menu
feature: Root toolbar Menu and Dependencies button
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Toolbar Menu and Dependencies button test procedure

## 1. Purpose

This procedure tests the right side of the root toolbar: the fps meter, the
**Dependencies** button and the **Menu** button. It also tests the commands in
the menu, their shortcuts, the keyboard path and the toolbar at the minimum
window width.

## 2. Applicability

- Candidate: a Horizon build that contains the toolbar menu.
- Platforms: Linux, with the local device fixture.
- This procedure does not test the Dependencies panel. If the candidate does
  not contain that panel, the **Dependencies** button does nothing.
- This procedure does not test windows narrower than 800 pixels. The window
  cannot be narrower. Unit tests in `crates/horizon-ui/src/app/root_chrome.rs`
  cover the narrower widths.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED DESKTOP.** A click on the developer desktop
> can change a real session.

## 4. Equipment and preconditions

- A frozen Horizon candidate and its SHA-256.
- The local device fixture with `--native-view` and private application state.
- A live Device panel in the current workspace.
- A workspace with one terminal panel in the candidate.

## 5. Setup

1. Start the fixture with the frozen candidate and `--native-view`.

   Result: The fixture prints a loopback VNC address.

2. Open that address in a Device panel.

   Result: The panel shows the isolated Horizon window.

3. Set the candidate window to 1280 × 800 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 1280 800
   ```

   Result: The window is 1280 pixels wide.

## 6. Tasks

### 6.1 LAYOUT — Toolbar items

1. Examine the root toolbar from left to right.

   Result: You see the app name, the tagline, the search field, the fps meter,
   **Dependencies** and **Menu**. You do not see **Quick Nav**, **Remote
   Hosts**, **Cloud**, **Sessions**, **Settings** or **More** in the toolbar.

2. Examine the **Dependencies** button.

   Result: The button has an accent fill and outline. A mark with three linked
   dots, one on the left and two on the right, comes before the label.

3. Examine the **Menu** button.

   Result: The button has the same neutral look as other chrome buttons. A mark
   with three horizontal lines comes before the label.

4. Put the pointer on **Menu** and wait one second.

   Result: The tooltip shows "Quick Nav, Remote Hosts, Cloud, Sessions and
   Settings".

### 6.2 MENU — Menu rows and shortcuts

1. Click **Menu**.

   Result: The menu opens below the toolbar edge. Its right edge aligns with
   the right edge of **Menu**.

2. Examine the rows from top to bottom.

   Result: The rows are Quick Nav, Remote Hosts, Cloud, Sessions, a separator
   line and Settings.

3. Examine the right side of each row.

   Result: Quick Nav shows `Ctrl+Shift+K`. Remote Hosts shows `Ctrl+Shift+H`.
   Sessions shows `Ctrl+Shift+J`. Settings shows `Ctrl+Shift+Comma`. Cloud
   shows an arrow.

4. Move the pointer onto Remote Hosts.

   Result: The row gets a hover fill. Its label and its shortcut become
   brighter.

5. Move the pointer onto Cloud.

   Result: The Cloud submenu opens. It shows **New cloud…**, **Cloud
   settings…** and **Fit all clouds**.

6. Push Escape.

   Result: The menu closes.

7. Click **Menu**, then **Settings**.

   Result: The Settings window opens. The menu closes.

8. Close Settings.

   Result: The board appears again.

### 6.3 SHORTCUT — Changed shortcut

1. Open Settings and click the **Shortcuts** tab.

   Result: The **Keyboard Shortcuts** section shows a field for each shortcut.

2. Set **Toggle Settings** to `Alt+S`. Click **Save**.

   Result: The configuration saves without an error.

3. Close Settings. Click **Menu**.

   Result: The Settings row shows `Alt+S`.

4. Push Escape. Set **Toggle Settings** back to `Ctrl+Shift+Comma` and save.

   Result: The Settings row shows `Ctrl+Shift+Comma` again.

### 6.4 KEYS — Keyboard path

1. Push Tab until **Menu** shows the focus ring.

   Result: **Menu** has keyboard focus.

2. Push Enter.

   Result: The menu opens.

3. Push Tab until the Quick Nav row has focus.

   Result: The Quick Nav row shows the hover fill and the brighter label.

4. Push Enter.

   Result: The command palette opens. It stays open, and it does not run a
   command.

5. Push Escape.

   Result: The command palette closes.

### 6.5 DEPS — Dependencies entry points

1. Click **Dependencies**.

   Result: The Dependencies panel opens. If the candidate does not contain the
   panel, nothing changes and no error shows.

2. Push Ctrl+Shift+K and type `dependabot`.

   Result: The palette shows **Open Dependencies**. The row has no shortcut
   badge.

3. Push Escape.

   Result: The command palette closes.

### 6.6 NARROW — Minimum window width

1. Set the candidate window to 800 × 900 pixels.

   ```sh
   DISPLAY=<display> xdotool search --pid <child-pid> --name '^Horizon$' windowsize %1 800 900
   ```

   Result: The toolbar shows the search field, the fps meter, **Dependencies**
   and **Menu**. The items do not overlap.

2. Open **Menu › Cloud › New cloud…**.

   Result: The New cloud dialog opens.

3. Click **Cancel**.

   Result: The dialog closes. No cloud starts.

4. Set the window back to 1280 × 800 pixels.

   Result: The toolbar shows all items again.

### 6.7 THEME — Light theme

1. In Settings, set the theme to light and save.

   Result: The board changes to the light theme.

2. Do task 6.1 and steps 1 to 4 of task 6.2 again.

   Result: The marks, the labels and the shortcuts are clear on the light
   surfaces.

3. Set the theme back to its first value and save.

   Result: The board changes back.

## 7. Pass criteria

- LAYOUT shows only the fps meter, **Dependencies** and **Menu** on the right.
- MENU shows the rows in order, with the configured shortcuts.
- SHORTCUT shows a changed shortcut in the menu.
- KEYS opens the command palette with the keyboard, and the palette stays open.
- DEPS starts the Dependencies entry from the button and from the palette.
- NARROW keeps **Dependencies** and **Menu** at 800 pixels.
- THEME shows clear marks and text in the light theme.

## 8. Cleanup

1. Close the candidate window.

   Result: The candidate process stops.

2. Close the Device panel.

   Result: The viewer connection closes.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
