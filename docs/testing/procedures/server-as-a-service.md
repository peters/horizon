---
procedure: server-as-a-service
feature: The Horizon server runs at login as a service of the person
platforms: [linux, macos]
cost: none
destructive: yes
secrets: the SSH login of the server computer, and the machine keys of the private Horizon home of the client computer
owner: peters
---

# Server as a service test procedure

## 1. Purpose

This procedure proves that the Horizon server runs at login as a service of the
person, that its server home survives a restart of the service, and that the
other computers are in step with it again after that restart. It proves that the
invitation answers while the service runs, that a stopped service answers one
line, and that a removal of the service leaves no server process.

## 2. Applicability

- Candidate: a build of `horizon` that has the `--server` mode, the
  `--server-link` mode and the `--server-invite` mode.
- Platforms: the server computer is Linux or macOS. Run the macOS lane on macOS
  and the Linux lane on Linux.
- The run uses the agent `net.horizon.server.test` and the unit
  `horizon-server-test`, and a private server home. It therefore does not touch
  an agent `net.horizon.server` or a unit `horizon-server` of the person.
- This procedure does not test the listener of a private overlay network, a
  removal of a computer, a linger of the user, or a reboot of the computer.

## 3. Safety

> **CAUTION: START THE SERVER AND THE LINK OF THIS RUN ONLY.** Use a computer
> that you own, and an SSH login that you may use for this test. Do not use the
> login of another person.

> **CAUTION: WRITE ONLY THE SERVICE FILES AND THE STATE FOLDER OF THIS RUN.** The
> agent file, the unit file and the state folder are the ones that this procedure
> names. Do not change an agent, a unit or a home of your own work.

## 4. Equipment and preconditions

- Two computers. The server computer has a Linux or macOS login that you reach
  with `ssh`. The client computer runs the candidate.
- The candidate `horizon` on both computers, at a path that you know, with its
  SHA-256.
- One task-owned state folder on the server computer, for example
  `$HOME/horizon-service-test`. The variable `STATE` names it below.
