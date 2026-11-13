---
procedure: device-viewer-reconnect
feature: Device viewer reconnect after a lost link
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Device viewer reconnect test procedure

## 1. Purpose

This procedure tests a native VNC Device viewer on a link that a person loses
for a time. It tests the automatic attempt, the state and the wait that the
viewer shows, the `Stop` control, and the fields that `device_panel` operation
`inspect` reports.

The viewer reconnects by itself only while its endpoint and its SSH route do not
change.

## 2. Applicability

- Use a current Horizon candidate on Linux.
- Use an agent in the candidate's workspace that has the `device_panel` tool.
- Use the isolated desktop of the local device fixture as the VNC target.
- This procedure does not test a VNC password, a recording or a browser panel.

## 3. Safety

> **CAUTION: USE ONLY THE ISOLATED TEST DESKTOP AND A LOOPBACK TCP PROXY.**
> Do not put a proxy between Horizon and a machine that you need. The proxy
> binds to `127.0.0.1` only, and you stop it at the end of the test.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- Xvfb, a window manager, x11vnc and Python 3.
- An agent in the candidate's workspace that has the `device_panel` tool.
- A loopback TCP proxy that can add a delay, stall a flow and cut a flow.
- A free loopback port for the proxy, for example `46000`.

## 5. Setup

1. Start the local device fixture with a private state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon \
     --native-view --state "$HOME/horizon-viewer-reconnect"
   ```

   Result: The fixture prints a manifest with a `vnc_address`.

2. Start the proxy between a new loopback port and `vnc_address`. Write the
   proxy port down as `PROXY`.

   ```sh
   python3 proxy.py --listen 127.0.0.1:46000 --to <vnc_address> --log proxy.log
   ```

   Result: The proxy listens on `PROXY`.

3. Create a viewer of `PROXY` in the calling agent's workspace.

   ```json
   {"operation": "create", "endpoint": "127.0.0.1:46000"}
   ```

   Result: The answer holds a `panel_id`. Write it down as `PANEL`.

4. Inspect the viewer until it reports a desktop.

   ```json
   {"operation": "inspect", "panel_id": "<PANEL>"}
   ```

   Result: `connection` is `connected`, `image_received` is true, and
   `frame_sequence` advances while the desktop changes.

## 6. Baseline

1. Read the header of the viewer in the window.

   Result: The badge says `Connected`, and no `Stop` text is in the badge line.

2. Record the time of the last frame and the value of `frame_sequence`.

   Result: The frame sequence advances when the desktop changes.

## 7. A cut comes back by itself

1. Cut the flow in the proxy.

   Result: The desktop stops changing. The viewer shows the last image.

2. Inspect the viewer.

   Result: `connection` becomes `reconnecting` within 60 seconds.
   `reconnect.attempt` is 1. `reconnect.next_attempt_in_millis` is a positive
   number of at most 3000 milliseconds. `connection_error` names the loss.

3. Read the header of the viewer.

   Result: The badge says `Reconnecting · next attempt in N s`, and the count
   goes down. The line below the badge says
   `The link to the desktop is lost. Horizon tries again by itself.`

4. Release the cut in the proxy.

   Result: An attempt starts at the wait that the badge showed. The badge says
   `Connecting…` and then `Connected`. The desktop changes again. No person
   pressed a control.

5. Inspect the viewer.

   Result: `connection` is `connected` and `frame_sequence` advances.

## 8. A stall is not the end

1. Stall the flow in the proxy for 5 seconds, then release it.

   Result: The viewer keeps the connection. The badge does not say
   `Reconnecting`. The desktop changes again after the release.

## 9. A long loss grows the wait

1. Cut the flow for two minutes.

2. Inspect the viewer every 10 seconds.

   Result: Each `reconnect.attempt` is one more than the attempt before it, and
   `reconnect.next_attempt_in_millis` grows to 30000 milliseconds and stays
   there. The viewer does not stop trying.

3. Release the cut.

   Result: The viewer connects at the next attempt without a control.

## 10. Stop ends the attempts

1. Cut the flow again and wait until `connection` is `reconnecting`.

2. Press `Stop` in the viewer.

   Result: The badge says `Stopped`. The last image stays. The line below the
   badge says `Select Reconnect to view this desktop.`

3. Release the cut and wait 60 seconds.

   Result: The viewer does not connect. `connection` stays `stopped`.

4. Press `Reconnect`.

   Result: The viewer connects again, and `connection_generation` is one more
   than before.

## 11. A refusal is not retried

Run `docs/testing/procedures/device-panel-ssh-trust.md` for a viewer on an SSH
route whose host key is not trusted.

Result: `connection` stays `disconnected` with `connection_failure.code` equal
to `host_not_trusted` or `host_key_changed`, and no attempt follows, although
the viewer of section 7 tries again.

## 12. A restored viewer stays stopped

1. Close the window and start Horizon again with the same state directory.

2. Open the restored viewer with `device_panel` operation `reconnect`, and
   before that operation, inspect the viewer.

   Result: The restored viewer reports `connection` `stopped` and connects
   nothing by itself.

## 13. Cleanup

1. Close only the viewer that this test created.

   ```json
   {"operation": "close", "panel_id": "<PANEL>"}
   ```

2. Stop the proxy and the fixture with the exact process identifiers that you
   started.

   Result: No test process remains, and the state directory is removed.

## 14. Result to record

- The time from the cut to `reconnecting`, in seconds.
- The attempt numbers and the waits that the viewer showed.
- The time from the release of the cut to `connected`, in seconds.
- Whether the viewer connected without a control.
- The value of `connection` 60 seconds after `Stop`.
- The candidate binary SHA-256.
