---
procedure: markdown-preview
feature: The pictures of a markdown preview in the editor panel
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Markdown preview test procedure

## 1. Purpose

This procedure proves that the preview of an editor panel shows a picture that
is a file on this computer. It also proves that the preview shows one line of
text in the place of a picture that it does not show, and that the pictures keep
their proportion to the text at every canvas zoom.

## 2. Applicability

- Candidate: a build that holds
  `crates/horizon-ui/src/editor_widget/preview_images/`. The build reads PNG and
  JPEG.
- Platforms: Linux, because the fixture needs an isolated X11 desktop. The
  reader and the decoder are the same code on macOS.
- This procedure does not test: a picture of another format, a picture with a
  transparent part, the animation of a GIF, and the picture of a browser panel.
  It does not test an image of a note that another application writes while the
  panel is open.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
fixture keeps its state in its own folder and touches no real Horizon home. The
test folder of section 5 holds pictures that the procedure makes, not a picture
of a person or of a screen.

## 4. Equipment and preconditions

- A Linux computer with the prerequisites of
  [the device smoke fixture](../../../scripts/device-smoke/README.md).
- A Horizon native VNC Device panel in the current workspace, for the live view.
- The command `python3`, which makes the test pictures.

## 5. Setup

1. Start the fixture with the native view.

   `python3 scripts/device-smoke/serve.py --native-view`

   Result: the fixture prints a `vnc_address`.

2. Open the panel of the fixture in a Device panel. Use the public `device_panel`
   tool with that `vnc_address`.

   Result: `connection` says connected, and `image_received` and
   `image_displayed` are true. `frame_sequence` advances while the desktop
   changes.

3. Make a test folder with three pictures and a note in it. Run this command in
   the terminal of the isolated desktop.

   ```bash
   python3 - <<'EOF'
   import struct, zlib, os
   folder = os.path.expanduser("~/preview-pictures")
   os.makedirs(folder + "/pictures", exist_ok=True)
   def png(name, width, height):
       raw = b"".join(b"\x00" + bytes([40, 90, 200, 255]) * width for _ in range(height))
       def chunk(kind, data):
           return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
       header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
       data = zlib.compress(raw)
       with open(name, "wb") as file:
           file.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", data) + chunk(b"IEND", b""))
   png(folder + "/pictures/narrow.png", 120, 60)
   png(folder + "/pictures/wide.png", 900, 60)
   png(folder + "/large.png", 300, 120)
   with open(folder + "/note.md", "w") as note:
       note.write("# Pictures\r\n\r\nA paragraph that wraps over more than one line of the panel.\r\n\r\n"
                  "![narrow](pictures/narrow.png)\r\n\r\n![wide](pictures/wide.png)\r\n\r\n"
                  "![large](<large.png>)\r\n\r\nA last line.\r\n")
   EOF
   ```

   Result: the folder `~/preview-pictures` holds `note.md` and three PNG files.

## 6. Tasks

### 6.1 P1 — The pictures of a note show at a zoom of 100 percent

1. Open `~/preview-pictures/note.md` in an editor panel. Choose **Preview**.

   Result: the heading, the paragraph, the three pictures and the last line
   show. The first picture is 120 points wide, the second one is not wider than
   the panel and keeps its proportion, and the third one is 300 points wide.

2. Look at the space beside the first picture and above it.

   Result: there is no line of text in the place of a picture, and no warning
   mark.

### 6.2 P2 — The pictures follow the canvas zoom

1. Set the canvas zoom to 67 percent with the zoom control of the canvas.

   Result: the letters stay sharp. The three pictures are smaller with the text,
   and the paragraph breaks on the same words as at 100 percent. The second
   picture is still not wider than the panel.

2. Set the canvas zoom to 150 percent.

   Result: the letters stay sharp. The pictures are larger with the text, and
   the paragraph breaks on the same words as at 100 percent. The second picture
   is still not wider than the panel.

3. Set the canvas zoom back to 100 percent, then close the panel and open
   `note.md` again.

   Result: the three pictures show again.

### 6.3 P3 — A picture that the preview does not show becomes a line

1. Add these two lines to the end of `note.md`, then choose **Preview**.

   ```markdown
   ![gone](pictures/gone.png)

   ![shot](https://example.test/shot.png)
   ```

   Result: the preview shows `Image not found: pictures/gone.png` and `Remote
   image not loaded: shot`. It shows no warning mark and no picture for them.

2. Wait 5 seconds.

   Result: the preview does not change. No picture arrives.

3. Write a file of more than 16 MiB and name it in a picture, then choose
   **Preview**.

   `dd if=/dev/zero of=~/preview-pictures/big.png bs=1M count=17`

   ```markdown
   ![big](big.png)
   ```

   Result: the preview shows `Picture too large: big.png`.

4. Write a file that is not a picture and name it in a picture.

   `printf 'text' > ~/preview-pictures/text.png`

   ```markdown
   ![text](text.png)
   ```

   Result: the preview shows `Not a picture file: text.png`.

### 6.4 P4 — A save reads the pictures again

1. Replace `pictures/narrow.png` with a picture of another size, for example by
   running the command of section 5 again after you change `120, 60` to
   `200, 80`.

   Result: the preview still shows the old picture, and it is 120 points wide.

2. Save the note with the save shortcut of the editor settings.

   Result: the preview shows the new picture, and it is 200 points wide.

## 7. Pass criteria

- Every step of section 6 gives the stated result.
- No state writes an error line at `ERROR` in the log of the fixture.
- The paragraphs of the note break on the same words at 67, 100 and 150 percent.

## 8. Cleanup

1. Close the editor panel and the Device panel of the fixture.

   Result: the workspace holds no panel of this procedure.

2. Stop the fixture with `Ctrl+C` in its terminal, and remove the test folder.

   `rm -rf ~/preview-pictures`

   Result: the folder is gone, and no fixture process stays.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the pictures of this procedure out of the repository.
