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
  synthetic files with those extensions.
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
| `set_files #avatar` with `claim.pdf` | error `accept_mismatch` |
| `set_files #avatar` with two files | error `multiple_not_allowed` |
| `set_files` on the button `#add` | error `not_file_input` |
| `set_files` with the outside file | error `attachment_policy … outside the allowed roots` |
| `set_files` with the symlink | error `attachment_policy … outside the allowed roots` |
| `set_files` with a missing path | error `attachment_policy … cannot be resolved` |
| `set_files` with a relative path | error `set_files paths must be absolute` |
| `browser_evaluate` avatar status | still `No avatar` (refusals had no side effects) |
| `set_files #avatar` with `photo.png` | `files: [photo.png]` and the avatar handler ran |
| `browser_audit` | `set_files` entries carry the resolved `paths` and target; no file contents anywhere |

After the run, the staging directory (`<runtime root>/runtime/browser-attachments/`,
or `~/Horizon/browser-attachments/` on Linux when the runtime root is a
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
Last run: Linux, 17/17, 2026-09-21.

## Lane B: Firefox (Linux, macOS, Windows)

`horizon-browser mcp --standalone --backend firefox` (needs `firefox` and
`geckodriver` on `PATH`). The engine path is Element Send Keys with
newline-separated paths. Page-side refusals travel as script values whose
object has an `error` member; the transport must not mistake them for a W3C
error envelope. Last run: Linux, 17/17, 2026-09-21.

## Lane C: Safari (macOS only)

`horizon-browser mcp --standalone --backend safari` after
`safaridriver --enable`. Same Element Send Keys path as Firefox; expect the
same table. If safaridriver rejects Send Keys on a file input, the readback
fails with `attachment_mismatch` rather than reporting success, which is the
contract; record the driver version with the result.

## Lane D: remote targets

On a `browser_create target=<remote>` panel, `set_files` must return
`unsupported_backend` and `browser_list` must not advertise `set_files`.

## Lane E: Horizon browser panel viewed through a native VNC Device panel

1. Start a task-owned `Xvfb :N` (1600x1000), `openbox`, and a loopback
   view-only `x11vnc -norc -no6 -display :N -localhost -viewonly -forever
   -shared -rfbport <port> -nopw -noxdamage`.
2. Launch the candidate `horizon --config <yaml> --ephemeral` on that display
   with a private `HOME`: `version: 11`, `browser.profile_root` under that
   `HOME`, one workspace whose terminal is `kind: browser` with
   `command: file:///…/upload.html`.
3. Verify the child PID's `/proc/<pid>/exe` hash matches the candidate.
4. View it through a native Device panel and confirm frames arrive:
   - from the developer's Horizon: `device_panel create 127.0.0.1:<port>` and
     inspect `connection: connected`, `image_received`, `image_displayed` and
     an advancing `frame_sequence` while the page changes;
   - or from a second candidate Horizon on another display whose config has
     a `kind: device` terminal with `command: 127.0.0.1:<port>`; press
     Reconnect, then F11 to fullscreen the viewer.
5. Record the viewer display with `ffmpeg -f x11grab -i :M` across the
   interaction, then run the checks above through `horizon-browser mcp
   --connect` sharing the target's private `HOME`.
6. Screenshot the viewer before and after; the Device panel must show the
   page moving from `No files` to the file list. Decode a frame from the
   recording to confirm the movement was captured.
7. Close the Device panel (`device_panel close`, or the viewer Horizon's
   window) and stop only the fixture-owned processes.

Last run: Linux, the PR #822 head viewed in a candidate Horizon's
Device panel, 8/8 checks, 2026-09-21.
