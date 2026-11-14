---
name: horizon-server-move
description: >-
  Move the Horizon server from one computer to another computer with its store, its machines, its
  keys, its values and the computers that follow it: the plan card, the stop of the old server, the
  copy of the store, the start on the new computer, the check of the store id and the counts, the
  entry on this computer through server_link_request, the note for each other computer, and the undo
  of each step. Use it when the person says "move the server to <machine>", or when the computer that
  runs the server is replaced.
---

# Move the Horizon server

The person says "move the server to `<machine>`". You move the server home from
the computer that runs it now to the new computer, prove that the new server
holds the same store, and give the person one card for the whole move.

The recipes of the server are in `horizon-server`: the binary, the server home,
the service at login, the proof that the server answers, the removal and the
update. Use that skill for each of those steps, and do not repeat its commands
from memory. This skill adds the order of a move, its cards, its check and its
undo. The card contract is in `horizon-block`.

## The rules

These rules have no exception:

- **The store is moved, not copied while a server runs.** Stop the old server,
  read its log until it says that it stopped, and copy the files of a stopped
  server. Two servers with one store id and two contents decide the machine list
  twice.
- **The old store is kept aside and never deleted by you.** Copy the two files of
  the stopped old server into a dated folder of the old computer at mode 700
  before you touch anything else, and tell the person where it is. You delete
  neither the old home nor that copy. Only the person deletes them.
- **The person confirms the entry.** Only `server_link_request`, and only its own
  card, writes the server entry of a computer. You send the request and read the
  answer; you press nothing and you restart nothing.
- **`secret.key` never leaves the two computers of the move as text.** Copy it as
  a file at mode 600 inside a folder at mode 700, with `scp -p`. Never draw it on
  a card, never write it into a panel, and never read it to type it anywhere.
- **A move keeps the store id.** The same store id after the move is the proof of
  the move. Another store id means that the new server made a new store, and the
  machine list, the keys and the values did not arrive.
- **One step at a time, on one card.** Say where the server is now, where it goes
  and what you copy, run the step, and draw the result.

## Read the server of this computer

Call `server_status` of `horizon-block` before you ask the person anything. It
answers the entry that the settings hold now, the state of the link and the store
id that this computer saw last:

- `ssh_target` or `machine` names the place of the server of today, `command` is
  the command of its link, and `address` is its listener when the link is TCP.
- `link` is `off`, `connecting`, `in_step`, `behind`, `away`, `removed` or
  `not_logged_in`.
- `store` is the store id that this computer saw last, and it is absent when the
  link read none. Record it before the move: step 6 compares it with the store id
  of the new server.

An agent writes no settings file. Read `server_status` and ask the person only
about a value that the answer does not hold, such as the name that `ssh` of this
computer uses for a new computer that is no machine of the list.

Take the new computer from the request of the person. When it is a machine of
`machines`, its record holds the address, the user and the key, and you reach it
with the SSH form of `horizon-block`. When it is a computer of the person, the
person names the SSH target.

## The steps

The card of the move holds this list of steps, so the person reads the order:

1. Read both computers: the system, the CPU, the free space, the service system
   and the start at boot choice of the old server, as `horizon-server` says.
2. Prove that the binary of the new computer is the binary of this computer: the
   same system and the same CPU, copied from `HORIZON_EXECUTABLE`. Stop when they
   differ, and say on the card what the person must build.
3. Keep the old store aside: stop the old server, read its log until it says
   `the Horizon server stopped`, and copy `server.sqlite` and `secret.key` into a
   dated folder of the old computer.
4. Copy the stopped store to the new server home, with `scp -p`, at mode 600 in a
   folder at mode 700.
5. Start the new server with its service, at boot as the old server started, and
   read its log until it says `the Horizon server answers`.
6. Read the store id and the counts of the old store and of the new store, and
   compare them.
7. Ask for the entry of this computer with `server_link_request`, and read the
   answer of the person.
8. Say on the card what every other computer of the person must do.

### 1. Read both computers

Run the probe of `horizon-server` on both computers. Record the output in the
plan of the move, not on the card:

- `uname -s; uname -m` and `id -un`;
- `echo "$HOME"` and `df -h "$HOME" | tail -n 1`;
- `command -v systemctl || true`;
- the two files of the server home: `ls -l "$HOME/.horizon-server"`;
- the start at boot choice of the old server, from the lingering state of its
  user: `loginctl show-user "$(id -un)" -p Linger` on Linux, and
  `grep -c 'RunAtLoad' ~/Library/LaunchAgents/net.horizon.server.plist` on macOS.

