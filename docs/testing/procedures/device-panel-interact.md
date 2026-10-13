---
procedure: device-panel-interact
feature: Interact in native VNC Device panels
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Device panel Interact test procedure

## 1. Purpose

This procedure makes sure that **Interact** sends mouse and keyboard input from
a local Device panel to its desktop. It also makes sure that a Device panel does
not offer **Interact** when its VNC server ignores input.

## 2. Applicability

- Candidate: a frozen debug build of one exact commit, `<candidate-commit>`.
- Platforms: Linux with Xvfb.
- This procedure does not test:
  - The cloud desktop viewer on a real worker. Area E of the
    [cloud panels procedure](cloud-panels/e-panels.md) (test E05) does that test.
  - Agent input through the device CLI and MCP tools.

## 3. Safety

> **CAUTION:** START THE VNC SERVER WITHOUT `-viewonly` ONLY ON A PRIVATE TEST
> DISPLAY. A VNC server that accepts input lets each local process that connects
> to it control that display.

## 4. Equipment and preconditions

- The equipment of the [local device smoke](../../../scripts/device-smoke/README.md),
  and `xterm`.
- A native Device panel in your Horizon window that shows the viewer fixture of
  step 3 in section 5. This is the live view.

## 5. Setup

1. Start a private Xvfb display, for example `:<n>`, and an `xterm` on it.

   Result: The `xterm` shows a shell prompt.

2. Start an x11vnc server for `:<n>` on loopback, without `-viewonly`:

   ```sh
   x11vnc -display :<n> -localhost -rfbport <port> -forever -shared -nopw
   ```

   Result: The x11vnc log shows `PORT=<port>`.

3. Start the viewer fixture with a Device panel for that server:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --device-address 127.0.0.1:<port> --state <new-state-dir>
   ```

   Result: The fixture Horizon shows a Device panel for `127.0.0.1:<port>`.

4. In the fixture Device panel, click **Reconnect**.

   Result: The panel shows **Connected** and the `xterm` image.

## 6. Tasks

### 6.1 I01 — Interact sends input to a local desktop

1. Examine **Interact** in the fixture Device panel.

   Result: **Interact** is enabled and off. The header shows `Read-only`.

2. Click **Interact**.

   Result: The panel shows `Click the image to type into it`.

3. Click the `xterm` in the image.

   Result: The panel shows `Keyboard captured, Escape included; click outside the image to release it`.

4. Type `echo interact-check` and press Enter.

   Result: The `xterm` in the image shows `interact-check` on a new line.

### 6.2 I02 — Interact off stops the input

1. Click **Interact** again.

   Result: **Interact** is off. The header shows `Read-only`.

2. Click the `xterm` in the image.

   Result: The panel does not capture the keyboard.

3. Type `echo stopped`.

   Result: The `xterm` does not change.

### 6.3 I03 — A view-only server gets no Interact

1. Do test E05 of the [cloud panels procedure](cloud-panels/e-panels.md) on a
   ready cloud.

   Result: The cloud desktop viewer shows **Interact** disabled with the text
   `View only: the VNC server of this desktop ignores mouse and keyboard input`.

## 7. Pass criteria

- I01: The text typed through **Interact** shows in the `xterm`.
- I02: No input reaches the `xterm` while **Interact** is off.
- I03: The cloud desktop viewer does not offer **Interact** and shows the reason.

## 8. Cleanup

1. Close the fixture Horizon normally.

   Result: The fixture stops its own processes.

2. Stop the x11vnc, `xterm` and Xvfb processes of section 5 by their process IDs.

   Result: The private display is gone.

3. Close the live-view Device panel with `device_panel` operation `close`.

   Result: The panel is gone from your Horizon window.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
