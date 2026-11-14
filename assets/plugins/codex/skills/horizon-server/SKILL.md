---
name: horizon-server
description: >-
  Put the Horizon server on a computer and prove that it answers: the binary of this computer, the
  server home, the service that starts it at login, the check with horizon --server-link, the link of
  another computer, the update, the removal and the move. Use it when the person wants one machine
  list on all their computers, or when the horizon-setup skill reaches a step that installs the
  server.
---

# Horizon server

The Horizon server keeps what more than one computer of one person agrees on: the
machine list, the key of each machine, and every machine key and provider
credential value that Horizon holds. Each computer names the same server, so a
machine that one computer adds appears on the others within seconds.

The server is the same `horizon` binary that the person already runs, with
`--server`. It needs no second program and no account. It opens no port on the
network: it answers the links of the computers over their SSH login, through a
socket in its own home.

This skill gives the work. The cards of the flow are in `horizon-setup`, the
card contract is in `horizon-block`, and the provider commands are in
`horizon-machines`.

## The model

- You run every command in your own shell, over SSH with the machine key of
  Horizon, in the one form that `horizon-machines` gives. Never ask the person to
  type a command that you can run.
- `This computer` as the place of the server is the same recipe, run in your own
  shell without SSH.
- Never download a `horizon` binary, from GitHub or from anywhere else. The
  binary comes from this computer, or the person builds it. A binary of another
  system or another CPU does not run.
- A secret is never drawn on a card and never written into the panel. The
  server home holds the key that seals each secret; it never leaves the server
  computer.

## Where the binary comes from

1. Read the system and the CPU of the place:

   ```sh
   uname -s; uname -m
   ```

2. Read the same two values of this computer, and the path of the binary that
   runs here. Horizon puts that path in the environment of every agent panel as
   `HORIZON_EXECUTABLE`, so read the variable and never look on `PATH`:

   ```sh
   uname -s; uname -m; printf '%s\n' "$HORIZON_EXECUTABLE"
   ```

   The variable names the program that runs this Horizon, which is the binary
   that this flow copies. On Linux it is `/proc/<pid>/exe`, which follows the
   live program after the file at the original path was replaced or deleted; on
   macOS it is the path that Horizon started from, and a file that the person
   deleted is not there to copy. An empty value means that an agent of another
   program started this panel, or that Horizon could not resolve its own path:
   stop and tell the person in one sentence to start the agent from a panel of
   Horizon.

3. When both pairs match, copy the binary of this computer to the place:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> \
       'mkdir -p "$HOME/bin" && chmod 700 "$HOME/bin"'
   scp -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes <path of horizon> <user>@<address>:bin/horizon
   ```

   `<private_key_path>` and `<known_hosts_path>` are the fields of the
   `machine_key` answer of `horizon-machines`, and `<path of horizon>` is the
   value of `HORIZON_EXECUTABLE`.

4. When the two pairs differ, stop and tell the person in one sentence what to
   do: build Horizon on a computer of that system and CPU, or choose a place of
   the same system and CPU as this computer. Draw that sentence on the card of
   the flow, with `Try again` and `Stop`.

## The steps

### 1. Probe the place

Run these lines with the machine key, in the SSH form above.

```sh
uname -s; uname -m; id -un; echo "$HOME"
df -h "$HOME" | tail -n 1
command -v systemctl || true
```

- `Linux` with `systemctl` uses the systemd user unit.
- `Darwin` uses the launchd user agent.
- Another answer, or no `systemctl` on Linux, means that this skill does not
  cover the place. Say so on the card and stop.

### 2. Make the server home

The server reads and writes `HORIZON_SERVER_HOME`, or `~/.horizon-server`:

```sh
mkdir -p "$HOME/.horizon-server/log"
chmod 700 "$HOME/.horizon-server"
```

| File | What it is |
|---|---|
| `server.sqlite` | The store: the machine list, the keys and the values, sealed |
| `secret.key` | The key that seals each value of the store, mode 0600 |
| `server.sock` | The socket that the link of a computer reaches, mode 0600 |
| `server.lock` | The lock that says that a server runs |
| `log/server.log` | The log of the macOS agent; a Linux unit writes the journal |

> **CAUTION: BACK UP `secret.key` SEPARATELY FROM `server.sqlite`.** A copy of the
> database without that key holds no readable secret. Keep the key in another
> backup set, at mode 0600. Tell the person this before the server holds a value.

Run the server by hand one time, so its first start makes the home and the key:

```sh
HORIZON_SERVER_HOME="$HOME/.horizon-server" "$HOME/bin/horizon" --server
```

Result: the process writes one line about the socket and stays in the
foreground. Stop it with `Ctrl-C`.

### 3. Start the server at login

#### macOS

1. Write `~/Library/LaunchAgents/net.horizon.server.plist`. Replace `CHANGE-ME`
   with the user of the place:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>net.horizon.server</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/CHANGE-ME/bin/horizon</string>
    <string>--server</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>/Users/CHANGE-ME/.horizon-server/log/server.log</string>
  <key>StandardErrorPath</key><string>/Users/CHANGE-ME/.horizon-server/log/server.log</string>
  <key>ExitTimeOut</key><integer>10</integer>
</dict>
</plist>
```

   `RunAtLoad` starts the server at each login, `KeepAlive` starts it again when
   it ends, and `ExitTimeOut` gives it ten seconds to close its store.

