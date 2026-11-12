---
procedure: macos-window-without-input
feature: A macOS window of the candidate that a tester drives with files, when no program may send keys or clicks
platforms: [macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# macOS window without input test procedure

## 1. Purpose

This procedure proves that a person can check a candidate in a real macOS window
when the tester can take screen pictures over SSH but cannot send input: the
window opens, a panel prints, an agent panel calls the tools of the candidate
from the lines of a command file, and the run stops clean. It also says what such
a run cannot prove.

## 2. Applicability

- Candidate: a build of `horizon` for macOS, from the branch that this run
  checks.
- Platforms: macOS with a logged-in graphical session, an SSH login to the same
  computer and the Screen Recording permission for that login, so that
  `screencapture` writes a picture.
- This procedure does not test: a key, a click, a drag, the speech key, the
  focus of a window, or a picture that a person sees live. Section 7 gives the
  steps that these need.

## 3. Safety

> **CAUTION:** USE A PRIVATE HOME FOLDER AND A NEW CONFIG FILE. The run writes
> its settings, its sessions and its records into `HORIZON_HOME`. Do not point it
> at the real `~/.horizon` of the person.

> **CAUTION:** STOP ONLY THE PROCESS ID THAT YOU STARTED. Do not use `pkill` or
> `killall`: another Horizon of the person can hold the same program name.

> **CAUTION:** DO NOT MAKE A VNC VIEWER FOR THE DESKTOP OF THE PERSON. A viewer of
> this procedure reads an isolated desktop only.

## 4. Equipment and preconditions

- The source checkout, and a build of it:

  ```sh
  cargo build -p horizon-ui --bin horizon
  ```

  Result: The binary is at `target/debug/horizon`. This is `<candidate>` below.

- A private folder, `<private>`, outside the checkout, such as
  `/Users/<tester>/horizon-no-input`. This is the value of `HORIZON_HOME`.
- The checkout of the repository at `<checkout>`, which holds
  `scripts/device-smoke/file_agent.py` and `scripts/device-smoke/card_agent.py`.
- A VNC endpoint on `127.0.0.1` that a disposable desktop of this Mac serves,
  such as a virtual machine on the same Mac. This is `<vnc>` below, for example
  `127.0.0.1:5900`. The endpoint must serve a desktop of its own, and not the
  desktop of the person.
- An SSH login to the Mac whose input the Accessibility settings block. Screen
  Recording alone is enough for this procedure.

## 5. Setup

1. Make the private folder and the two files of the file agent:

   ```sh
   mkdir -p <private>
   : > <private>/commands.jsonl
   : > <private>/answers.jsonl
   ```

   Result: Both files are empty.

2. Write the launcher of the file agent, `<private>/file-agent.sh`:

   ```sh
   #!/bin/sh
   exec /usr/bin/python3 -u <checkout>/scripts/device-smoke/file_agent.py \
     --mcp-command "<candidate> --browser-mcp" \
     --commands <private>/commands.jsonl \
     --answers <private>/answers.jsonl "$@"
   ```

   Then make it executable:

   ```sh
   chmod 700 <private>/file-agent.sh
   ```

   Result: The script runs the file agent, and `"$@"` takes the arguments that
   Horizon adds to an agent panel.

3. Write the config file, `<private>/horizon.json`:

   ```json
   {
     "version": 11,
     "window": {"width": 1200, "height": 800},
     "appearance": {"theme": "dark"},
     "workspaces": [
       {"name": "No input", "cwd": "<private>",
        "terminals": [
          {"name": "Clock", "command": "/bin/sh",
           "args": ["-c", "while true; do date; sleep 2; done"],
           "position": [40, 60], "size": [520, 380]},
          {"name": "File agent", "kind": "claude",
           "command": "<private>/file-agent.sh", "args": [], "cwd": "<private>",
           "position": [600, 60], "size": [520, 380]}
        ]}
     ]
   }
   ```

   Result: The config holds one shell panel that prints the date every two
   seconds, and one agent panel that runs the launcher of step 2. The shape is
   the shape that the device smoke fixture writes: `version`, `window`,
   `appearance`, and `workspaces` with a `terminals` list.

4. Start the candidate in the graphical session of the logged-in person. An SSH
   login is not in that session, so `launchctl asuser` gives the window to the
   person who is logged in:

   ```sh
   launchctl asuser $(id -u) env \
     HORIZON_HOME=<private>/home \
     HORIZON_AGENT_INTEGRATIONS=off \
     <candidate> --config <private>/horizon.json --ephemeral &
   echo $!
   ```

   Result: The command prints one number. This is `<pid>` below.
   `HORIZON_AGENT_INTEGRATIONS=off` keeps the run out of the home folder of the
   person: it installs no skill, no plugin and no desktop entry.

5. Take a picture of the whole screen, and keep it as evidence:

   ```sh
   screencapture -x <private>/window.png
   ```

   Result: `window.png` shows the window of Horizon with the two panels. Read
   the position and the size of each panel from that picture.

## 6. Tasks

### 6.1 MWI-1 — the window opens and a panel prints

1. Take a picture of the panel `Clock`:

   ```sh
   screencapture -x -R<x,y,w,h> <private>/clock.png
   ```

   Result: The picture holds a date line, and a second picture two seconds later
   holds a later line. The shell panel runs and shows its output.

2. Take a picture of the panel `File agent`.

   Result: The picture holds the line `file-agent: commands <private>/commands.jsonl`,
   the line `file-agent: answers <private>/answers.jsonl` and a count of the
   tools of the MCP server. The panel has no error line and no traceback.

### 6.2 MWI-2 — a machine that the agent sets shows in the sidebar

1. Write one command into the command file:

   ```sh
   printf '%s\n' \
     '{"tool": "machine_set", "arguments": {"name": "build-1", "ssh": {"host": "127.0.0.1"}}}' \
     '{"tool": "machines"}' >> <private>/commands.jsonl
   ```

   Result: Within one second the panel holds one line that starts
   `file-agent: machine_set -> ` and one line that starts
   `file-agent: machines -> `, and the second line holds `build-1`.

2. Take a picture of the sidebar, and of the panel `File agent`.

   Result: The sidebar holds the row `build-1` under **Machines**, and its tooltip
   or its line names the route `127.0.0.1`.

3. Read the answer file:

   ```sh
   cat <private>/answers.jsonl
   ```

   Result: Two lines, each one JSON object with `request` and `answer`. The
   answer of `machine_set` holds `"created": true`, and the answer of `machines`
   holds the record of `build-1`.

4. Forget the machine again:

   ```sh
   printf '%s\n' '{"tool": "machine_delete", "arguments": {"name": "build-1"}}' \
     >> <private>/commands.jsonl
   ```

   Result: The row `build-1` goes from the sidebar, and the answer file gains one
   line with `{"status": "machine_deleted", "name": "build-1"}`.

### 6.3 MWI-3 — a browser panel that the agent makes paints a page

> **CAUTION:** THE PAGE BELOW IS A LOOPBACK ADDRESS OF THIS COMPUTER. Start a page
> server on it, or use a page that needs no network. Do not send a page of a
> customer or of a private system.

1. Start a page server in the private folder, and keep its process id:

   ```sh
   cd <private> && /usr/bin/python3 -m http.server 8765 --bind 127.0.0.1 &
   echo $!
   ```

   Result: The server prints one listening line. Put a page `index.html` with a
   clear heading in the folder before this step.

2. Write the command that makes a browser panel:

   ```sh
   printf '%s\n' \
     '{"tool": "browser_create", "arguments": {"url": "http://127.0.0.1:8765/"}, "timeout": 60}' \
     >> <private>/commands.jsonl
   ```

   Result: The panel holds one line that starts `file-agent: browser_create -> `,
   and its answer holds a `panel_id` and the navigation state `committed`.

3. Take a picture of the workspace.

   Result: A browser panel stands beside the agent panel, and its body shows the
   heading of the page. The page paints and no error page is there.

4. Close that panel with one more command:

   ```sh
   printf '%s\n' '{"tool": "browser_close", "arguments": {"panel_id": "<panel_id>"}}' \
     >> <private>/commands.jsonl
   ```

   Result: The panel goes, and the answer file gains the answer of `browser_close`.

### 6.4 MWI-4 — a VNC viewer that the agent makes connects

1. Write the command that makes a Device panel of `<vnc>`:

   ```sh
   printf '%s\n' \
     '{"tool": "device_panel", "arguments": {"operation": "create", "endpoint": "<vnc>"}}' \
     >> <private>/commands.jsonl
   ```

   Result: The answer holds one `panel_id`, and the state of the viewer is
   `connected` after the first picture.

2. Wait two seconds, then write the command that inspects the viewer:

   ```sh
   printf '%s\n' \
     '{"tool": "device_panel", "arguments": {"operation": "inspect", "panel_id": "<panel_id>"}}' \
     >> <private>/commands.jsonl
   ```

   Result: The answer holds `image_received: true` and a `frame_sequence` that is
   above zero. A second inspect after two seconds answers a higher
   `frame_sequence` while that desktop changes.

3. Take a picture of the Device panel.

   Result: The panel shows the desktop of `<vnc>`.

### 6.5 MWI-5 — a card of `block_panel` draws

1. Write the command that draws one card:

   ```sh
   printf '%s\n' \
     '{"tool": "block_panel", "arguments": {"operation": "create", "title": "No input check", "document": {"components": [{"type": "heading", "text": "No input check"}, {"type": "text", "text": "This card came from a command file."}]}}}' \
     >> <private>/commands.jsonl
   ```

   Result: The answer holds `"revision": 1` and one `panel_id`.

2. Take a picture of the workspace.

   Result: The card stands beside the agent panel, with the heading `No input
   check` and the text below it. Its buttons are drawn, because the agent panel
   of this card is in the workspace.

3. Close the card with one more command, with the operation `close` and its
   `panel_id`.

   Result: The card goes, and the answer names the closed card.

### 6.6 MWI-6 — a clean stop

1. Stop the candidate by the process id that you started:

   ```sh
   kill -TERM <pid>
   ```

   Result: The window goes, and no process of the candidate stays:

   ```sh
   ps -p <pid>
   ```

   Result: The command prints no line.

2. Read the panel lines of the run, from the terminal that started it.

   Result: No line of a panic and no traceback.

## 7. What this procedure cannot prove

This run sends no key and no click, so it cannot prove any behaviour that a key
or a click starts. That covers every shortcut, the focus of a panel, the
selection of a row or a menu, a drag of a panel or a window, a rename, a resize,
and the speech key. Those need one of two conditions:

- the Accessibility permission for the SSH login, so a program of the test can
  send keys and clicks; then the automation of the other macOS procedures
  applies;
- a person at the Mac, who does the steps of the procedure that names the
  feature.

A picture that `screencapture` writes also proves the state of one moment. It
does not prove motion, and it does not prove what a person sees live: the
AGENTS rules on a live Device panel still apply to an interactive test of the
candidate on this computer.

## 8. Pass criteria

- The window opens in the graphical session of the logged-in person.
- The shell panel prints, and a later picture shows later output.
- The panel of the file agent holds one line for each command of the command
  file, and the answer file holds each request with its answer.
- A machine that the agent sets shows in the sidebar, and the Forget command
  removes it.
- A browser panel that the agent makes paints the page.
- A Device panel that the agent makes connects, and its `frame_sequence` rises
  while the desktop changes.
- A card of `block_panel` draws in the workspace, and its close removes it.
- The candidate stops on its own process id, and no process of the run stays.

## 9. Cleanup

1. Stop the page server of task MWI-3 by its process id.

   Result: No listener of the run stays.

2. Delete the private folder, and with it the private home, the config file, the
   two files of the file agent and the pictures of the run.

   Result: No file and no record of the run stays.

3. If the run made a machine with a key, delete that key in **Settings**.

   Result: The private home holds no key of the run.

## 10. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the pictures and the private home out of the repository.
Name no real host, no person and no customer.
