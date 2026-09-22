# Smoke test: `browser_act` `set_files` across browsers

Permanent recipe for proving the file-attachment primitive (issue #821) on
every backend. Each lane is self-contained; run the ones the change touches.

## Fixture

Create a plain, visible directory (snap Chromium cannot read `file://` pages
under `/tmp` or hidden home directories; `/home/<user>/set-files-smoke` works)
with these entries:

- `upload.html`:

  ```html
  <!doctype html><html><head><meta charset="utf-8"><title>Documents</title>
  <style>input[type=file]{display:none}</style></head><body>
  <button id="add" type="button">Add file</button>
  <input id="docs" type="file" name="docs" multiple
         accept=".pdf,.png,.jpg,.jpeg,.txt,application/pdf,image/*">
  <label>Avatar <input id="avatar" type="file" name="avatar" accept="image/*"></label>
  <p id="status">No files</p><ul id="list"></ul><p id="avatar-status">No avatar</p>
  <script>
    const docs = document.getElementById('docs'), list = document.getElementById('list');
    let changes = 0;
    document.getElementById('add').addEventListener('click', () => docs.click());
    docs.addEventListener('change', () => {
      changes += 1; list.innerHTML = '';
      for (const f of docs.files) { const li = document.createElement('li');
        li.textContent = `${f.name} (${f.size} bytes, ${f.type || 'unknown'})`; list.appendChild(li); }
      document.getElementById('status').textContent = `${docs.files.length} file(s) after ${changes} change event(s)`;
    });
    document.getElementById('avatar').addEventListener('change', (e) => {
      document.getElementById('avatar-status').textContent = `avatar: ${e.target.files[0]?.name || 'none'}`;
    });
  </script></body></html>
  ```

- `uploads/claim.pdf`, `uploads/photo.png`, `uploads/notes.txt`: any small
  synthetic files with those extensions, and `uploads/empty.txt` with no
  content.
- `uploads/looks-inside.txt`: a symlink to a file outside the fixture root.
- A file outside the fixture root, for example `../outside/secret.txt`.

Run every MCP client with the inherited `HORIZON*` and `CLAUDE_CODE*`
variables scrubbed from its environment first, then `HORIZON_WORK_ROOT`
set to the fixture root for that client.

## Driving the MCP server

`horizon-browser mcp` speaks newline-delimited JSON-RPC over stdio: send
`initialize`, then the `notifications/initialized` notification, then
`tools/call` requests. Tool results arrive in `structuredContent`; tool
failures arrive as `isError` content whose text carries the typed code.
Standalone mode (`--standalone --backend <chromium|firefox|safari>`) owns its
own browser and lists one `standalone-*` panel after a short delay; connect
mode (`--connect`) lists the panels of the Horizon whose private `HOME` the
server shares. A minimal Python driver is about 40 lines: `subprocess.Popen`
with pipes, `json.dumps(msg) + "\n"` per request, and `readline` until the
reply with the matching `id` arrives.

## Checks (identical for every local backend)

| Step | Expect |
|------|--------|
| `browser_list` | panel `capabilities` include `set_files` |
| `browser_navigate` to `file://…/upload.html`, `wait: dom_content_loaded` | `completed: true` |
| `browser_query` selector `input[type=file]` | two nodes with `file_input` (`accept`, `multiple`, `files: 0`); the hidden one has `visible: false` |
| `browser_act set_files` with the `#docs` ref and `claim.pdf` + `photo.png` | `completed: true`, `files` lists both names with sizes > 0 and MIME types |
| `browser_evaluate` `document.getElementById('status').textContent` | `2 file(s) after 1 change event(s)` |
| `browser_query` `#docs` | `file_input.files == 2` |
| `set_files #docs` with `empty.txt` | `files: [{name: empty.txt, size: 0}]` (the readback really reads an empty file) |
| `set_files #docs` with `notes.txt` | `files` holds only `notes.txt`: an attachment replaces the selection on every backend, including WebDriver where Send Keys would otherwise append |
| `browser_evaluate` the status text | `1 file(s) after 3 change event(s)` |
| `set_files #avatar` with `claim.pdf` | error `accept_mismatch` |
| `set_files #avatar` with two files | error `multiple_not_allowed` |
| `set_files` on the button `#add` | error `not_file_input` |
| `set_files` with the outside file | error `attachment_policy … outside the allowed roots` |
| `set_files` with the symlink | error `attachment_policy … outside the allowed roots` |
| `set_files` with a missing path | error `invalid_input … cannot be resolved` |
| `set_files` with a relative path | error `invalid_input` with `set_files paths must be absolute` |
| `set_files` with a valid accept token after character 2048 | the complete accept policy is enforced |
| `set_files` with an accept attribute over 8192 UTF-16 code units | `accept_unsupported`, selection and events unchanged |
| `browser_evaluate` avatar status | still `No avatar` (refusals had no side effects) |
| `set_files #avatar` with `photo.png` | `files: [photo.png]` and the avatar handler ran |
| `browser_audit` | `set_files` entries carry the resolved `paths` and target; no file contents anywhere |

After the run, the staging directory (`<runtime root>/runtime/browser-attachments/`,
or `~/Horizon/browser-attachments/<digest of the runtime root>/` on Linux when the runtime root is a
hidden directory beneath `HOME`) holds one directory per panel with the
staged copies of its attachment actions; they are retained for the page's
lazy reads and pruned by age, count and size on the next attachment.

Snap check (Linux, Snap Chromium or Firefox): put a copy of `claim.pdf`
under a hidden directory directly beneath the real home directory, list that
directory in `HORIZON_BROWSER_ATTACHMENT_ROOTS`, attach it, then
`browser_evaluate` a `file.arrayBuffer()` read of `input.files[0]`; the
read must succeed because the browser reads the staged copy, not the hidden
source.

## Lane A: Chromium (Linux, macOS, Windows)

`horizon-browser mcp --standalone --backend chromium` with `HOME` pointed at
a throwaway directory. The engine path is `DOM.setFileInputFiles`.
Last run: Linux, 20/20, 2026-09-21.

## Lane B: Firefox (Linux, macOS, Windows)

`horizon-browser mcp --standalone --backend firefox` (needs `firefox` and
`geckodriver` on `PATH`). The engine path is Element Send Keys with
newline-separated paths. Page-side refusals travel as script values whose
object has an `error` member; the transport must not mistake them for a W3C
error envelope. Last run: Linux, 20/20, 2026-09-21.

## Lane C: Safari (macOS only)

`horizon-browser mcp --standalone --backend safari` after
`safaridriver --enable`. Same Element Send Keys path as Firefox; expect the
same table. If safaridriver rejects Send Keys on a file input, the readback
fails with `attachment_mismatch` rather than reporting success, which is the
contract; record the driver version with the result.

## Lane D: remote targets

Discover a BrowserStack macOS Safari target and an Android Chrome target with
`browser_provider_devices`. Test each in a separate `browser_create` session,
closing it before allocating the next. Both must advertise `set_files`, select
an authorized synthetic file, read its exact bytes through the page File API,
and submit a multipart upload successfully. Verify replacement, multiple-file
inputs, empty files, accept refusals, and the 16 MiB/file and 32 MiB/request
remote limits. Confirm provider usage returns to zero after closing.

iOS remains unsupported: omit `set_files` from capabilities and return
`unsupported_backend` before opening source paths. Native-picker support is
outside this change.

## Lane E: Horizon browser panel viewed through a native VNC Device panel

1. Start a task-owned `Xvfb :N` (1600x1000), `openbox`, and a loopback
   view-only `x11vnc -norc -no6 -display :N -localhost -viewonly -forever
   -shared -rfbport <port> -nopw -noxdamage`.
2. Launch the candidate `horizon --config <yaml> --ephemeral` on that display
   with a private `HOME`: `version: 11`, `browser.profile_root` under that
   `HOME`, one workspace whose terminal is `kind: browser` with
   `command: file:///…/upload.html`.
3. Verify the child PID's `/proc/<pid>/exe` hash matches the candidate.
4. In the calling agent's current workspace, use public `device_panel`
   `create` with that fixture's VNC endpoint. Inspect `connection: connected`,
   `image_received`, `image_displayed` and advancing `frame_sequence` during
   changing output. A second isolated viewer alone does not satisfy this lane.
   If presentation fails, use the bounded viewer health procedure in
   `scripts/device-smoke/README.md`; report the lane blocked if recovery fails.
5. Record the target display directly with `ffmpeg -f x11grab -i :N` across
   the interaction. Run the checks above through the candidate's public
   `browser_*` MCP tools sharing only the target's private state.
6. Capture screenshots after launch and resize/fit. Decode representative
   recording frames to confirm the selection and page updates were captured.
   Record the exact candidate commit, binary hash and application child PID.
7. Close the exact candidate window normally, close the task-owned Device
   panel through `device_panel`, and clean up only fixture-owned processes.

Repeat this lane on each behavior-changing candidate. Keep timestamped native
viewer observations, decoded recording frames, the executable hash and the
candidate identity with the private smoke evidence. Standalone browser checks
do not replace this current-workspace live-view lane.
