---
procedure: device-letter-shortcuts
feature: native device key input
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Device letter shortcut test procedure
## 1. Purpose
This procedure proves that native device input sends lowercase `a`-`z` keysyms for Control chords and Ctrl+Shift+P, releases modifiers, refuses stale geometry, and keeps the CLI and MCP request shapes.
## 2. Applicability
- Candidate: the source build of `horizon-device` with the `cli` feature and the `horizon` UI with the lowercase key contract.
- Platforms: Linux X11 through the isolated fixture.
- Not tested: macOS, Wayland, browser pages, real input devices, the cloud worker desktop, or key names outside `a`-`z`, Enter and Escape.
## 3. Safety
> **CAUTION:** USE ONLY THE TASK-OWNED ISOLATED DISPLAY. The test sends X11 input to the focused window. A different display can receive the keys.
> **CAUTION:** DO NOT SEND AN ACTION AGAIN AFTER AN UNCLEAR RESULT. A repeated chord can run a shortcut two times. Take a new screenshot and stop the run.
> **CAUTION:** DO NOT TYPE INTO A BROWSER PAGE OR THE DEVICE PANEL. The panel is a read-only view for agents. Browser tools are out of scope.
## 4. Equipment and preconditions
- A Linux host with Xvfb, Openbox, x11vnc, `bwrap`, `dbus-daemon`, Python 3, `xinput`, `jq`, a Rust toolchain and the libxkbcommon headers.
- A source checkout. All new files go in one new task root under `$TMPDIR`; the repository stays clean.
- A live Device panel in the current workspace. Read [the fixture runbook](../../../scripts/device-smoke/README.md) before you start.
- No other process uses the fixture display, the task root or its state child.
## 5. Setup
Use Bash from the repository root for the shell commands.
1. Make the task root and children, then build and freeze the executables.
   ```sh
   taskroot=$(mktemp -d "${TMPDIR:-/tmp}/horizon-letter.XXXXXX"); chmod 700 "$taskroot"
   bin="$taskroot/bin"; evidence="$taskroot/evidence"; build="$taskroot/build"; state="$taskroot/state"
   mkdir "$bin" "$evidence" "$build"; chmod 700 "$bin" "$evidence" "$build"
   export taskroot bin evidence build state CARGO_TARGET_DIR="$build"
   cargo build -p horizon-device --features cli && cargo build -p horizon-ui --bin horizon
   cp "$build/debug/horizon" "$build/debug/horizon-device" "$bin"; sha256sum "$bin/horizon" "$bin/horizon-device" > "$bin/SHA256SUMS"
   printf 'taskroot=%q\nbin=%q\nevidence=%q\nbuild=%q\nstate=%q\n' "$taskroot" "$bin" "$evidence" "$build" "$state" > "$taskroot/env"; printf 'taskroot=%s\n' "$taskroot"
   ```
   Result: `SHA256SUMS` has one line for each frozen executable; `taskroot`, `bin`, `evidence` and `build` have mode 0700 and are separate children; `state` does not exist yet. Print `taskroot` for terminal 2.
2. In terminal 2, copy the printed root, load the saved values, and start the fixture.
   ```text
   export taskroot=<printed value>
   set -a; . "$taskroot/env"; set +a
   python3 scripts/device-smoke/serve.py --horizon "$bin/horizon" --native-view --state "$state"
   ```
   Result: The terminal 2 values equal the terminal 1 values. The manifest and `$state/lab.json` show `vnc_address`, the display and the owned process IDs. Keep terminal 2 open.
3. Set the target and lab paths, examine the display, and examine the child.
   ```sh
   target="$state/target.json"; lab="$state/lab.json"; export target lab
   jq -r '.endpoint.display' "$target"; printf '%s\n' "${DISPLAY:-unset}"
   readlink /proc/<horizon-pid>/exe; sha256sum /proc/<horizon-pid>/exe
   ```
   Result: The target display is not `$DISPLAY`; stop if the values are equal. The executable path is under `$bin` and the hash matches `SHA256SUMS`.
4. Pass the lab path to Python and create the native Device panel.
   ```sh
   vnc_address=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["vnc_address"])' "$lab")
   ```
   Result: `device_panel` `create` with that endpoint, then `inspect`, shows `connection` `connected`, and `image_received` with a `frame_sequence` that advances while the output changes. If the host does not support the panel, report the lane blocked; do not use noVNC.