Result: both computers answer, each has room for the store and the key, and the
old computer says `Linger=yes` or `Linger=no`.

The start at boot choice belongs to the server and not to the computer that runs
it, so the move keeps it: a new server takes the choice of the old one, and the
person is not asked again. Ask only when the old state cannot be read, for
example when the old computer is already gone, and say on the card which state you
read. Write the choice in the row `Starts at boot` of the card of the move.

### 2. Prove the binary

The move keeps the store, and the store needs a `horizon` of the same system and
the same CPU as the one that wrote it. Compare the two pairs of `uname -s` and
`uname -m` as `horizon-server` says, and copy the binary of this computer to the
new computer. When the pairs differ, stop: draw the failure card with the
sentence that the person must build Horizon on that system, and copy nothing.

### 3. Keep the old store aside

> **CAUTION: STOP THE OLD SERVER BEFORE THE FIRST COPY.** A copy of a store that
> a server writes is not one state. Stop the service, read the log, and copy the
> files of the stopped server only.

1. Stop the service of the old computer with the stop step of its system, from
   `horizon-server`:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server"     # macOS
   systemctl --user disable --now horizon-server           # Linux
   ```

2. Read the log until it holds this line:

   ```sh
   tail -n 20 "$HOME/.horizon-server/log/server.log"
   ```

   Result: the log holds `the Horizon server stopped`, `server.sock` is gone, and
   `server.sqlite-wal` is empty or absent. Stop when the line is not there.

3. Make the dated folder and copy the two files into it:

   ```sh
   aside="$HOME/horizon-server-before-move-$(date -u +%Y%m%dT%H%M%SZ)"
   mkdir -p "$aside" && chmod 700 "$aside"
   cp -p "$HOME/.horizon-server/server.sqlite" "$HOME/.horizon-server/secret.key" "$aside/"
   ls -l "$aside"
   ```

   Result: the folder is at mode 700 and holds `server.sqlite` and `secret.key` at
   mode 600. Record its path on the card of the done step, and delete nothing
   from it.

4. Read the store id and the counts of this store, as step 6 says. Record them.

### 4. Copy the store to the new computer

1. Make the server home of the new computer, and copy the two files with their
   modes:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> \
       'mkdir -p "$HOME/.horizon-server" && chmod 700 "$HOME/.horizon-server"'
   scp -p -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes \
       "$HOME/.horizon-server/server.sqlite" "$HOME/.horizon-server/secret.key" \
       <user>@<address>:.horizon-server/
   ```

   Use `scp` in your own shell without SSH when the new computer is this
   computer.

2. Copy `server.sqlite-wal` and `server.sqlite-shm` too when the folder of the
   stopped server still holds them and they are not empty. They belong with their
   `server.sqlite`.

3. Do not copy `server.lock`, `server.sock` or the `log` folder. The lock is the
   open file of a process, the socket belongs to one run of a server, and the log
   is the history of the old computer.

4. Correct the modes, and read them:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> \
       'chmod 600 "$HOME/.horizon-server/server.sqlite" "$HOME/.horizon-server/secret.key" && ls -l "$HOME/.horizon-server"'
   ```

   Result: `server.sqlite` and `secret.key` are there at mode 600, and no
   `server.lock` and no `server.sock` are there.

### 5. Start the new server

Install the service of the new computer with the service steps of its system from
`horizon-server`, with the same flags as the old server: `--listen` and `--name`
when the computers of the listener reach it, and `HORIZON_SERVER_HOME` only when
the home is not `~/.horizon-server`.

Start it at boot exactly as the old server started: run the linger step of
`horizon-server` when step 1 read `Linger=yes`, and leave the linger away when it
read `Linger=no`. That step needs the press of the person only when the old state
could not be read, because the person already chose it for the old server.

Start the service and read its log until it holds this line:

```text
the Horizon server answers
```

Result: the line names the socket of the new computer and its listener. Stop when
the log says that the store cannot be opened, or when the line is not there.

Prove that the server answers with the link of `horizon-server`:

```sh
HORIZON_SERVER_HOME="$HOME/.horizon-server" "$HOME/bin/horizon" --server-link < /dev/null
echo "exit $?"
```

Result: exit code `0`. A `3` means that no server answers at that socket.

### 6. Check the same store

Read the store id and the counts on both computers, with the same four lines and
`sqlite3`:

```sh
sqlite3 "$HOME/.horizon-server/server.sqlite" "select value from meta where key = 'store_id';"
sqlite3 "$HOME/.horizon-server/server.sqlite" "select kind, count(*) from objects where deleted = 0 group by kind order by kind;"
sqlite3 "$HOME/.horizon-server/server.sqlite" "select count(*) from secrets where deleted = 0;"
sqlite3 "$HOME/.horizon-server/server.sqlite" "select count(*) from devices;"
```

The four answers, in this order, are:

1. The store id: 36 characters. It must be the same on both computers. A
   different value means that the new server made a new store.
2. The live objects of each kind: the machines and the shared settings sections.
   Each count must be the same on both computers.
3. The sealed values of the named secrets. The count must be the same on both
   computers.
4. The computers that follow the server. The count must be the same on both
   computers.

A store id that is the same and counts that are the same prove that the move
kept the store. Draw both sets of values as two `kv` columns, and draw the count
that differs as a row with the state `failed`.

### 7. Ask for the entry of this computer

Send the request of `horizon-block` for the settings entry. A server on a machine
of the list takes `machine`; a server on a computer of the person takes
`ssh_target`:

```json
{"operation": "server_link_request", "machine": "build-1",
 "command": "/home/person/bin/horizon --server-link"}
