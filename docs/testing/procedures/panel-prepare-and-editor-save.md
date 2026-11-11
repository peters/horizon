---
procedure: panel-prepare-and-editor-save
feature: A panel that prepares its process, and an editor panel that saves
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Panel preparation and editor save test procedure

## 1. Purpose

This procedure proves that a restored panel says that Horizon prepares it
before it shows its saved screen, and that an editor panel opens at once and
shows its text when the read answers. It also proves that an editor save shows
that it is in flight, that a failed save keeps the dirty mark and says why, and
that a closed editor panel writes its text.

## 2. Applicability

- Candidate: each candidate that changes the start of a panel, the restore of a
  saved board, or the save, the open and the close of an editor panel.
- Platforms: Linux, with the fixture.
- This procedure does not test the content of the saved screen, the browser
  panels, or the Device panels.

## 3. Safety

> **CAUTION:** REMOVE FILES ONLY IN THE PRIVATE HOME OF THE FIXTURE. The
> private home holds the saved board of this task. Do not touch the home of the
> person.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [fixture](../../../scripts/device-smoke/README.md).
- A Device panel that shows a live view of the isolated desktop.
- A state path that this task owns.
- A markdown file in the private home of the fixture with the text
  `first line`.

## 5. Setup

1. Start the fixture with the frozen candidate, `--native-view`,
   `--saved-session` and `--reuse-state`, and the state path of this task.

   Result: The fixture prints a manifest with a `vnc_address`.

2. Open that `vnc_address` in a Device panel.

   Result: The Device panel shows the candidate window.

## 6. Tasks

### 6.1 PREPARE — A restored panel says that Horizon prepares it

1. Open a terminal panel, type `echo saved screen`, and close the candidate
   window with the close button of the window manager.

   Result: The candidate stops. The fixture keeps the private home.

2. Start the fixture again with the same arguments and the same state path, and
   open the new `vnc_address` in the Device panel.

   Result: The board opens with every panel of the saved board.

3. Look at the body of the restored terminal panel immediately after it
   appears.

   Result: The body says `Preparing this panel`. It does not yet show
   `saved screen`.

4. Wait one second.

   Result: The body shows `saved screen`. The text `Preparing this panel` is
   gone.

### 6.2 OPEN — An editor panel opens at once

1. Drop the markdown file of the private home on the canvas.

   Result: The panel of the file opens at once. Its title is the file name.

2. Look at the body and at the mode bar of the panel.

   Result: Within one second the body shows `first line`. The mode bar shows
   the file name and no `*` mark.

### 6.3 SAVE — A save shows that it is in flight

1. Type `second line` at the end of the body of the editor panel.

   Result: The mode bar shows `*` before the file name.

2. Press the save shortcut of the settings.

   Result: The mode bar shows `Saving...` for a moment, and then only the file
   name without `*`.

3. Read the file in the private home of the fixture.

   Result: The file holds `first line` and `second line`.

### 6.4 FAIL — A failed save keeps the dirty mark and says why

> **CAUTION:** REMOVE THE FILE OF THE EDITOR ONLY IN THE PRIVATE HOME OF THE
> FIXTURE. The text of that file is not in Horizon after this task.

1. Remove the file of the editor panel in the private home of the fixture, and
   make a directory with the same name.

   Result: The file is gone and a directory takes its name.

2. Type `third line` in the body of the editor panel and press the save
   shortcut.

   Result: The mode bar keeps `*` before the file name. It shows a red line
   that starts with `Could not save this file`.

3. Remove the directory from step 1.

   Result: The path of the file is free again.

4. Press the save shortcut again.

   Result: The red line goes away. The `*` mark goes away after the save ends.

### 6.5 CLOSE — A closed editor panel writes its text

1. Type `fourth line` at the end of the body of the editor panel.

   Result: The mode bar shows `*` before the file name.

2. Close the panel with its close control.

   Result: The panel leaves at once. The window does not stop.

3. Read the file in the private home of the fixture.

   Result: The file holds `fourth line`.

4. Drop the markdown file again, and type `fifth line` at the end of the body
   of the new panel.

   Result: The mode bar shows `*` before the file name.

5. Close the window with the close button of the window manager.

   Result: The candidate stops normally.

6. Start the fixture again with the same arguments and the same state path.
   Read the file in the private home of the fixture.

   Result: The file holds `fifth line`.

### 6.6 FAIL-CLOSE — A failed write of a closed panel leaves one line

> **CAUTION:** REMOVE THE FILE OF THE EDITOR ONLY IN THE PRIVATE HOME OF THE
> FIXTURE. The text of that file is not in Horizon after this task.

1. Remove the file of the editor panel in the private home of the fixture, and
   make a directory with the same name.

   Result: The file is gone and a directory takes its name.

2. Type `sixth line` in the body of the editor panel, and close the panel with
   its close control.

   Result: The panel leaves. The machine section of the sidebar shows one red
   line that starts with `Could not save` and names the file.

3. Remove the directory from step 1.

   Result: The path of the file is free again.

## 7. Pass criteria

- A restored panel says that Horizon prepares it, and then shows its saved
  screen.
- An editor panel shows its text within one second of the drop.
- A save shows `Saving...` and writes the text of the frame.
- A failed save keeps the dirty mark and shows the reason in the panel.
- A closed editor panel writes its text, and the panel leaves at once.
- A failed write of a closed panel leaves one line that names the file.

## 8. Cleanup

1. Close the Device panel with the panel menu of the candidate.

   Result: The viewer closes and the fixture keeps the private home.

2. Close the candidate window normally.

   Result: The candidate stops. The processes of the fixture end.

3. Remove the state path of this task.

   Result: The private home and the folders of the recordings are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
