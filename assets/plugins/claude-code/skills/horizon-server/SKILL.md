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
   runs here:

   ```sh
   uname -s; uname -m; command -v horizon
   ```

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
   absolute path that `command -v horizon` answered.

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

The other computer joins with one of two transports:

- With an SSH login on the server computer: start Horizon there, open Set up, and
  choose `Join a server that you already have`.
- Without one: **Settings**, **Server**, **Add a computer** shows a QR code and a
  code for one computer, for ten minutes. A card cannot draw a code; name that
  place on the card.

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
readable, and a socket and a lock belong to one run of one server. The procedure
`docs/testing/procedures/server-move.md` of the Horizon repository gives the
order, the checks and the undo. Tell the person that this procedure exists, and
follow the recipe of this skill for each step that it names.

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