```

Horizon draws its own card in your workspace, and the person presses `Link` or
declines it. You press nothing.

Read the answer:

- `confirmed` with `start: linked` means that the running Horizon follows the new
  entry now.
- `confirmed` with `start: restart_needed` means that the person starts Horizon
  again one time. An agent never restarts the program of the person.
- `confirmed` with `link: in_step` means that the new server answered within
  about 20 seconds.
- `confirmed` with `link: away` means that the entry is written and the new server
  did not answer. The answer carries `reason` with one sentence. Draw the failure
  card with that sentence, and keep the old store aside.
- `declined` and `timed_out` mean that this computer keeps its old entry. Say
  which one on the card, and say that the server is on the new computer with the
  store of the move.

### 8. Say what every other computer must do

The server entry is a local section of the settings of one computer: no computer
learns the new place by itself, and each one keeps its old target until a person
sets it. Say this on the card of the done step, as one line per other computer:

- **A computer that reached the old server over SSH**: it needs its own entry. Its
  agent sends `server_link_request` with the new place on that computer, and the
  person presses `Link` there. A person who has no agent there writes the fields
  of **Settings**, **Server**, **This computer** and starts Horizon again.
- **A computer that joined with a link and a code**: its token still works,
  because the token is a row of the moved store and `secret.key` reads it. Its
  **Address** must reach the listener of the new computer, so the service of the
  new computer needs the `--listen` and the `--name` that the links held. Its
  device name is its own file `machines/server-device` of its Horizon home, so a
  token under another name is not logged in.
- **A computer that never reached this server**: it joins as `horizon-server`
  says, and it reads every machine and every named secret of the moved store.

You do not run a step on another computer of the person unless the person asks you
to, and each of those computers needs its own press. Say that plainly.

## The cards

One card, titled with the new computer, changed with `update` at each step. The
card of the move:

```json
{"components": [
  {"type": "heading", "text": "Move the server to build-1",
   "subtitle": "From this computer. The store, the machines, the keys and your settings move with it.",
   "badge": {"text": "Step 1 of 8"}},
  {"type": "progress", "label": "1 of 8 steps", "percent": 13, "detail": "reading both computers"},
  {"type": "steps", "steps": [
    {"label": "Read both computers", "state": "now", "at": "now"},
    {"label": "Prove the binary of the new computer"},
    {"label": "Keep the old store aside"},
    {"label": "Copy the store to the new computer"},
    {"label": "Start the new server and read its answer"},
    {"label": "Check the store id and the counts"},
    {"label": "Link this computer, with your press"},
    {"label": "Say what every other computer does"}]},
  {"type": "kv", "rows": [
    {"label": "Now", "value": "this computer"},
    {"label": "Goes to", "value": "build-1, a machine of your list"},
    {"label": "Starts at boot", "value": "yes, as the old server does"},
    {"label": "Is copied", "value": "server.sqlite and secret.key, at mode 600"},
    {"label": "Is not copied", "value": "the lock, the socket and the log"},
    {"label": "Kept aside", "value": "the old store in a dated folder, never deleted"}]},
  {"type": "buttons", "items": [
    {"id": "start", "label": "Start the move", "tone": "primary"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

The card of the link, when the new server answers and the entry waits for the
person:

```json
{"components": [
  {"type": "heading", "text": "The new server answers",
   "subtitle": "Press Link on the card of Horizon to point this computer at build-1.",
   "badge": {"text": "Needs you", "tone": "warn"}},
  {"type": "kv", "rows": [
    {"label": "Store id", "value": "9f1c0a52-6d28-4d0f-9a3e-1c0a52f0c9a1", "mono": true},
    {"label": "Machines", "value": "4 on both computers"},
    {"label": "Named secrets", "value": "2 on both computers"},
    {"label": "Computers", "value": "3 on both computers"},
    {"label": "Old store kept", "value": "horizon-server-before-move-20261114T071500Z", "mono": true}]},
  {"type": "text", "text": "Horizon asks you to confirm the entry. An agent cannot press it and cannot restart Horizon for you."},
  {"type": "buttons", "items": [
    {"id": "read_answer", "label": "I pressed Link", "tone": "primary"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

The card of the done step:

```json
{"components": [
  {"type": "heading", "text": "The server moved to build-1",
   "subtitle": "This computer follows it. The store id did not change, so the store moved.",
   "badge": {"text": "Done", "tone": "ok"}},
  {"type": "kv", "rows": [
    {"label": "Store id", "value": "9f1c0a52-6d28-4d0f-9a3e-1c0a52f0c9a1", "mono": true},
    {"label": "New home", "value": "/home/person/.horizon-server", "mono": true},
    {"label": "Old store kept", "value": "horizon-server-before-move-20261114T071500Z", "mono": true}]},
  {"type": "caption", "text": "Your other computers"},
  {"type": "list", "selection": "none", "rows": [
    {"id": "other.laptop", "title": "laptop, which reached the old server over SSH",
     "subtitle": "Its agent sends server_link_request with build-1, and a person presses Link there.", "value": "its own press"},
    {"id": "other.studio", "title": "studio, which joined with a link and a code",
     "subtitle": "Its token still works. Its Address must reach the listener of build-1.", "value": "its own press"}]},
  {"type": "text", "text": "A server entry belongs to one computer, so no computer learns the new place by itself. Do not start the old server again."},
  {"type": "buttons", "items": [
    {"id": "close", "label": "Close", "tone": "primary"},
    {"id": "remove_old", "label": "Remove the old server", "tone": "danger"}]}
]}
```

The card of a step that did not finish:

```json
{"components": [
  {"type": "heading", "text": "The step did not finish",
   "subtitle": "The old server is stopped and its store is kept aside.",
   "badge": {"text": "Not moved", "tone": "danger"}},
  {"type": "text", "text": "<the one sentence of the reason>"},
  {"type": "text", "text": "Undone: nothing. Left: the old server is stopped, the old store is in the dated folder, and this computer still points at the old place."},
  {"type": "buttons", "items": [
    {"id": "try_again", "label": "Try again", "tone": "primary"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

## The undo of each step

| Step | The undo |
|---|---|
| 1. Read both computers | Nothing to undo. |
| 2. Prove the binary and copy it | Remove the copied binary of the new computer. |
| 3. Stop the old server and keep the store aside | Start the old service again with the start step of its system, and prove it with `--server-link`. The store did not change. |
| 4. Copy the store | Remove the copy of the new computer, at mode 600 in its folder. The old store and the aside copy stay. |
| 5. Start the new server | Stop the service of the new computer. |
| 6. Check the same store | Nothing to undo. A store id that differs stops the move, and nothing else changes. |
| 7. Ask for the entry | `server_link_request` writes one setting after a press. To go back, send a new request with the old place, and the person presses `Link` again. An agent writes no settings file. |
| 8. The other computers | Each of them repeats step 7 with the place that it must follow. |

A move that fails before step 7 leaves the old server stopped and this computer
pointing at the old place. Start the old service again, or go on with the new
computer and step 7. Say which one the person chose on the card.

## Limits of this version

- An agent cannot read the server entry of a computer. Ask the person, and use
  `machines` for the state of the link only.
- An agent cannot press the card of `server_link_request`, cannot write a settings
  file and cannot restart Horizon. A `restart_needed` answer waits for the person.
- An agent cannot see the other computers of the person. Each of them needs its own
  request and its own press, and says so on the card of the done step.
- The store id needs `sqlite3` on both computers. Without it, the counts of the
  objects, the secrets and the computers are the proof, and the card says that the
  store id was not read.
- A store that a server wrote is one state only when the server stopped. A copy
  of a running server is refused by this skill, and a lost `-wal` file loses the
  last changes.
- A server on another system or another CPU needs a binary that the person builds
  there. This skill downloads nothing and copies nothing across systems.
