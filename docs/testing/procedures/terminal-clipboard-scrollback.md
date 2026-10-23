---
procedure: terminal-clipboard-scrollback
feature: terminal clipboard and scrollback
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Terminal clipboard and scrollback test procedure

## 1. Purpose

This procedure proves that a terminal panel uses the desktop clipboard and that
the scrollback chords do not take keys from the program in the terminal.

## 2. Applicability

- Candidate: a Horizon build that has the clipboard chords and the scrollback
  chords.
- Platforms: X11 and Wayland on Linux. macOS and Windows use the same chords
  with the Command key in place of the Control key.
- This procedure does not test: the browser panel, the device panel, or a
  remote SSH session.

## 3. Safety

> **CAUTION:** A COPY STEP REPLACES THE DESKTOP CLIPBOARD. Save the clipboard
> content that you need before you start.

## 4. Equipment and preconditions

- A Linux host with `xdotool`, ImageMagick `import`, and a file manager.
- An image file with the PNG format, for example in `/tmp`.
- A text editor and the program `less` in the guest system.
- A task-owned isolated desktop, as the
  [local smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  starts it. The fixture gives a private `HOME` and a private `/tmp`.

## 5. Setup

1. Start the fixture with the candidate binary and `--native-view`.

   Result: the fixture writes `lab.json` with a `display` and a `vnc_address`.

2. Make the Device panel for `vnc_address` visible in your workspace.

   Result: the panel shows the Horizon window of the fixture.

3. Open a terminal panel and put the panel in focus.

   Result: the cursor is in the terminal.

## 6. Tasks

### 6.1 T1 — Copy and paste text with the keyboard

1. Type `echo COPIED-FROM-THE-SCREEN` and press Enter.

   Result: the terminal shows the text two times.

2. Select the text `echo COPIED-FROM-THE-SCREEN` with the pointer.

   Result: the selected text has a background color.

3. Press Shift+Ctrl+C, then press Shift+Ctrl+V.

   Result: the terminal shows the selected text at the prompt.

4. Press Enter.

   Result: the terminal shows the text again.

5. Repeat steps 2 to 4 with Ctrl+Insert for the copy and Shift+Insert for the
   paste.

   Result: the same text appears at the prompt.

### 6.2 T2 — Paste the desktop clipboard

1. Copy a text in another application of the isolated desktop.

2. Press Shift+Ctrl+V in the terminal panel.

   Result: the terminal shows the text of the other application.

### 6.3 T3 — Paste a clipboard image

1. Copy a PNG image in another application of the isolated desktop.

2. Press Shift+Ctrl+V in the terminal panel.

   Result: the terminal shows a path to a PNG file.

3. Type `file -b ` before the path and press Enter.

   Result: the terminal shows `PNG image data` and the size of the image.

### 6.4 T4 — Drop a file on the terminal

1. Open the file manager of the isolated desktop and select the PNG image.

2. Drag the file to the terminal panel and release the pointer.

   Result: the terminal shows the path of the file and a space after the path.

3. Type `file -b ` before the path and press Enter.

   Result: the terminal shows `PNG image data` and the size of the image.

### 6.5 T5 — Move the scrollback

1. Type `seq -f 'output-line-%03g' 1 200` and press Enter.

   Result: the terminal shows the last lines of the output.

2. Press Shift+PageUp two times.

   Result: the terminal shows older lines.

3. Press Shift+Home.

   Result: the terminal shows the first line of the output.

4. Press Shift+End.

   Result: the terminal shows the prompt again.

5. Press Ctrl+Shift+Up one time.

   Result: the terminal moves by one line.

### 6.6 T6 — Make sure the programs keep their keys

1. Press the PageUp key and the PageDown key at the prompt.

   Result: the terminal shows the characters that the shell writes for these
   keys, or it does not change.

2. Type `less /etc/services` and press Enter.

3. Press PageDown, PageUp, Home and End.

   Result: `less` moves in its own page and keeps its own keys.

4. Press `q`.

   Result: the prompt shows again.

5. Type `vim /tmp/t1.txt` and press Enter.

6. Press Shift+PageUp, then press Shift+End.

   Result: the terminal moves the scrollback and `vim` does not change its
   text.

7. Press `:q!` and press Enter.

   Result: the prompt shows again.

8. Press Ctrl+A, type `echo ALPHA`, and press Ctrl+E and then Ctrl+U.

   Result: the shell line editing works and the clipboard does not change.

### 6.7 T7 — Make sure an idle window stays idle

1. Do not touch the pointer and do not type for ten seconds.

   Result: the frame counter of the panel does not change.

2. Examine the CPU time of the candidate process with `ps -o time= -p <pid>`.

   Result: the CPU time does not change.

## 7. Pass criteria

- Each copy chord puts the selection on the desktop clipboard.
- Each paste chord inserts the desktop clipboard content.
- A clipboard image and a dropped file become a path that `file -b` reads.
- Shift+PageUp, Shift+PageDown, Shift+Home and Shift+End move the scrollback.
- `less`, `vim` and the shell line editing keep the keys that they bind.
- The idle window does not paint.

## 8. Cleanup

1. Close the terminal panel and the window of the fixture.

   Result: the candidate process stops.

2. Stop the fixture process and remove its state directory.

   Result: the fixture directory is gone.

## 9. Record of results

Put the results in the pull request. Keep the private evidence out of the
repository.
