---
procedure: horizon-server
feature: One machine list on two computers, kept by a Horizon server
platforms: [linux, macos]
cost: none
destructive: no
secrets: the SSH login of the server computer, the machine keys of each Horizon home, and one test credential value (use a synthetic value such as `test-value-1`)
owner: peters
---

# Horizon server test procedure

## 1. Purpose

This procedure proves that two computers show one machine list and hold one set of
keys and credential values. It proves that a machine that one computer adds
appears on the other computer, that a removal travels back, that a change waits
while the server is away, and that the change settles when the server returns. It
proves that a machine key that one computer makes logs in from the other computer,
that a credential value that one computer sets is used by the other computer, and
that a server whose key file is lost keeps running and gets the values back. It
also proves that a computer without a server in its settings behaves as before.

## 2. Applicability

- Candidate: a build of `horizon` that has the `--server` mode, the
  `--server-link` mode and the `server` settings entry.
- Platforms: the server computer is Linux or macOS. A client computer is Linux
  or macOS.
- This procedure does not test a rented machine, a card, a click that waits for an
  agent, or two servers.
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
   socket to the standard error.

2. In a second terminal on the server computer, look at the server home:

   ```bash
   export HORIZON_SERVER_HOME="$HOME/.horizon-server-test"
   ls -l "$HORIZON_SERVER_HOME"
   ```

   Result: the directory has mode 0700. `server.sock`, `server.lock`,
   `server.sqlite` and `secret.key` have mode 0600. The `server.sqlite-wal` and
   `server.sqlite-shm` files, when the server has written a change, have mode
   0600 too. `secret.key` holds 32 bytes.

3. On the server computer, the process must write the socket line only. Start a
   second server in a second terminal with the same `HORIZON_SERVER_HOME`.

   Result: the second process writes `error: a Horizon server already runs for
   ...` and exits with code 1. The first process still answers.

4. On client computer A, start the fixture with the server entry:

   ```bash
   python3 scripts/device-smoke/serve.py --native-view --state /path/to/a \
     --horizon /path/to/horizon-frozen \
     --ssh-config /path/to/a/ssh-config --ssh-identity /path/to/a/id_ed25519 \
     --server-target server-computer
   ```

   Result: the candidate opens in the isolated desktop. The Machines section of
   the sidebar shows the line `Server: connecting` and then `Server: in step`,
   also before the first machine arrives.

5. Do step 4 again for client computer B, with `--state /path/to/b` and the same
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

   Result: the section shows `The key of build-1 was deleted. Ask an agent to make a
   new key and install it.` while the server has no key for that machine, and no
   terminal opens. A key that the server has arrives with the next task.

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

### 6.6 S6 — the server stops while a computer is idle

This task proves that a computer that does nothing learns that the server is
gone, and that it catches up when the server returns.

1. On client computer B, do nothing. Its Machines section says `Server: in step`,
   as at the end of step 6.1, and no click is needed.

2. On the server computer, stop the server with Ctrl-C in its terminal.

   Result: the section of client computer B says `Server away`. Its link ends at
   once when the socket of the server closes, and at the bound of its open wait at
   the latest: the server answers a wait at 50 seconds, and the link of a computer
   ends at 65 seconds. Write down which of the two you saw, and how long it took.

3. On client computer B, look for a child of the link that lives on:

   ```bash
   pgrep -fl -- '--server-link'
   ```

   Result: no process of `--server-link` stays. The attempt of a link whose
   server answers nothing ends at once, and its child ends with it.

4. On client computer A, add a machine `build-2` with the request of step 6.1.

   Result: client computer A says `Server away · 1 pending`, and client computer
   B still shows its last list without `build-2`.

5. On the server computer, start the server again with the command of step 5.1.

   Result: both client computers say `Server: in step` within a few seconds.
   Client computer B shows `build-2`, its list is the list of the server, and the
   `pending` mark of client computer A goes away.

6. Prove that a server which does not answer does not read `Server: connecting`
   for ever. On the server computer, make the port that the link uses drop
   packets instead of refusing them:

   > **CAUTION:** The next command changes the firewall of the server computer.
   > Run it on a test computer only, and keep the second command ready to undo it.

   ```bash
   sudo nft add rule inet filter input tcp dport 22 drop
   ```

7. On client computer B, watch the Machines section for one minute.

   Result: the line reads `Server away`, with the number of intents that wait
   when one waits. It does not read `Server: connecting` after the first attempt
   of the link, and it does not stay on that line while the server is silent.

8. Undo the rule, and start no server: the link of client computer B still reads
   `Server away`. Start the server again with the command of step 5.1, and the
   line reads `Server: in step` within a few seconds.

   ```bash
   sudo nft delete rule inet filter input tcp dport 22 drop
   ```

### 6.7 S7 — a key that one computer makes logs in from the other

1. On client computer B, stay at the machine list of the server with `build-1`.

