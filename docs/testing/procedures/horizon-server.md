---
procedure: horizon-server
feature: One machine list on two computers, kept by a Horizon server
platforms: [linux, macos]
cost: none
destructive: no
secrets: the SSH login of the server computer, and the private keys of the machine keys in the Horizon home of each computer
owner: peters
---

# Horizon server test procedure

## 1. Purpose

This procedure proves that two computers show one machine list. It proves that a
machine that one computer adds appears on the other computer, that a removal
travels back, that a change waits while the server is away, and that the change
settles when the server returns. It also proves that a computer without a server
in its settings behaves as before.

## 2. Applicability

- Candidate: a build of `horizon` that has the `--server` mode, the
  `--server-link` mode and the `server` settings entry.
- Platforms: the server computer is Linux or macOS. A client computer is Linux
  or macOS.
- This procedure does not test a rented machine, a machine key on the server, a
  card, a click that waits for an agent, or two servers.
- This procedure does not test a computer that is away for more than five
  minutes.

## 3. Safety

> **CAUTION: EACH TASK BELOW STARTS A PROCESS ON THE SERVER COMPUTER AND OPENS
> AN SSH LOGIN TO IT.** Use a computer that you own, and an SSH login that you
> may use for this test. Do not use the login of another person.

> **CAUTION: THE TEST WRITES TO A PRIVATE HORIZON HOME ONLY.** Start each client
> computer through the fixture, which sets its own `HORIZON_HOME` and its own
> desktop. Do not start it with your own home.

> **CAUTION: THE TEST FORGETS MACHINE RECORDS.** Use a private test home, so no
> record of your own work is lost.

## 4. Equipment and preconditions

- A second computer, here called the server computer, with a Linux or macOS
  login that you can reach with `ssh`.
- The candidate `horizon` binary on the server computer, at a path that you
  know, with its SHA-256.
- The candidate `horizon` binary on each client computer, with its SHA-256.
- An isolated test desktop for each client computer, viewed through a Horizon
  native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  describes. Give each run its own `--state` folder.
- An SSH configuration on each client computer that reaches the server computer
  without a prompt. The fixture copies the configuration and the identity that
  you give it with `--ssh-config` and `--ssh-identity`.

## 5. Setup

1. On the server computer, make the server home and start the server:

   ```bash
   export HORIZON_SERVER_HOME="$HOME/.horizon-server-test"
   /path/to/horizon --server
   ```

   Result: the process stays in the foreground and writes one line about the
   socket to the standard error. The directory has mode 0700, and
   `server.sock` and `server.sqlite` are in it.

2. On the server computer, the process must write the socket line only. Start a
   second server in a second terminal with the same `HORIZON_SERVER_HOME`.

   Result: the second process writes `error: a Horizon server already runs for
   ...` and exits with code 1. The first process still answers.

3. On client computer A, start the fixture with the server entry:

   ```bash
   python3 scripts/device-smoke/serve.py --native-view --state /path/to/a \
     --horizon /path/to/horizon-frozen \
     --ssh-config /path/to/a/ssh-config --ssh-identity /path/to/a/id_ed25519 \
     --server-target server-computer
   ```

   Result: the candidate opens in the isolated desktop. The Machines section of
   the sidebar shows the line `Server: connecting` and then `Server: in step`,
   also before the first machine arrives.

4. Do step 3 again for client computer B, with `--state /path/to/b` and the same
   `--server-target`.

   Result: client computer B also reaches the server, and the server writes one
   link line for each computer.

## 6. Tasks

### 6.1 S1 — the same list on two computers

1. On client computer A, ask an agent for a machine with `machine_set`, or add
   one with this request:

   ```json
   {"operation": "machine_set", "name": "build-1", "ssh": {"host": "203.0.113.10"}}
   ```

   Result: the Machines section of client computer A shows `build-1` at once.
   The row carries no `pending` mark after a few seconds.

