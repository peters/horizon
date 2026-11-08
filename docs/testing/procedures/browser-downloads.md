---
procedure: browser-downloads
feature: Browser panel downloads
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser panel downloads test procedure

## 1. Purpose

This procedure tests that a browser panel saves each download of its page into
one folder that the panel owns, and that `browser_downloads` reports the state
and the path of each download. It also tests that the report never holds a
download URL.

## 2. Applicability

- Candidate: the exact build of the pull request.
- Platforms: Linux and macOS, local panels only.
- Engines: Chromium and Firefox. Repeat each engine task for both.
- This procedure does not test Safari, a remote device, or a cloud worker
  browser. Those answer `unsupported_backend`, and task P06 checks this answer.

## 3. Safety

> **CAUTION:** DO NOT OPEN A REAL DOCUMENT ARCHIVE OR A PRIVATE SITE. Use only
> the synthetic fixture of this procedure, because a download writes a file to
> this computer.

## 4. Equipment and preconditions

- A task-owned isolated desktop that you watch in a live Device panel: a private
  Xvfb display, Openbox, and a loopback `x11vnc`. Do not use
  `scripts/device-smoke/serve.py` here: Snap Chromium refuses to start inside
  that fixture's filesystem namespace. A private display keeps the run isolated
  in the same way, as the [engine panels procedure](browser-engine-panels.md)
  does.
- A frozen candidate binary and its SHA-256 digest.
- Chromium, Firefox, and geckodriver. Give the candidate a private home and a
  `browser.profile_root` beneath that home: Snap Chromium cannot read a profile
  under a hidden folder or under `/tmp`.
- A synthetic fixture page served on a loopback address. The page is:

  ```html
  <!doctype html><html><body>
  <a id="first" href="/files/report.pdf" download>first</a>
  <a id="second" href="/files/report.pdf" download>second</a>
  <a id="third" href="/files/notes.txt">third</a>
  </body></html>
  ```

- A static server that returns `Content-Disposition: attachment` and a synthetic
  body for each path under `/files/`. Use `application/pdf` for the PDF path,
  and a multi-threaded server: one thread per request.
- A valid minimal PDF at `/viewer/doc.pdf`, served with
  `Content-Disposition: inline`, for the viewer task.
- A slow path at `/slow/report.pdf` that sends its body in ten parts, one second
  apart.
- The public browser MCP server of the candidate. Set `HORIZON_BROWSER_ROOT` to
  the runtime root of the candidate so the tools control its panels.

## 5. Setup

1. Start the private display and the loopback VNC server on it. In the
   repository root, start the chapter 4 page server.

   Result: `127.0.0.1:<port>` accepts a VNC connection and the page server
   answers.

2. Open a Device panel for that endpoint with `device_panel`, then show it with
   `reveal`.

   Result: `connection` is `connected`, `image_displayed` is true, and
   `frame_sequence` increases. Keep this panel open and watch each task in it.

3. Start the candidate with the private home, the display, its configuration
   file and `--ephemeral`. The configuration opens an agent panel that records
   `HORIZON_BROWSER_ACTOR` and `HORIZON_BROWSER_HOST_INSTANCE`.

   Result: The candidate shows its board on the isolated desktop, and the
   recorded identity file exists.

4. Start a stdio MCP client as `horizon --browser-mcp` with the private home and
   the two recorded variables. Remove all `CLAUDE_CODE_*` variables from its
   environment.

   Result: `browser_list` answers with an empty list.

5. Confirm the running executable: compare `sha256sum /proc/<pid>/exe` with the
   frozen digest.

   Result: The digests agree.

## 6. Tasks

### 6.1 P01 — A page-initiated download lands in the panel folder

1. Call `browser_create` with `backend: chromium` and the fixture URL.

   Result: The panel is ready and shows the page.

2. Call `browser_act` with `action: click` and the ref of `#first`.

   Result: The action completes.

3. Call `browser_downloads` with the panel id.

   Result: `reporting` is `true`. One row has `state: completed`, the file name
   `report.pdf`, and an absolute `path` under a folder that ends in `downloads`.

4. Read the reported path with a read-only tool.

   Result: The file exists and its bytes are the synthetic bytes of the server.

### 6.2 P02 — The second and later downloads are saved

This step is the defect of issue 1994: the second page-initiated download left
no file.