## 6. Tasks
### 6.1 L1 — Check the target with the CLI doctor
1. Run the doctor against the fixture target.
   ```sh
   "$bin/horizon-device" --target "$target" doctor
   ```
   Result: Exit code 0 and a JSON object with `ok` true. Do not continue on `ok` false.
### 6.2 L2 — Assert the letter chords on the owned display
1. Run the owned-display test. It makes its own synthetic receiver.
   ```sh
   HORIZON_DEVICE_TEST_TARGET="$target" \
     cargo test -p horizon-device --features cli --test x11_live \
     letter_chords_deliver_lowercase_keysyms_and_release_modifiers -- --ignored --exact
   ```
   Result: `test result: ok. 1 passed`.
2. Read the assertions in [x11_live.rs](../../../crates/horizon-device/tests/x11_live.rs). The test makes an override-redirect window with `KEY_PRESS | KEY_RELEASE`, sets the focus and reads each keysym and state.

   Result: The test asserts these conditions:
   - A stale geometry returns `StaleGeometry`; no key event arrives for 200 ms; Control and Shift are not held.
   - Each `a`-`z` with Control gives keysym `0x61`-`0x7a` and state `0x0004`; Control and Shift are clear after each key.
   - `p` with Control+Shift gives keysym `0x70` and state `0x0005`; Enter with Control+Shift gives keysym `0xff0d` and state `0x0005`.
   L2 is the library receiver assertion. It uses no terminal, browser or person and never defaults to `DISPLAY`.
### 6.3 L3 — Run the CLI screenshot and act sequence
1. Take screenshots, capture fresh geometry, and send Ctrl+A and Ctrl+Shift+P.
   ```sh
   "$bin/horizon-device" --target "$target" screenshot "$evidence/00.png"
   geometry=$("$bin/horizon-device" --target "$target" screenshot "$evidence/01.png" | jq -c '.result.geometry')
   jq -nc --argjson g "$geometry" '{geometry:$g,action:{kind:"key",key:"a",modifiers:["control"]}}' | "$bin/horizon-device" --target "$target" act -
   geometry=$("$bin/horizon-device" --target "$target" screenshot "$evidence/02.png" | jq -c '.result.geometry')
   jq -nc --argjson g "$geometry" '{geometry:$g,action:{kind:"key",key:"p",modifiers:["control","shift"]}}' | "$bin/horizon-device" --target "$target" act -
   ```
   Result: `screenshot` gives `ok` true, `result.path` and a PNG with no `image_base64`; each `act` gives `ok` true and no `error`. `ok` true means dispatched, not that the shortcut took effect.
2. Send an action with the geometry revision changed.
   ```sh
   jq -nc --argjson g "$geometry" '{geometry:($g + {revision:($g.revision + "-stale")}),action:{kind:"key",key:"a",modifiers:["control"]}}' | "$bin/horizon-device" --target "$target" act -
   ```
   Result: Non-zero exit and `ok` false with `error.code` `stale_geometry`; L2 proves a stale geometry sends no key event.
### 6.4 L4 — Run the MCP doctor, screenshot and act sequence
1. Start the MCP server, initialize, and call the doctor before the screenshot.
   ```text
   "$bin/horizon-device" --target "$target" mcp
   {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"device-letter-procedure","version":"1"}}}
   {"jsonrpc":"2.0","method":"notifications/initialized"}
   {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"device_doctor","arguments":{}}}
   {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"device_screenshot","arguments":{}}}
   ```
   Result: `id` 1 has `result.capabilities.tools`. `id` 2 has `isError` false and `ok` true before any input. `id` 3 has `isError` false, `content[0].text` with `ok` true and `result.geometry`, and `content[1]` with `type` `image` and `mimeType` `image/png`.
2. Send the act requests with the copied geometry, then keep standard input open for L5.
   ```json
   {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"device_act","arguments":{"geometry":<id 3 geometry>,"action":{"kind":"key","key":"p","modifiers":["control","shift"]}}}}
   {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"device_act","arguments":{"geometry":<changed revision from L3 step 2>,"action":{"kind":"key","key":"a","modifiers":["control"]}}}}
   ```
   Result: `id` 4 has `isError` false and `ok` true. `id` 5 has `isError` true and `error.code` `stale_geometry`.

   The MCP tools are `device_doctor`, `device_screenshot`, `device_act`, `device_set_resize_enabled` and `device_resize`. L4 checks the JSON-RPC transport and request shape only; it does not assert keysyms, and dispatch success is not a shortcut result.