2. Look at client computer B.

   Result: within a few seconds the Machines section of client computer B shows
   `build-1`. The row has no key on client computer B.

3. On client computer B, click the row of `build-1`.

   Result: no terminal opens. The section shows `This computer has no key for
   build-1 yet`.

4. On client computer B, forget the machine with the row menu, or with this
   request:

   ```json
   {"operation": "machine_delete", "name": "build-1"}
   ```

   Result: the row goes from client computer B at once and from client computer
   A within a few seconds. The line of the section says `Server: in step`.

### 6.2 S2 — the server away with a change that waits

1. On the server computer, stop the server with Ctrl-C in its terminal.

   Result: the process ends with code 0 and removes its socket. Both client
   computers show `Server away · 0 pending` within a few seconds. Each keeps its
   last list.

2. On client computer A, add a machine `build-2` with the request of step 6.1.

   Result: the row appears at once and carries the `pending` mark. The line says
   `Server away · 1 pending`.

3. Look at client computer B.

   Result: client computer B still shows its last list. It does not show
   `build-2`.

4. On the server computer, start the server again with the command of step 5.1.

   Result: both client computers say `Server: in step` within a few seconds.
   Client computer B shows `build-2`, and the `pending` mark of client computer A
   goes away.

### 6.3 S3 — a restart of one computer while the server is away

1. On the server computer, stop the server with Ctrl-C.

2. On client computer A, add a machine `build-3`, and then stop the fixture with
   Ctrl-C in its terminal.

   Result: the candidate ends. The state folder of client computer A still holds
   `data/home/.horizon/machines/server-queue.json` and
   `data/home/.horizon/machines/server-mirror.json`.

3. Start client computer A again with the same `--state` and
   `--reuse-state --resume-session`.

   Result: the Machines section shows `build-3` with the `pending` mark, and the
   line says `Server away · 1 pending`.

4. On the server computer, start the server again.

   Result: client computer A loses the `pending` mark, and client computer B
   shows `build-3`.

### 6.4 S4 — the first link of a computer that already has machines

1. Stop client computer B and stop the server.

2. On client computer B, keep the machine list of the private home, and start
   the fixture again without `--server-target`.

   Result: the candidate shows the machines that its own home holds. It starts
   no `ssh`, and the section shows no `Server` line.

3. Add a machine `build-4` with the request of step 6.1, then stop the fixture.

4. On the server computer, start the server again. Start client computer B with
   the `--server-target` of step 5.

   Result: after a few seconds client computer B shows `build-4` and every
   machine of the server. A machine that the server and the home both held keeps
   the record of the server.

### 6.5 S5 — no server in the settings

1. On client computer A, remove `--server-target` from the command, and keep the
   same `--state` and `--reuse-state --resume-session`.

   Result: the candidate shows the last list of that home, the section shows no
   `Server` line, and no `ssh` process of the candidate runs. The machine list of
   that home is the file `machines/registry.json`.

## 7. Pass criteria

- A machine that one computer adds appears on the other computer within a few
  seconds, and a removal travels back.
- With the server away, both computers keep their last list, a change shows
  `pending`, and the change settles when the server returns.
- The queue of a computer survives its restart while the server is away.
- A machine that only the server gave has no key on the other computer, and a
  click on it opens nothing and says why.
- A second server of one home refuses with one typed line and code 1.
- A computer without a server in its settings behaves as before, and it starts
  no child.

## 8. Cleanup

1. Stop each fixture with Ctrl-C in its terminal.

   Result: each candidate closes its window, and the fixture stops its display,
   its VNC server and its bus.

2. On the server computer, stop the server if it still runs, and remove its
   test home:

   ```bash
   rm -rf "$HOME/.horizon-server-test"
   ```

   Result: the directory and its store are gone.

3. Remove the state folders of the client computers.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the host names, the login names, the addresses and the
private keys of the test out of the repository.
