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
  `--notifications-log` and `--plain-window`.
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

1. Start the fixture with the stand-in agent, the notification log and the plain
   window.

   ```bash
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --state /var/tmp/notifications-1 --native-view --attention-agent 0 \
     --notifications-log --plain-window
   ```

   Result: the isolated desktop holds a Horizon window, a plain window and a
   panel named Notifications. The terminal prints the VNC address.

2. Open the VNC address in a Horizon native Device panel.

   Result: the image of the isolated desktop shows. The panel advances its frame
   sequence while the desktop changes.

## 6. Tasks

Give each task the ID that follows. A report gives one result for each ID.

### 6.1 N1 — A change that a person in another window receives

1. Click the plain window, so that it has the focus of the desktop.

2. In the Attention stand-in panel, type `ask` and press Enter.

   Result: the notification line names the panel and says `needs you`. Wait two
   seconds for the group of the changes.

3. Read the bottom line of the Notifications panel.

   Result: one line of JSON holds `--app-name=Horizon`, then `--`, then
   `Horizon`, then the name of the panel and `needs you`. The file holds one
   line.

### 6.2 N2 — A change that a person in Horizon does not receive

1. Click the Horizon window, so that it has the focus of the desktop.

2. In the Attention stand-in panel, click the row of the panel in the sidebar,
   then type `stop` and press Enter. Type `start`, then `stop`, to end a turn.

   Result: the sidebar says `Done`, and the Notifications panel shows no new
   line after two seconds. The sidebar is the only place that shows the change.

### 6.3 N3 — Several changes become one line

1. Click the plain window.

2. Type `start`, then `ask`. Then add a second agent panel of the same stand-in,
   and type `ask` in it.

   Result: the Notifications panel shows one line for the changes that arrived
   within two seconds of each other, and its body is a count such as
   `2 agents need you`. Horizon sends no line for each change alone.

### 6.4 N4 — The setting

1. Open Settings, then General, and choose `Off` under Desktop notifications.

   Result: the value is saved.

2. Click the plain window, and type `answer`, then `ask`, in the stand-in panel.

   Result: the Notifications panel shows no new line.

3. Choose `When an agent needs me`, and repeat step 2 with a turn that ends.

   Result: no line arrives for the end of a turn, and a line arrives for a
   question.

### 6.5 N5 — A name with markup and a long name

1. Choose `When an agent needs me or is done` in the settings again.

2. Set the terminal title of the stand-in panel to `<b>Fix</b> & more than sixty
   characters of a name that an agent writes`.

   Result: the next notification holds the escaped text `&lt;b&gt;Fix&lt;/b&gt;
   &amp; more`, one line, and at most sixty characters of the name.

## 7. Pass criteria

- Each task holds its result.
- No notification arrives while the Horizon window has the focus.
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