### 6.5 L5 — Reject uppercase and unknown key names
1. Take a fresh screenshot, then send the uppercase name `A` and the unknown name `aa` through the CLI.
   ```sh
   geometry=$("$bin/horizon-device" --target "$target" screenshot "$evidence/03.png" | jq -c '.result.geometry')
   jq -nc --argjson g "$geometry" '{geometry:$g,action:{kind:"key",key:"A",modifiers:["control"]}}' | "$bin/horizon-device" --target "$target" act -
   jq -nc --argjson g "$geometry" '{geometry:$g,action:{kind:"key",key:"aa",modifiers:["control"]}}' | "$bin/horizon-device" --target "$target" act -
   ```
   Result: Non-zero exit and `ok` false with `error.code` `invalid_request` for both. No key event is dispatched. L3 shows that `a` and `p` are accepted.
2. Send a fresh screenshot, then the same two names through MCP, and close standard input.
   ```json
   {"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"device_screenshot","arguments":{}}}
   {"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"device_act","arguments":{"geometry":<id 6 geometry>,"action":{"kind":"key","key":"A","modifiers":["control"]}}}}
   {"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"device_act","arguments":{"geometry":<id 6 geometry>,"action":{"kind":"key","key":"aa","modifiers":["control"]}}}}
   ```
   Result: `id` 6 has `isError` false. `id` 7 and `id` 8 fail before dispatch: a JSON-RPC `error` with code `-32602` (invalid parameters). The process exits after the close.
### 6.6 L6 — Observe the live panel and stop on an unclear result
1. Inspect the panel at least three times, two seconds apart, during L3-L5.
   Result: The panel was presented once: `connection` `connected`, `image_displayed` true and `frame_sequence` advancing with visible change. This is visual observation, not an assertion.
2. If the person navigates away and `inspect` reports `not_rendered` with `outside_canvas`, keep testing and recording. Do not reveal, reconnect or restart, and do not call the panel failed on a stationary frame counter alone.
   Result: The run continues; evidence keeps the earlier presentation and any later `outside_canvas` state separately. If a response is missing or ambiguous, do not resend; take a screenshot, record and stop.
## 7. Pass criteria
- L2 reports `1 passed`, all 26 Ctrl letters use `0x61`-`0x7a`, Ctrl+Shift+P uses `0x70`, and Control and Shift are clear after each chord.
- L1 and the MCP doctor return `ok` true before input.
- The CLI returns `ok` true for Ctrl+A and Ctrl+Shift+P with fresh geometry; the MCP returns `isError` false with fresh geometry and names `stale_geometry` for the changed revision.
- Uppercase and unknown names fail before dispatch through the CLI and through MCP.
- Dispatch success is not a shortcut result. The procedure proves transport, receiver events and delivery; it does not prove that the application ran the shortcut.
- The frozen hash matches `/proc/<horizon-pid>/exe`; the display is owned; the panel was presented once and frames advanced while output changed. A later `outside_canvas` is not a failure and `image_displayed` need not stay true.
- No browser tool, no noVNC and no input to a real desktop is used. An indeterminate result is not replayed.
## 8. Cleanup
> **CAUTION:** DELETE ONLY PATHS UNDER THE TASK ROOT THAT THIS RUN CREATED.
1. Close the exact Device panel with `device_panel` operation `close`, and close only that panel.
   Result: The viewer connection closes and the fixture target stays alive.
2. Press Ctrl-C in terminal 2, then examine that the owned fixture exited and the target expired.
   ```sh
   test ! -e "$target"
   pgrep -af -- "$bin"; pgrep -af -- "$state"
   ```
   Result: The target file is absent and no process of this run remains. If a process remains, stop and reconcile before you remove anything.
3. After that check, remove only the disposable build and frozen copies; keep the raw evidence.
   ```sh
   pgrep -af -- "$build"
   rm -rf "$bin" "$build"; ls -ld "$taskroot" "$evidence" "$state"
   ```
   Result: `taskroot`, `evidence` and `state` remain with mode 0700; `bin` and `build` are gone. Do not delete or move the raw evidence or the fixture manifest; the operator decides when to remove them.
## 9. Record of results
Write a report in `docs/testing/reports/` with [the report template](../reports/TEMPLATE.md), or put the results in the pull request.
Record the candidate commit and SHA-256, the target display, L1-L6, panel observations and private evidence paths. Keep private evidence out of the repository.
