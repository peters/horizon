# Smoke plan: `browser_act` `set_files` (issue #821)

Temporary validation plan for the file-attachment primitive. Delete after the
validation pass unless asked to keep it.

## Fixture

Create a plain, visible directory (snap Chromium cannot read `file://` pages
under `/tmp` or hidden home directories) holding:

- `upload.html`: a page with a hidden `<input id="docs" type="file" multiple
  accept=".pdf,.png,.jpg,.jpeg,.txt,application/pdf,image/*">` behind a
  styled button, a visible `<input id="avatar" type="file" accept="image/*">`,
  and `change` handlers that render `N file(s) after M change event(s)` plus
  each file's name, size and type.
- `uploads/claim.pdf`, `uploads/photo.png`, `uploads/notes.txt` (synthetic).
- `uploads/looks-inside.txt`: a symlink to a file outside the fixture root.
- An `outside/secret.txt` file outside the fixture root.

Run every MCP client with `HORIZON_WORK_ROOT=<fixture root>` and the
`HORIZON*` and `CLAUDE_CODE*` variables scrubbed.

## Lane A: standalone Chromium (engine proof)

`horizon-browser mcp --standalone --backend chromium` with `HOME` pointed at a
throwaway directory. Speak newline-delimited JSON-RPC over stdio.

| Step | Expect |
|------|--------|
| `browser_list` | panel `capabilities` include `set_files` |
| `browser_navigate` to the fixture page | `completed: true` |
| `browser_query input[type=file]` | two nodes with `file_input` (`accept`, `multiple`, `files: 0`), both `visible: false` for the hidden one |
| `browser_act set_files` on `#docs` ref with `claim.pdf` + `photo.png` | `files` lists both names with sizes > 0 and MIME types |
| `browser_evaluate` the status text | `2 file(s) after 1 change event(s)` and both names listed |
| `browser_query #docs` | `file_input.files == 2` |
| `set_files #avatar` with `claim.pdf` | error `accept_mismatch` |
| `set_files #avatar` with two files | error `multiple_not_allowed` |
| `set_files` on the button `#add` | error `not_file_input` |
| `set_files` with `outside/secret.txt` | error `attachment_policy … outside the allowed roots` |
| `set_files` with the symlink | error `attachment_policy … outside the allowed roots` |
| `set_files` with a missing path | error `attachment_policy … cannot be resolved` |
| `set_files` with a relative path | error `set_files paths must be absolute` |
| `browser_evaluate` avatar status | still `No avatar` (refusals had no side effects) |
| `set_files #avatar` with `photo.png` | `files: [photo.png]`, avatar handler ran |
| `browser_audit` | `set_files` entries carry `paths` and the target; no file contents |

## Lane B: Horizon browser panel viewed through a native VNC Device panel

1. Start a task-owned `Xvfb :N` (1600x1000), `openbox`, and a loopback,
   view-only `x11vnc -localhost -viewonly -forever -shared -rfbport <port>`.
2. Launch the candidate `horizon --config <yaml> --ephemeral` on that display
   with a private `HOME`, where the config has `version: 11`, a workspace whose
   terminal is `kind: browser` with `command: file:///…/upload.html`, and
   `browser.profile_root` under the private `HOME`.
3. Verify the exact child PID's `/proc/<pid>/exe` hash matches the candidate.
4. Create the viewer with `device_panel create 127.0.0.1:<port>` in the
   agent's workspace; inspect `connection`, `image_received`,
   `image_displayed` and an advancing `frame_sequence` during changing output
   before claiming a live view. Record the isolated display with a scoped
   recorder (for example `ffmpeg -f x11grab -i :N`) across the interaction.
5. Run `horizon-browser mcp --connect` with the same private `HOME` and drive
   the panel: `set_files #docs` with three files, evaluate the status text,
   the `accept_mismatch` and `attachment_policy` refusals, `set_files #avatar`,
   and `browser_audit`.
6. Screenshot after launch and after the attachment; decode representative
   video frames and confirm the page moved from `No files` to the file list.
7. Close the Device panel through `device_panel close`; stop only the
   fixture-owned Horizon, x11vnc, openbox and Xvfb processes.

## Lane C: macOS (Safari WebDriver, local)

Same fixture. On a Safari panel, `set_files` goes through Element Send Keys
with the host paths. Expect the readback to hold the requested files or a
typed `attachment_mismatch`; a remote target must return `unsupported_backend`.
