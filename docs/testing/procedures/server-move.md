---
procedure: server-move
feature: The Horizon server moves from one computer to another computer
platforms: [linux, macos]
cost: none
destructive: yes
secrets: the SSH login of the server computer, the machine keys and the named secrets of the private Horizon home of each computer
owner: peters
---

# Server move test procedure

## 1. Purpose

This procedure proves that a server moves from one computer to another computer
with its store, its machines, its keys, its values and the logins of its
computers. It proves that a computer keeps its machine list and settles the
changes that waited, that a new computer reads every machine and every named
secret after the move, and that the store id stays the same. It proves that the
move back works the same way.

## 2. Applicability

- Candidate: a build of `horizon` that has the `--server` mode, the
  `--server-link` mode and the `--server-invite` mode.
- Platforms: two computers, each one Linux or macOS. This procedure names the
  move from macOS to Linux, and the move back. Run each lane on the operating
  systems that it names.
- The run uses the agent `net.horizon.server.test` or the unit
  `horizon-server-test`, and a private server home. It therefore does not touch
  an agent `net.horizon.server` or a unit `horizon-server` of the person.
- This procedure does not test a copy of a server that runs, a restore from a
  continuous copy, or a removal of a computer.

## 3. Safety

> **CAUTION: START THE SERVER OF THIS RUN ONLY, AND STOP IT BEFORE THE FIRST COPY.**
> Use computers that you own and an SSH login that you may use for this test. Do
> not copy the server home of a person who uses it.

> **CAUTION: WRITE ONLY THE STATE FOLDERS AND THE SERVICE FILES THAT THIS
> PROCEDURE NAMES.** Do not change the agent, the unit or the server home of your
> own work.

> **CAUTION: DO NOT START THE OLD SERVER AFTER THE MOVE.** Two servers with one
> store id and two contents decide the machine list twice.

## 4. Equipment and preconditions

- Two computers that you own: the first one, named `first`, and the second one,
  named `second`. Each one has a login that you reach with `ssh`.
- The candidate `horizon` on both computers, at a path that you know, with its
  SHA-256.
- One task-owned state folder on each computer, for example
  `$HOME/horizon-move-test`. The variable `STATE` names it below.
- A port on each computer that no process uses, for example `17872` on `first`
  and `17873` on `second`.
- An SSH configuration on the client computer that reaches the server computer
  without a prompt.
- The program `sqlite3` on the server computer, to read the store id. Skip both
  store-id steps when it is not there, and pass that lane by the machine list and
  the sealed values alone.
- One named secret with a value that you know in the private home of the client
  computer, made as [the named secrets procedure](named-secrets.md) says.
- No agent `net.horizon.server.test` and no unit `horizon-server-test` on either
  computer.

## 5. Setup

1. Freeze the candidate. Record the SHA-256 of the candidate on both computers.

2. On `first`, make the private server home and the folder of the log:

   ```sh
   export STATE="$HOME/horizon-move-test"
   mkdir -p "$STATE/home/log" "$STATE/user"
   chmod 700 "$STATE/home"
   ```

   Result: the folder `$STATE/home` exists at mode 700.