1. Call `browser_evaluate` with `document.querySelector('#second').click()`.
   Repeat it for `#third`.

   Result: Both calls succeed.

2. Call `browser_downloads` again.

   Result: Three rows have `state: completed`. Each row has a different `path`.
   The folder holds three files.

3. Call `browser_downloads` and examine the whole answer.

   Result: No row holds a URL, and no row holds a query string or a token.

### 6.3 P03 — Firefox saves into the same panel folder

1. Call `browser_create` with `backend: firefox` and the fixture URL.

   Result: The Firefox panel is ready. A previous Chromium panel stays.

2. Repeat P01 and P02 for this panel.

   Result: Each download is reported and each file exists. The Firefox panel
   folder is its own folder, not the folder of the Chromium panel.

3. Examine one Firefox row.

   Result: The row holds `state: completed` and the path that Firefox named.

### 6.4 P04 — A person's download is reported too

1. On the isolated desktop, in the browser panel, click the `#first` link with
   the pointer.

   Result: The page starts one download.

2. Call `browser_downloads`.

   Result: The list holds one more row with `state: completed`.

3. In a Chromium panel, navigate to the PDF path directly through
   `browser_navigate`. Use the PDF viewer's own download button on the isolated
   desktop.

   Result: The viewer starts one download and `browser_downloads` reports it
   with `state: completed` and a path.

### 6.5 P05 — An in-progress download and a canceled download

1. Serve a file whose body the server sends in 20 parts with one second between
   the parts. Click its link through `browser_act`.

   Result: The download starts.

2. Call `browser_downloads` while the transfer is active.

   Result: The row has `state: in_progress`, a `received_bytes` value that can
   grow, and no `path`.

3. Call `browser_downloads` after the transfer ends.

   Result: The row has `state: completed` and its path.

4. Start the same slow file, then stop the fixture server before the transfer
   ends.

   Result: The row has `state: canceled` and no `path`.

### 6.6 P06 — Unsupported backends and the capability

1. Read the capabilities of the Chromium panel and of the Firefox panel through
   `browser_panel`.

   Result: Both advertise `downloads: true`.

2. Call `browser_downloads` on a Safari panel, if the platform has one.

   Result: The tool returns `unsupported_backend`.

3. Call `browser_downloads` on a remote device panel, if one is configured.

   Result: The tool returns `unsupported_backend`.

### 6.7 P07 — The bytes of one file cross in the answer

1. Call `browser_downloads` with the panel id and `fetch` set to the file name of
   a row whose state is `completed`.

   Result: The structured result holds `fetched_file_name` and `fetched_bytes`,
   and the answer holds one resource block with the bytes of that file.

2. Call `browser_downloads` with `fetch` set to the file name of a row whose
   state is `in_progress`.

   Result: The answer holds no bytes and one text block that says no completed
   download has that name.

3. Call `browser_downloads` with `fetch` set to `../secret`.

   Result: The answer holds no bytes and one text block that states the panel
   folder rule. The panel keeps control of the path.

### 6.8 P08 — A shared profile states one folder

1. Call `browser_duplicate` on the Chromium panel.

   Result: A second panel opens on the same profile.

2. Click one download link in each panel.

   Result: Both downloads are saved, and both panels report the same folder and
   the same rows.

### 6.9 P09 — Retention and the audit

1. Start more than 64 downloads through `browser_evaluate`.

   Result: `browser_downloads` returns 64 rows and a non-zero `dropped` count.

2. Call `browser_audit`.

   Result: The journal holds a `downloads` record for each `browser_downloads`
   call, and no record holds a file path or a URL.

## 7. Pass criteria

- P01 through P08 pass for both engines on each tested platform.
- Each reported `path` exists in the panel folder and holds the bytes of the
  server.
- No row and no audit record holds a download URL or a token.
- Record an unavailable lane separately. Do not mark it as passed.

## 8. Cleanup

1. Call `browser_close` for each panel that this run created.

   Result: The panels leave `browser_list`.

2. Stop the synthetic fixture server.

   Result: The loopback port is closed.

3. Stop the candidate and the private display, VNC and page servers. Delete the
   private home, including the panel download folders.

   Result: No process of this run stays.

4. Close the task-owned Device panel.

   Result: The viewer releases its connection.

## 9. Record of results

Put the candidate digest, the platform results, the `browser_downloads` answers,
and the screenshots in the pull request. Keep the recording of the isolated
desktop outside the repository, except the pull request GIF.