- One task-owned state path on the client computer for the private home of the
  candidate, or
  [the fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  with a live view in a Device panel when the run is on Linux.
- An SSH configuration on the client computer that reaches the server computer
  without a prompt. The candidate reads the one of the person, because its private
  home does not change `HOME`.
- A port on the server computer that no process uses, for example `17871`.
- One agent panel on the client computer whose agent can call the `block_panel`
  tool with the `machine_set` operation, as
  [the machines procedure](sidebar-machines.md) says.
- No agent `net.horizon.server.test` and no unit `horizon-server-test` on the
  server computer.

## 5. Setup

1. On the client computer, freeze the candidate. On the server computer, record
   the SHA-256 of the candidate.

2. On the server computer, make the state folder and the private server home:

   ```sh
   export STATE="$HOME/horizon-service-test"
   mkdir -p "$STATE/home/log" "$STATE/user"
   chmod 700 "$STATE/home"
   ```

   On macOS, make the folder of the agents. On Linux, make the folder of the
   units:

   ```sh
   mkdir -p "$HOME/Library/LaunchAgents"
   ```

   ```sh
   mkdir -p "$HOME/.config/systemd/user"
   ```

   Result: the folder `$STATE/home` exists at mode 700.

3. On the server computer, make the file of the service. The text is the file of
   the section [Run the server at login](../../../packaging/horizon-server/README.md#run-the-server-at-login).
   Replace each `CHANGE-ME` with the user name of the server computer, and change
   these four items:

   - the label is `net.horizon.server.test`, in the name of the file and in the
     `Label` key;
   - the command holds `--server`, `--listen`, `127.0.0.1:17871`, `--name` and
     `test-server`: in the `ProgramArguments` array on macOS, and at the end of
     `ExecStart` on Linux;
   - the environment line of the private home is not a comment, and it sets
     `HORIZON_SERVER_HOME` to `$STATE/home`;
   - the two log paths name `$STATE/home/log/server.log`. The Linux unit writes
     the standard output and the standard error to the journal, so it needs no
     path.

   On macOS the file is `$HOME/Library/LaunchAgents/net.horizon.server.test.plist`.
   On Linux the file is `$STATE/user/horizon-server-test.service`, and a link of
   it is in the folder of the person:

   ```sh
   ln -s "$STATE/user/horizon-server-test.service" "$HOME/.config/systemd/user/horizon-server-test.service"
   ```

   Result: the file names the user of the server computer, the private home and
   the listener of the loopback address.

4. On the client computer, start the candidate with the private home of this run:

   ```sh
   HORIZON_HOME=<state path>/home <path>/horizon
   ```

   On Linux, start it through the fixture instead, as
   [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
   says, and watch the window in the Device panel.

   Result: the candidate window shows the board. The sidebar shows no `Machines`
   section, because the home names no server and holds no machine.

## 6. Tasks

### 6.1 S1 — Start the service

1. On the server computer, start the service. On macOS:

   ```sh
   launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/net.horizon.server.test.plist"
   launchctl print "gui/$(id -u)/net.horizon.server.test" | head -20
   tail -n 20 "$STATE/home/log/server.log"
   ```

   On Linux:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now horizon-server-test
   systemctl --user status horizon-server-test
   journalctl --user -u horizon-server-test -n 20
   ```

   Result: the state is `running` on macOS and `active (running)` on Linux. The
   log holds the line `the Horizon server answers` with the home, the socket and
   the listener. `$STATE/home` holds `server.sqlite`, `secret.key`, `server.sock`
   and `server.lock`.

2. On the server computer, read the socket line of the log.

   Result: the line names `$STATE/home/server.sock` and `127.0.0.1:17871`.

### 6.2 S2 — The invitation answers

1. On the server computer, ask for one invitation:

   ```sh
   HORIZON_SERVER_HOME="$STATE/home" <path>/horizon --server-invite
   echo "exit: $?"
   ```

   Result: the command prints the link, the code, the minutes that it lives and
   the QR code of the link as blocks of text. The link holds `test-server:17871`.
   The exit code is 0.

### 6.3 S3 — A computer joins

1. On the client computer, open **Settings** (**Ctrl+Shift+,**) and select the
   **Server** tab.

2. Set the two fields of **This computer**:

   - **SSH target**: the SSH name of the server computer;
   - **Command**: the absolute path of the candidate on the server computer and
     `--server-link`, for example `/Users/<user>/bin/horizon --server-link`.

3. Close the candidate window, and start the candidate again with the same
   private home, as step 4 of the setup says.

   Result: the window opens with the same board, and the link of the settings
   starts.

4. Read the **Machines** section of the sidebar.

   Result: the line `Server: in step` is there, and the **Server** tab lists this
   computer under the computers of the server.

### 6.4 S4 — A machine arrives on the server

1. On the client computer, ask the agent of a panel for a machine with
   `machine_set`, or send this request, as
   [the machines procedure](sidebar-machines.md) says:

   ```json
   {"operation": "machine_set", "name": "service-test-1", "ssh": {"host": "203.0.113.10"}}
   ```

   Result: the row of the machine shows `pending` for a moment, and then no
   `pending`.

2. Read the state of the link.

   Result: the sidebar reads `Server: in step`.

### 6.5 S5 — A restart of the service keeps the home

1. On the server computer, restart the service. On macOS:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server.test"
   launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/net.horizon.server.test.plist"
   ```

   On Linux:

   ```sh
   systemctl --user restart horizon-server-test
   ```

   Result: the log holds `the Horizon server stopped` and then a new
   `the Horizon server answers` line. The state is `running` again.

2. On the server computer, examine the files of the home:

   ```sh
   ls -l "$STATE/home"
   ```

   Result: `server.sqlite` and `secret.key` are there with the times that they had
   before the restart. `server.sock` and `server.lock` are there again.

### 6.6 S6 — The computer is in step again

1. On the client computer, read the **Machines** section of the sidebar.

   Result: the line `Server: in step` is there again, and the machine
   `service-test-1` is in the list. The service kept the store across its restart.

2. On the client computer, ask for a second machine, named `service-test-2`, with
   an address such as `203.0.113.11`, and the request of step 1.

   Result: the row shows `pending` for a moment, and then no `pending`. The
   sidebar reads `Server: in step`.

### 6.7 S7 — Stop the service

1. On the server computer, stop the service. On macOS:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server.test"
   ```

   On Linux:

   ```sh
   systemctl --user disable --now horizon-server-test
   ```

   Result: the server receives `SIGTERM`, closes its store and removes its socket.
   The log holds the line `the Horizon server stopped`.

2. On the server computer, ask for an invitation again:

   ```sh
   HORIZON_SERVER_HOME="$STATE/home" <path>/horizon --server-invite
   echo "exit: $?"
   ```

   Result: the command prints one line about the home that no server answers, and
   the exit code is 1.

3. On the client computer, read the **Machines** section of the sidebar.

   Result: the line is `Server: away`, and the machine list of the client computer
   stays.

### 6.8 S8 — Remove the service

> **CAUTION: REMOVE ONLY THE AGENT OR THE UNIT OF THIS RUN.** The agent
> `net.horizon.server.test` and the unit `horizon-server-test` are the files of
> this run. A removal of the agent `net.horizon.server` or the unit
> `horizon-server` stops the server of your own work.

1. On the server computer, remove the file of the service. On macOS:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server.test" 2>/dev/null
   rm "$HOME/Library/LaunchAgents/net.horizon.server.test.plist"
   ```

   On Linux:

   ```sh
   systemctl --user disable --now horizon-server-test 2>/dev/null
   rm "$HOME/.config/systemd/user/horizon-server-test.service"
   systemctl --user daemon-reload
   ```

   Result: no agent or unit of this run is left.

2. On the server computer, read the state one time more. On macOS:

   ```sh
   launchctl print "gui/$(id -u)/net.horizon.server.test" | head -5
   ```

   On Linux:

   ```sh
   systemctl --user status horizon-server-test
   ```

   Result: macOS answers that there is no service of that label. Linux answers
   that the unit is not found.

## 7. Pass criteria

- S1: the service runs at login, and the home holds the four files.
- S2: the invitation prints the link, the code and the QR code, and the exit code
  is 0.
- S3: the client computer joins over SSH, and the sidebar reads `Server: in step`.
- S4: a machine of the client computer settles on the server.
- S5: a restart of the service keeps `server.sqlite` and `secret.key`.
- S6: the client computer is in step again, and a second machine settles.
- S7: the stopped service answers one line, the exit code is 1, and the client
  computer reads `Server: away`.
- S8: no agent and no unit of the run is left on the server computer.

## 8. Cleanup

1. On the server computer, remove the state folder of this run:

   > **CAUTION:** REMOVE ONLY THE STATE FOLDER OF THIS RUN. The folder holds the
   > store and the key of the test server, and its path is the one in `STATE`.

   ```sh
   rm -rf "$STATE"
   ```

2. On the client computer, close the Device panel of the live view when the run
   watched one, and stop the candidate and its private home.

3. On the client computer, remove the server entry of **Settings** → **Server**,
   so the private home names no server again.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
