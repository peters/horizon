---
procedure: desktop-notifications
feature: Desktop notification of an agent that needs the person
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Desktop notification test procedure

## 1. Purpose

This procedure proves that Horizon sends one desktop notification when an agent
panel needs the person or is done, and only while no Horizon window has the
focus. It proves the line of the notification, the group of the changes, the
bounds and the setting.

## 2. Applicability

- Candidate: a Linux build with the desktop notification and the fixture options
  `--attention-agent`, `--attention-agent-count`, `--notifications-log` and
  `--plain-window`.
- Platforms: Linux. macOS uses another program of the system, and this
  procedure does not test it.
- This procedure does not test: the attention states of the sidebar, which
  [the agent attention procedure](agent-attention.md) covers. It does not test a
  real notification server, because the fixture records the calls in a file.

## 3. Safety

> **CAUTION:** REMOVE THE STATE FOLDER OF THE FIXTURE. `rm -rf` deletes every file
> of that folder and nothing can restore it. Give the path that you created for
> this run, and no other path.

## 4. Equipment and preconditions

- A Linux computer with the prerequisites of the local device smoke fixture:
  Xvfb, Openbox, x11vnc, `bwrap`, `dbus-daemon` and Python 3.
- One of `xterm`, `xmessage` or a window of Tk for `--plain-window`.
- A frozen candidate binary and the `horizon-device` binary of the same commit.
- A new state path that this run owns.

## 5. Setup

1. Start the fixture with the stand-in agent, two stand-in panels, the
   notification log and the plain window.

   ```bash
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --state /var/tmp/notifications-1 --native-view --attention-agent 0 \
     --attention-agent-count 2 --notifications-log --plain-window
   ```

   Result: the isolated desktop holds a Horizon window and a plain window. The
   Horizon window holds the two stand-in panels in a row above the panel
   Notifications, which follows the log of the notifications. The terminal prints
   the VNC address.

   The canvas also shows the card `Review work before continuing (1)`, with the
   line `Attention stand-in 1 Automatic continuation is not verified for this
   session.` and the button `Resume work`. The card is expected. A panel of the
   fixture comes from the config of the run, so Horizon restores it, and this
   private home holds no saved work for the conversation of the panel, so
   Horizon asks before it continues. The panel's program starts after that
   decision, so no event of the stand-in changes it: one keystroke in the panel
   clears the card. The tasks of section 6 type into the panel, which clears it.
   Do not click `Resume work`, because that restarts the panel.

2. Open the VNC address in a Horizon native Device panel.

   Result: the image of the isolated desktop shows. The panel advances its frame
   sequence while the desktop changes.

## 6. Tasks

Give each task the ID that follows. A report gives one result for each ID.

A notification arrives only while no window of Horizon has the focus, and a
keypress gives Horizon the focus again. So a task types into a panel first and
gives the focus to the plain window after that. The word `after SECONDS WORD`
of the stand-in sends `WORD` after that delay, which makes this order possible;
the delay is at most 60 seconds. Each step names the window that it types into,
and that window has the focus when the step starts.

### 6.1 N1 — A change that a person in another window receives

1. Click the panel `Attention stand-in 1`, so that it has the focus of the
   desktop.

2. Type `after 6 ask` and press Enter.

   Result: the panel says `ask in 6 s`. The stand-in sends the question six
   seconds later.

3. Within those six seconds, click the plain window, so that no window of
   Horizon has the focus. Do not click into a window of Horizon again.

4. Wait eight seconds for the delay and the group of the changes, then read the
   bottom line of the Notifications panel.

   Result: one line of JSON holds `--app-name=Horizon`, then `--`, then
   `Horizon`, then the name of the panel and `needs you`. The file holds one
   line.

### 6.2 N2 — A change that a person in Horizon does not receive

1. Click the panel `Attention stand-in 1`, so that it has the focus of the
   desktop, and keep that focus. This task clicks no other window.

2. Type `start` and press Enter. Then type `stop` and press Enter, to end a
   turn.

   Result: the sidebar says `Done`, and the Notifications panel shows no new
   line after eight seconds. The sidebar is the only place that shows the
   change.

### 6.3 N3 — Several changes become one line

1. Click the panel `Attention stand-in 1` and type `after 10 ask`.

2. Click the panel `Attention stand-in 2` and type `after 6 ask`.

   Result: each panel says that it sends `ask` later. Type the second line at
   about four seconds after the first one, so that both questions arrive within
   the two seconds of one group.

3. Click the plain window, so that no window of Horizon has the focus.

4. Wait twelve seconds.

   Result: the Notifications panel shows one line, and its body is a count such
   as `2 agents need you`. A line for each change alone is no pass: repeat the
   task with `after 12 ask` in the first panel when the second line was typed
   later.

### 6.4 N4 — The setting

1. Open Settings, then General, and choose `Off` under Desktop notifications.

   Result: the value is saved.

2. Click the panel `Attention stand-in 1`, type `after 6 ask`. Click the plain
   window within six seconds and wait eight seconds.

   Result: the Notifications panel shows no new line.

3. Choose `When an agent needs me`. Click the panel `Attention stand-in 1`, type
   `start`, press Enter, type `after 6 stop` and press Enter. Click the plain
   window and wait eight seconds.

   Result: no line arrives for the end of a turn.

4. Click the panel `Attention stand-in 1`, type `after 6 ask`. Click the plain
   window and wait eight seconds.

   Result: a line arrives for the question.

### 6.5 N5 — A name with markup and a long name

1. Choose `When an agent needs me or is done` in the settings again.

2. Double-click the title bar of the panel `Attention stand-in 1`, type
   `<b>Fix</b> & more than sixty characters of a name that an agent writes` and
   press Enter.

   Result: the title bar holds the new name.

3. Click the panel `Attention stand-in 1`, type `after 6 ask`. Click the plain
   window and wait eight seconds.

   Result: the next notification holds the escaped text `&lt;b&gt;Fix&lt;/b&gt;
   &amp; more`, one line, and at most sixty characters of the name.

## 7. Pass criteria

- Each task holds its result.
- No notification arrives while the Horizon window has the focus. Every task that
  types into a panel gives the focus to the plain window before the change.
- Each line of `notifications.log` holds the arguments of one call, and the text
  stands after `--`.
- No line of the log holds a prompt, a tool argument or a path.

## 8. Cleanup

1. Close the Device panel in Horizon with the `device_panel` operation `close`.

   Result: the viewer is gone and the fixture still runs.

2. Stop the fixture with Ctrl-C in its terminal.

   Result: the children of the fixture end and the private home is removed.

> **CAUTION:** REMOVE THE STATE FOLDER OF THE FIXTURE. `rm -rf` deletes every file
> of that folder and nothing can restore it. Give the path that you created for
> this run, and no other path.

3. Remove the state folder.

   ```bash
   rm -rf /var/tmp/notifications-1
   ```

   Result: the folder is gone.

## 9. Record of results

Write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md) when the run must be kept.
Otherwise put the results in the pull request. Keep the evidence private until
publication is authorized.