2. Load the agent and read its state:

   ```sh
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/net.horizon.server.plist
   launchctl print "gui/$(id -u)/net.horizon.server" | head -20
   ```

   Result: the state is `running`.

#### Linux

1. Write `~/.config/systemd/user/horizon-server.service`:

```ini
[Unit]
Description=Horizon server

[Service]
Type=simple
ExecStart=/home/CHANGE-ME/bin/horizon --server
Restart=on-failure

[Install]
WantedBy=default.target
```

   Add the line `Environment=HORIZON_SERVER_HOME=/home/CHANGE-ME/.horizon-server`
   only when the home is not `~/.horizon-server`.

2. Start the unit and read its state:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now horizon-server
   systemctl --user status horizon-server
   ```

   Result: the state is `active (running)`.

3. On a computer where nobody logs in, a linger starts the unit at boot:

> **CAUTION: ENABLE A LINGER ONLY FOR THE USER OF THE SERVER.** The linger keeps
> every process of that user alive after the logout, also a process that the
> person forgot, and the server holds every secret of the person. Ask the person
> before this step.

   A move of a server reads the choice of the old server with
   `loginctl show-user "$(id -un)" -p Linger`, as `horizon-server-move` says, and
   applies the same choice here without a question: the person chose it once.
   Ask only when the state of the old server could not be read.

```sh
sudo loginctl enable-linger CHANGE-ME
```

   Result: the unit starts when the computer starts, without a login.

### 4. Prove that the server answers

Run the link of another computer against the socket of the place:

```sh
HORIZON_SERVER_HOME="$HOME/.horizon-server" "$HOME/bin/horizon" --server-link < /dev/null
echo "exit $?"
```

- Exit code `0` means that the socket accepted the link: the server answers.
- Exit code `3` means that no server answers at that socket.
- Any other code means that the server is there and refused the link, and the
  standard error says why.

Solve a `3` before you link this computer: read the log of the service, or the
journal, and start the service again.

### 5. Link this computer

Call `server_status` of `horizon-block` first. It answers the entry that this
computer holds now, the state of its link and the store id that this computer saw
last, and it names no token and no secret. When its `link` is already `in_step`
with the place of your card, this computer needs no new entry.

Call `server_link_request` with `machine` and the name of the place, and the
command:

- `machine` names the machine of the list that runs the server, as `machines`
  reports it. The link uses the address, the user, the port, the key file and the
  known-hosts file that a terminal of that machine uses. Name `ssh_target`
  instead only when the server runs on a computer of the person: that target is
  the name that `ssh` of this computer resolves, and `This computer` uses its own
  name or `127.0.0.1` only when that name is in the SSH configuration.
- The command is the whole path of the binary on the place and `--server-link`,
  for example `/home/person/bin/horizon --server-link`. A non-interactive SSH
  session has a short `PATH`, so give the whole path.

Horizon draws its own card and the person presses `Link`. The answer is
`confirmed`, `declined` or `timed_out`, and `start` says `linked` when the
running Horizon follows the entry now, or `restart_needed` when the person starts
Horizon again one time. A `confirmed` answer also carries `link`: `in_step` when
the server answered within about 20 seconds, or `away` when it did not, with
`reason` holding one sentence that names the cause. Say `in_step` as the ready
card of the flow, and draw a failure card for `away` with that sentence.

An agent never restarts the program of the person.

### 6. Another computer

The other computer of the person joins with one of three ways. The place of this
server decides which one works, and only the last one needs no step of the person
beyond a press.

**A. The server runs on a computer of the person, and the other computer has an
SSH login to it.** The other computer writes the SSH name of that computer in
**Settings**, **Server**, **SSH target**, with the command of the server, and
starts Horizon again one time: the tab writes the store and the entry applies at
the next start. An agent of that computer shortens this with
`server_link_request { ssh_target }`, whose card applies the entry at once. Its
link then travels over SSH with the login of the person, and the server needs no
code, no listener and no key of Horizon. The skill `horizon-setup` gives the same
way as the goal `join`.

**B. The server runs on a rented machine, and there is no overlay network.** The
machine holds the machine key of this computer and no login of the person, so the
other computer cannot reach it. This is the one way that works today:

1. On the other computer, call `machine_key` with a name, and keep the
   `public_key` line of the answer. The private half stays there.
2. The person writes that line in the panel of the agent on this computer.
3. On this computer, add the line to the machine through the machine key. The
   form is the one of `horizon-machines`, and the user is the user of the record:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> \
       'mkdir -p "$HOME/.ssh" && chmod 700 "$HOME/.ssh" && touch "$HOME/.ssh/authorized_keys" && chmod 600 "$HOME/.ssh/authorized_keys" && grep -qxF "<public line of the other computer>" "$HOME/.ssh/authorized_keys" || printf "%s\n" "<public line of the other computer>" >> "$HOME/.ssh/authorized_keys"'
   ```