3. On `first`, make the service of this run. The text is the file of the section
   [Run the server at login](../../../packaging/horizon-server/README.md#run-the-server-at-login).
   Replace each `CHANGE-ME` with the user name of `first`, and change these four
   items:

   - the label is `net.horizon.server.test`;
   - the command holds `--server`, `--listen` and `127.0.0.1:17872`;
   - the environment line of the private home is not a comment, and it sets
     `HORIZON_SERVER_HOME` to `$STATE/home`;
   - the two log paths name `$STATE/home/log/server.log` on macOS. The Linux unit
     writes the journal instead.

   Result: the service file names the user of `first`, the private home and the
   listener of the loopback address.

4. Start the service on `first`, and read its log until it holds the line
   `the Horizon server answers`.

5. On the client computer, start the candidate with a private home of this run,
   and set the fields of **Settings**, **Server**, **This computer** to reach the
   server of `first`, as [the server guide](../../../packaging/horizon-server/README.md#set-the-server-on-each-computer)
   says. On Linux, start the candidate through
   [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
   and watch the window in a Device panel.

   Result: the sidebar reads `Server: in step`.

6. Add one machine with the agent of a panel, as
   [the machines procedure](sidebar-machines.md) says, and read the sidebar.

   Result: the row of the machine holds no `pending` after a moment.

## 6. Tasks

### 6.1 M1 — Stop the old server and copy its home

1. On the client computer, ask the agent for a machine while the link is up, then
   stop the server on `first` at once, with the stop step of its system. On
   macOS:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server.test"
   ```

   On Linux:

   ```sh
   systemctl --user disable --now horizon-server-test
   ```

2. Read the log of `first` until it holds this line:

   ```sh
   tail -n 20 "$STATE/home/log/server.log"
   ```

   Result: the log holds `the Horizon server stopped`, `$STATE/home` holds
   `server.sqlite` and `secret.key`, the socket is gone, and the `-wal` file is
   empty or absent.

3. On `first`, read the store id of the stopped store:

   ```sh
   sqlite3 "$STATE/home/server.sqlite" "select value from meta where key = 'store_id';"
   ```

   Result: the command prints 36 characters. Record them.

4. Copy the store to the server home of `second`, as step 2 of [Move the server to
   another computer](../../../packaging/horizon-server/README.md#move-the-server-to-another-computer)
   says. Make the home of `second` first, and copy the two files with `scp -p`.

   Result: the server home of `second` holds `server.sqlite` and `secret.key`, each
   at mode 600, in a folder of mode 700. It holds no `server.lock` and no
   `server.sock`.

### 6.2 M2 — Start the server on the new computer

1. On `second`, make the service of this run, as step 3 of the setup says, with
   the user name of `second`, the listener `127.0.0.1:17873`, and the same
   `HORIZON_SERVER_HOME` folder of this run.

2. Start the service on `second`, and read its log.

   Result: the log holds `the Horizon server answers` with the new socket and the
   listener `127.0.0.1:17873`. It holds no line about a store that cannot be
   opened.

3. On `second`, ask for one invitation:

   ```sh
   HORIZON_SERVER_HOME="$STATE/home" <path>/horizon --server-invite
   echo "exit: $?"
   ```

   Result: the command prints a link, a code and a QR code, and the exit code is
   0. The code is made from the store of the move, so the server reads it.

### 6.3 M3 — Each computer takes the new target

1. On the client computer, open **Settings** (**Ctrl+Shift+,**), select the
   **Server** tab, and set **SSH target** and **Command** to the names of
   `second`. Press **Save**, and start the candidate again.

   Result: the sidebar reads `Server: in step`, and the machine list holds the
   machine of the setup and the machine that step 1 of M1 added.

2. Read the sidebar before the server of `second` answers again: stop the service
   on `second` and ask the agent of a panel for one machine.

   Result: the sidebar reads `Server away · 1 pending`, and the row of that
   machine shows `pending`.

3. Start the service on `second` again.

   Result: the sidebar reads `Server: in step` within a few seconds, and the row
   of the machine holds no `pending`. The change that waited settled by itself.

4. Add one machine with the agent of a panel.

   Result: the row holds no `pending` after a moment, and the list of the
   computers of the **Server** tab holds the client computer.

### 6.4 M4 — A new computer reads the moved server

1. On a third computer, or with a second private Horizon home on the client
   computer, start the candidate, and join it with a code of the invitation of
   step 3 of M2, as [Join a computer with a QR code](../../../packaging/horizon-server/README.md#join-a-computer-with-a-qr-code)
   says.

   Result: the sidebar of that computer reads `Server: in step`.

2. Read the machine list and the named secret of that computer.

   Result: the list holds every machine of the moved store, and the named secret
   holds the value that the setup wrote. A key of a machine that the moved
   `secret.key` sealed opens there, and its panel logs in to that machine.

3. Ask for one machine from the new computer, and read the first computer.

   Result: the row arrives on the first computer within a few seconds.

### 6.5 M5 — Move the server back

1. Stop the service on `second` and read its log until it holds
   `the Horizon server stopped`. Confirm that `second` shows no socket.

2. Copy `server.sqlite` and `secret.key` of `second` to the server home of
   `first`, as step 2 of [Move the server to another computer](../../../packaging/horizon-server/README.md#move-the-server-to-another-computer)
   says.

3. Start the service on `first` again, and read its log.

   Result: the log holds `the Horizon server answers`. The listener is
   `127.0.0.1:17872` again.

4. On the client computer, set **SSH target** and **Command** to the names of
   `first`, press **Save**, and start the candidate again.

   Result: the sidebar reads `Server: in step`. The machine list holds every
   machine of the moved store, and no row shows `pending`.

5. Stop both services, and read the store id of the server home on `first`, as
   step 3 of M1 says.

   Result: the value is the one that step 3 of M1 recorded. The move kept the
   store.

### 6.6 M6 — A computer reads the line of another store

1. With the server of `first` stopped, copy its `server.sqlite` to a scratch
   folder, and delete `server.sqlite` and `secret.key` from the server home of
   `first`.

   > **CAUTION: DO THIS STEP ONLY ON THE PRIVATE HOME OF THIS RUN.** These two
   > files are the whole state of a server.

2. Start the service on `first`, and read its log.

   Result: the log holds `the Horizon server answers` with a new store id.

3. On the client computer, read the sidebar within a few seconds.

   Result: the sidebar shows the line about another store: the machine list and
   the settings of this computer are now the ones of that store. The list is
   empty, because the new store holds no machine.

4. Stop the service, put the two saved files back in the server home of `first`,
   start it again, and read the sidebar of the client computer.

   Result: the sidebar shows the line about another store one time more, and the
   machine list holds the machines of the restored store again.

## 7. Pass criteria

- The log of each server holds `the Horizon server answers` and
  `the Horizon server stopped` at the steps that name them.
- The client computer reads `Server: in step` after each start, and it keeps its
  machine list.
- A change that waited while the server was away settles by itself.
- A new computer reads every machine, every named secret and a machine key of the
  moved store.
- The store id before the move is the one after the move back.
- The line about another store appears for a new store and not for a move.

## 8. Cleanup

1. Stop both services and remove them from the login items of their computers.

2. Remove the folders that this run made: `$STATE` on both computers, the service
   files, and the private Horizon home of the new computer of M4.

3. Confirm that no server process of this run is left:

   ```sh
   ps -C horizon -o pid=,args=
   ```

   Result: no process holds `--server` of a `$STATE` home.

## 9. Record of results

Write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md), or put the results in the pull
request. Keep private evidence out of the repository.
