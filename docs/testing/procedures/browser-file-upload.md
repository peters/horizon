---
procedure: browser-file-upload
feature: Browser file picker and file drops
platforms: [linux, windows, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser file upload test procedure

## 1. Purpose

This procedure tests the shared picker and file drops in browser panels.
It checks file contents, selection, navigation, themes, and the public browser contract.

## 2. Applicability

- Use the exact candidate for the pull request.
- Test local Chromium and Firefox on each available platform.
- Safari and remote sessions must refuse `drop_files` with `unsupported_backend`.
- Firefox uses DOM drag events. A page can refuse these events.
- Firefox supports same-origin frames. Cross-origin frame drops require Chromium.
- Remote file upload through `set_files` is a separate function.

## 3. Equipment and preconditions

- A task-owned isolated desktop with a live Device panel.
- The candidate, Chromium, Firefox, a file manager, and a desktop recorder.
- The fixture at `docs/testing/fixtures/browser-file-upload.html`.
- A local HTTP server that serves the fixture on `localhost` or a loopback address.
- A SHA-256 utility to calculate the expected digest of each synthetic file.
- Synthetic files in two folders: a PDF, a text file, an image, and a hidden file.
- A public browser MCP server for the candidate and its private coordination root.

## 4. Setup

1. Start the isolated desktop with `scripts/device-smoke/serve.py --native-view`.

   Result: The fixture reports its display and VNC endpoint.

2. Open a Device panel for that endpoint with `device_panel`.

   Result: The panel shows changing frames from the isolated desktop.

3. Open the upload fixture in a candidate browser panel.

   Result: The page shows **Project files** and **No files attached yet**.

4. Start the recorder for the isolated display.

   Result: The recorder captures the candidate and its interactions.

5. Calculate the SHA-256 digest of each synthetic file with a local utility. Record each digest before upload.

   Result: The expected digests are independent of the fixture. The loopback page has a secure context for its SHA-256 function.

## 5. Tasks

### 5.1 P01 — Browse and select files

1. Click **Choose files** through `browser_act`.

   Result: A centered picker shows a path field, file rows, and an **Upload** button.

2. Enter the path of the first synthetic folder.

   Result: The picker shows folders first, then matching files with sizes.

3. Select a PDF.

   Result: The row shows a check mark and the footer shows the file name.

4. Open the second folder.

   Result: The first file remains selected.

5. Select a text file.

   Result: The footer shows **2 files selected**.

6. Click **Upload 2 files**.

   Result: The dialog closes and the page lists both files. Compare each `sha256` value with the digest from setup step 5. Each value must match.

### 5.2 P02 — Search, limits, and cancellation

1. Open **Choose one PDF**.

   Result: The picker shows PDF files and folders.

2. Type part of a PDF name.

   Result: The list shows matching names.

3. Select two PDF rows in sequence.

   Result: The second selection replaces the first selection.

4. Press Escape.

   Result: The picker closes without changing the page attachments.

5. Open **Choose files** again.

   Result: No previous selection remains.

6. Select **Hidden files**.

   Result: Hidden files become visible.

7. Enter a path that does not exist.

   Result: An error appears and the picker remains usable.

8. Enter a folder with more than 36 matching files.

   Result: All bounded results remain accessible with scrolling and arrow keys.

### 5.3 P03 — Drop files

1. Drag two synthetic files from the isolated file manager onto the browser page.

   Result: The panel shows **Drop files to upload** during the drag.

2. Release the files over the drop area.

   Result: The page lists both files with **Drag and drop**. Each `sha256` value matches its expected digest.

3. Drop a folder onto the page.

   Result: The browser reports an error without changing the page attachments.

4. Drop a file onto an overlapping browser panel.

   Result: Only the topmost panel receives the file.

5. Drop a file onto a browser toolbar or the area outside the page image.

   Result: The browser reports an error and no terminal or editor receives the file.

6. Drop a file into an open picker.

   Result: The picker selects the file and waits for **Upload**.

7. Repeat step 6 in a detached workspace. Include native Wayland on Linux when available.

   Result: The picker selects the real file and waits for **Upload**. The page receives its expected SHA-256 digest after confirmation.

8. Pan the canvas. Change its zoom. Repeat steps 1 and 2 at each view.

   Result: The highlight follows the page image. The file reaches the drop area under the pointer and has the correct bytes.

### 5.4 P04 — Public contract and access policy

1. Query `#drop-zone` through `browser_query`.

   Result: The tool returns a current element ref.

2. Call `browser_act` with `action: drop_files`, that ref, and an authorized synthetic file path.

   Result: The page receives the same file contents as a manual drop. Its `sha256` value matches the expected digest.

3. Repeat with a path outside the permitted attachment roots.

   Result: The attachment policy refuses the action before the page receives files.

4. Read `browser_audit`.

   Result: The file drop entry shows a file count without file contents or private names.

5. Repeat `drop_files` on Safari or a remote session, if available.

   Result: The tool returns `unsupported_backend` before attachment staging.

### 5.5 P05 — Visual and navigation regressions

1. Repeat P01 in the light theme.

   Result: Text, focus, selection, and the primary action remain clear.

2. Resize the candidate to a narrow window.

   Result: The picker fits the window and its actions remain accessible.

3. Repeat P03 with a fullscreen browser and a detached workspace.

   Result: The correct browser receives the file at the correct page position.

4. Open a directory picker for a new terminal.

   Result: Path completion, selection, cancellation, and keyboard focus still work.

5. Navigate the browser while a picker request is pending.

   Result: An invalidated request cannot attach files to the replacement page.

### 5.6 P06 — Frame drop boundaries

1. Add `?frame=same` to the fixture URL. Navigate through `browser_navigate`.

   Result: The page shows a same-origin frame with a file drop area.

2. Drop a synthetic file into the frame. Repeat on local Firefox and Chromium.

   Result: The frame and parent page show the exact file name and size. Each `sha256` value matches its expected digest.

3. Add `?frame=cross` to the fixture URL. Navigate through `browser_navigate`.

   Result: The sandbox frame has an opaque origin. The parent cannot read its document.

4. Drop the same file into the sandbox frame on Firefox.

   Result: Firefox reports that cross-origin frame drops are unsupported. The frame receives no file.

5. Repeat step 4 on Chromium.

   Result: The frame and parent page show the exact file name and size. Each `sha256` value matches its expected digest.

### 5.7 P07 — CLI plan dispatch without a live browser

1. Run `cargo test -p horizon-browser-cli drop_files_plan_dispatches_the_public_action_with_synthetic_bytes`.

   Result: The CLI plan sends one `drop_files` action through public MCP to a mock host. The host checks its selector and synthetic file bytes. The structured report contains the mock action acknowledgement.

2. Repeat P04 through public browser MCP on a live candidate.

   Result: The shared handler delivers the file to the browser. Agent browser control stays on public MCP.

## 6. Pass criteria

- P01 through P07 pass on each tested platform and backend.
- Baseline terminal and editor file drops remain unchanged.
- The page receives exact synthetic file names and sizes. Each full-content SHA-256 digest matches the expected digest from the local utility.
- Record unavailable platform lanes separately.
- This change adds no persistent settings or migration.

## 7. Cleanup

1. Stop the recorder for the isolated display.

   Result: The finalized recording contains the complete feature flow.

2. Close the candidate normally.

   Result: The fixture stops only its owned processes.

3. Close the task-owned Device panel.

   Result: The viewer releases its connection.

## 8. Record of results

Put the candidate hash, platform results, screenshots, and recording in the pull request.
Keep private evidence outside the repository.