4. On the other computer, write one block of its SSH configuration, with the two
   paths of its own `machine_key` answer:

   ```text
   Host horizon-server
       HostName <address of the machine>
       User <user of the record>
       IdentityFile <private_key_path on the other computer>
       IdentitiesOnly yes
       IdentityAgent none
       UserKnownHostsFile <known_hosts_path on the other computer>
       StrictHostKeyChecking accept-new
   ```

5. The other computer writes `horizon-server` in **Settings**, **Server**, **SSH
   target**, with the command of the server, and its link travels over SSH to the
   machine with its own key.

   This way is not one press: steps 1 and 2 belong to the person. Tell the person
   that, and do not invent a code that does not exist.

**C. An overlay network holds the machine and both computers.** Start the server
on its overlay address with `--listen <overlay address>:7000 --name <overlay
name>`, and open the listener port in the firewall of the overlay, which carries
its own encryption. Then **Settings**, **Server**, **Add a computer** shows a link
and a code for one computer, for ten minutes, and the other computer pastes the
link in **Join with a link**. Its token is stored and its link travels over the
overlay. A card cannot draw a code: name that place on the card.

The tab lists the computers of the server. Removing one there ends its links, and
that computer keeps the copies of the keys and the values that it already had.

## Update the binary

1. Copy the new binary of this computer over the old path, as step 2 of the
   model does.
2. Restart the service, so the new program runs:

   ```sh
   launchctl kickstart -k "gui/$(id -u)/net.horizon.server"   # macOS
   systemctl --user restart horizon-server                     # Linux
   ```

3. Prove it, with step 4. The store, the keys and the values stay: they are files
   of the server home, and a restart reads them again.

## Remove the server

1. Stop the service and take it out of the login items:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server"   # macOS
   systemctl --user disable --now horizon-server         # Linux
   ```

2. Remove the agent file, the unit and the linger, on Linux with
   `sudo loginctl disable-linger CHANGE-ME`.

3. Tell the person what the home still holds before anything else: every machine
   key and every credential value of every computer. Remove `~/.horizon-server`
   only when the person says that the server goes for good.

4. Each computer of that server keeps its copy of the machine list and its keys.
   A computer that must stop following the server takes the entry out of
   **Settings**, **Server**.

## Move a server

A move is a new server on another place, then a link of each computer to it. Do
not move the home with a copy alone: `secret.key` is what makes the values
readable, and a socket and a lock belong to one run of one server. The skill
`horizon-server-move` gives the whole move: the stop, the copy of the store, the
check of the store id and the cards of the steps. Use it for a move, and follow
the recipe of this skill for each step that it names.

## What costs money

- A server on `This computer` costs nothing.
- A server on a rented machine costs what the machine costs, hour by hour, until
  the machine is deleted. A stopped machine still costs its disk and its address.
- A server that runs on its own computer needs no firewall rule and no port.

## Limits of this version

- The service of this skill is a user service of systemd or launchd. A system
  service, a container and `systemd --user` without a session bus are not
  covered.
- A place of another system or another CPU than this computer needs a binary that
  the person builds there. This skill downloads nothing.
- A card cannot draw the QR code of an invitation. That stays in Settings,
  Server.