2. On client computer A, ask for a machine key with this request:

   ```json
   {"operation": "machine_key", "name": "build-1"}
   ```

   Result: the answer holds `name`, `public_key`, `private_key_path`,
   `known_hosts_path` and `created: true`. The text of the private key is in no
   answer.

3. On client computer B, look at the key folder:

   ```bash
   ls -l /path/to/b/data/home/.horizon/machine-keys
   ```

   Result: within a few seconds the folder holds `build-1`, `build-1.pub` and
   `build-1.known_hosts`. The folder has mode 0700 and the three files have mode
   0600.

4. On client computer B, click the row of `build-1`.

   Result: a terminal opens and runs `ssh` with
   `-i <key folder>/build-1` and
   `-o UserKnownHostsFile=<key folder>/build-1.known_hosts`. The section no longer
   says that the key was deleted. The login reaches the machine when its
   `authorized_keys` holds the public line of that key.

4.1. On client computer B, ask an agent for the machine list with this request:

   ```json
   {"operation": "machines"}
   ```

   Result: the record of `build-1` says `key_missing: false`.

5. On client computer B, delete the key with this request:

   ```json
   {"operation": "delete_machine_key", "name": "build-1"}
   ```

   Result: the three files go from client computer A within a few seconds. The
   machine list of both computers says `key_missing: true` for the record of
   `build-1`, and a click on the row of `build-1` on either computer opens no
   terminal and says `The key of build-1 was deleted. Ask an agent to make a new key
   and install it.`

### 6.8 S8 — a credential value that one computer sets is used on the other

> **CAUTION: THIS TASK WRITES A VALUE INTO THE SECRETS FILE OF EACH COMPUTER.**
> Use a synthetic value such as `test-value-1`, and delete it at the end of the
> task.

1. On client computer A, open the settings, the tab Remote. Add a provider with
   one endpoint, one reference and the store **Secrets file**, or use a provider
   that the test configuration already holds.

2. Save the value `test-value-1` for that reference, and press Save in the
   settings.

   Result: the row of the reference shows the state `Present`.

3. On client computer B, open the same tab with the same provider configuration.

   Result: within a few seconds the row of the same reference shows `Present`, and
   Delete removes it on both computers after a few seconds. The providers that use
   that reference read the value without another step.

4. Delete the value on both computers, and remove the provider that this task
   added.

### 6.9 S9 — the key file of the server is lost

> **CAUTION: THIS TASK DELETES `secret.key` OF THE TEST SERVER HOME.** Do it only
> on the test server home of this procedure. The secrets of the test server are
> then lost unless a client computer still holds them, which is what this task
> proves.

1. Stop both client computers and stop the server.

2. On the server computer, delete the key file:

   ```bash
   rm "$HORIZON_SERVER_HOME/secret.key"
   ```

3. Start the server again with the command of step 5.1, and start client computer
   A.

   Result: the server keeps running and writes one warning line about a secret that
   it cannot open. Client computer A sends its own values again, and the new
   `secret.key` is at mode 0600.

4. Start client computer B. Look at its key folder.

   Result: client computer B holds `build-1` with its two files, as in step 6.6,
   and its key folder holds no file whose name starts with `build-1.conflict-`.
   Client computer B reads no notice about the key: the server now holds the same
   private key and the same public line, so the value that client computer B sent
   again is no conflict.

## 7. Pass criteria

- A machine that one computer adds appears on the other computer within a few
  seconds, and a removal travels back.
- With the server away, both computers keep their last list, a change shows
  `pending`, and the change settles when the server returns.
- The queue of a computer survives its restart while the server is away.
- A machine that only the server gave has no key on the other computer while the
  server holds no key, and a click on it opens nothing and says why.
- A machine key that one computer makes is on the other computer with mode 0700
  for the folder and 0600 for the three files, and the `ssh` argv of the other
  computer names that key and its known-hosts file.
- A key that one computer deletes goes from the other computer.
- A credential value that one computer sets shows `Present` on the other computer
  and is read by the providers there.
- A server whose `secret.key` is lost keeps running, reports the loss in its log,
  and holds the values again after a computer that holds them links.
- A second server of one home refuses with one typed line and code 1.
- A server that stops while a computer is idle makes that computer say
  `Server away` within 70 seconds, and the computer is in step with the change of
  the other computer when the server returns.
- A server that does not answer reads `Server away`, and not
  `Server: connecting`, after the first attempt of the link.
- After the key file of the server is lost, the computers send their values again,
  and no computer keeps a `*.conflict-*` copy of a value that the server holds the
  same way.
- `server.sqlite`, `server.sqlite-wal` and `server.sqlite-shm` have mode 0600,
  and `server.lock` stays in the home after a stop of the server.
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

   Result: the directory, its store and `secret.key` are gone.

3. Remove the state folders of the client computers.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the host names, the login names, the addresses and the
private keys of the test out of the repository.
