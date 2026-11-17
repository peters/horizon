# Horizon server

The Horizon server keeps the state that more than one computer of one person
agrees on: the machine list, the key that each machine uses, and every machine key
and provider credential value that Horizon holds. Each computer names the same
server in its settings, so a machine that an agent or the person adds on one
computer shows on the other computers within a few seconds, and it opens with the
same key.

This folder holds one sample: a launchd user agent for macOS,
`net.horizon.server.plist`. Its text is in the section
[Run the server at login](#run-the-server-at-login), which also gives the systemd
user unit of Linux. The server is the same `horizon` binary that the person
already runs, with `--server`. It needs no second program and no account.

## What the server is

| Part | What it is |
|---|---|
| Program | `horizon --server`, the binary of this repository |
| Server home | `HORIZON_SERVER_HOME`, or `~/.horizon-server` |
| Store | `<server home>/server.sqlite`, mode 0600 |
| Key of the secrets | `<server home>/secret.key`, mode 0600, made at the first start |
| Socket | `<server home>/server.sock`, mode 0600 in a directory at mode 0700 |
| Lock | `<server home>/server.lock`, so a second server of the same home refuses |
| Log | The standard error of the process |

The server reads no setting and keeps no file of the person's desktop: its home
is a directory of its own. It answers the links of the computers over the SSH
login of its user. It opens no port.

## The key of the secrets

The server holds every machine key and every provider credential value of the
person, because a second computer must work at once. Each value is sealed with
the key in `<server home>/secret.key`.

> **CAUTION: BACK UP `secret.key` SEPARATELY FROM `server.sqlite`.** A copy of the
> database without the key holds no readable secret. Keep the key on another disk
> or in another backup set, and give it the mode 0600.

- The server makes the key at its first start. It never sends the key to a
  computer and never writes it into the database.
- If the key is lost, the secrets are lost. The server keeps running. Each
  computer that still holds a value sends it again at its next link, so a lost
  key is a lost key and not a lost value while one computer still holds it.
- A lost computer is a computer that holds every key and every value. Remove its
  ssh login at the server, and then replace the machine keys and the credentials:
  removing the login does not rotate them.

## Build the binary

1. On the server computer, go to a checkout of this repository.

2. Build the release binary:

   ```sh
   git lfs pull
   cargo build --release
   ```

   Result: `target/release/horizon` exists.

3. Copy the binary to a folder of its own on the server computer, such as
   `/Users/<user>/bin/horizon`.

4. Run the binary by hand one time to see its log:

   ```sh
   HORIZON_SERVER_HOME="$HOME/.horizon-server" ./horizon --server
   ```

   Result: the process writes one line about the socket and stays in the
   foreground. Stop it with Ctrl-C.

## Run the server at login

The server must run while the other computers use it, and a person wants it to
start without a terminal. One service of the user starts it at login and starts it
again when it ends: a launchd user agent on macOS, and a systemd user unit on
Linux. Both start the binary with `--server`, and both use the label
`net.horizon.server`.

### macOS

1. Make the folders of the server home and of its log:

   ```sh
   mkdir -p "$HOME/Library/LaunchAgents" "$HOME/.horizon-server/log"
   chmod 700 "$HOME/.horizon-server"
   ```

2. Make the file `~/Library/LaunchAgents/net.horizon.server.plist` with this text.
   Replace each `CHANGE-ME` with your user name, and change
   `/Users/CHANGE-ME/bin/horizon` when the binary is in another folder:

   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
   <plist version="1.0">
   <dict>
     <key>Label</key>
     <string>net.horizon.server</string>

     <key>ProgramArguments</key>
     <array>
       <string>/Users/CHANGE-ME/bin/horizon</string>
       <string>--server</string>
     </array>

     <!-- Keep this block only when the server home is not ~/.horizon-server. -->
     <!--
     <key>EnvironmentVariables</key>
     <dict>
       <key>HORIZON_SERVER_HOME</key>
       <string>/Users/CHANGE-ME/.horizon-server</string>
     </dict>
     -->

     <key>RunAtLoad</key>
     <true/>

     <key>KeepAlive</key>
     <true/>

     <key>ProcessType</key>
     <string>Background</string>

     <key>StandardOutPath</key>
     <string>/Users/CHANGE-ME/.horizon-server/log/server.log</string>

     <key>StandardErrorPath</key>
     <string>/Users/CHANGE-ME/.horizon-server/log/server.log</string>

     <key>ExitTimeOut</key>
     <integer>10</integer>
   </dict>
   </plist>
   ```

   `RunAtLoad` starts the server at each login, and `KeepAlive` starts it again
   when it ends. `ProcessType` is `Background`, so the system does not slow the
   server down for a person who waits at the window. The two paths name one log
   file, because the log of the server is its standard error. `ExitTimeOut` gives
   the server ten seconds to close its store at a stop.

3. To hand out invitations as QR codes, the `ProgramArguments` array holds the
   listener too. A plist comment cannot hold the `--` of a flag, so replace the
   array of step 2 with this one:

   ```xml
   <key>ProgramArguments</key>
   <array>
     <string>/Users/CHANGE-ME/bin/horizon</string>
     <string>--server</string>
     <string>--listen</string>
     <string>100.64.0.1:7000</string>
     <string>--name</string>
     <string>server-computer</string>
   </array>
   ```

   `100.64.0.1:7000` is an address of a private overlay network, and
   `server-computer` is the name that the other computers resolve. The server
   refuses every other address.

4. Load the agent:

   ```sh
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/net.horizon.server.plist
   ```

   Result: the agent starts the server now, and at each login.

5. Read the state and the log:

   ```sh
   launchctl print "gui/$(id -u)/net.horizon.server" | head -20
   tail -n 20 "$HOME/.horizon-server/log/server.log"
   ```

   Result: the state is `running`, and the log holds the line about the socket.

6. Stop the server and remove it from the login items:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server"
   ```

   Result: the server receives `SIGTERM`, closes its store and removes its socket.
   The plist stays in `~/Library/LaunchAgents`, so a delete of that file removes
   the agent for good.

7. To put a new binary in place, boot the agent out, copy the file, and boot it in
   again:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server"
   cp target/release/horizon /Users/CHANGE-ME/bin/horizon
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/net.horizon.server.plist
   ```

### Linux

1. Make the folder of the unit and the server home:

   ```sh
   mkdir -p "$HOME/.config/systemd/user" "$HOME/.horizon-server"
   chmod 700 "$HOME/.horizon-server"
   ```

2. Make the file `~/.config/systemd/user/horizon-server.service` with this text.
   Replace each `CHANGE-ME` with your user name, and change
   `/home/CHANGE-ME/bin/horizon` when the binary is in another folder:

   ```ini
   [Unit]
   Description=Horizon server

   [Service]
   Type=simple
   ExecStart=/home/CHANGE-ME/bin/horizon --server
   Restart=on-failure
   # Keep this line only when the server home is not ~/.horizon-server.
   # Environment=HORIZON_SERVER_HOME=/home/CHANGE-ME/.horizon-server

   [Install]
   WantedBy=default.target
   ```

   `Restart=on-failure` starts the server again when it ends with a failure, for
   example when the address of the listener is not there yet.

3. Read the unit and start it:

   ```sh
   systemctl --user daemon-reload
   systemctl --user enable --now horizon-server
   ```

   Result: the unit runs now, and at each login of the user.

4. Read the state and the log:

   ```sh
   systemctl --user status horizon-server
   journalctl --user -u horizon-server -n 20
   ```

   Result: the state is `active (running)`, and the log holds the line about the
   socket. The standard output and the standard error of the unit go to the
   journal.

5. Stop the server and remove it from the login items:

   ```sh
   systemctl --user disable --now horizon-server
   ```

   Result: the server receives `SIGTERM`, closes its store and removes its socket.

6. On a computer where nobody logs in, a linger starts the unit at boot and keeps
   it after a logout. Run this step on the server computer only:

   > **CAUTION:** ENABLE A LINGER ONLY FOR THE USER OF THE SERVER. The linger keeps
   > every process of that user alive after the logout, also a process that the
   > person forgot, and the server holds every secret of the person.

   ```sh
   sudo loginctl enable-linger CHANGE-ME
   ```

   Result: the unit starts when the computer starts, without a login.

### The server home

The service reads and writes the server home at each start:
`HORIZON_SERVER_HOME`, or `~/.horizon-server`. The home holds these files:

| File | What it is |
|---|---|
| `server.sqlite` | The store: the machine list, the keys and the values, sealed. |
| `secret.key` | The key that seals each value of the store. |
| `server.sock` | The socket that the link of a computer reaches. |
| `server.lock` | The lock that says that a server runs. |
| `log/server.log` | The log of the macOS agent. The Linux unit writes the journal. |

The socket belongs to one run of the server: a stop removes it and a start makes
it. The lock file stays, because the lock is the open file of the running server.
The log grows with each run. `server.sqlite` and `secret.key` survive a restart of
the service, with its `-wal` and `-shm` files, so a server that stops and starts
again answers with the same machine list, the same keys and the same values.

A person must keep `secret.key` safe. Without it, the server reads no stored
secret, and a copy of `server.sqlite` alone holds no readable value. The section
[The key of the secrets](#the-key-of-the-secrets) gives the rules of that key.

### The setting of the other computers

Each other computer names the server in **Settings** (**Ctrl+Shift+,**), in the
**Server** tab. Two fields name it:

- **SSH target**: the SSH name of the server computer, for example
  `server-computer`.
- **Command**: the absolute path of the binary and `--server-link`, for example
  `/Users/<user>/bin/horizon --server-link`.

A non-interactive SSH session of macOS has only the system folders on its `PATH`,
so it finds neither `/usr/local/bin` nor `~/bin`. The **Command** field gives the
whole path, so the link needs no `PATH`. Without that field, Horizon runs
`horizon --server-link` and the `PATH` of the login decides.

### Check the service

1. Ask the server for one invitation while the service runs:

   ```sh
   HORIZON_SERVER_HOME="$HOME/.horizon-server" <path>/horizon --server-invite
   ```

   Result: the command prints the link, the code, the minutes that it lives and
   the QR code of the link as blocks of text. It ends with exit code 0. Without
   `--listen`, the answer is one sentence about a server that has no listener.

2. Stop the service, as the macOS steps or the Linux steps above say, and run the
   same command again.

   Result: the command prints one line and ends with exit code 1:

   ```text
   error: no Horizon server answers at /Users/<user>/.horizon-server/server.sock; start `horizon --server` on that computer
   ```

   The mode of the same home that ends with exit code 3 is `--server-link`: that
   is the command of the link of another computer, and a computer reads that code
   as a server that is away.

## Set the server on each computer

One computer reaches the server in one of two ways: through the SSH login of the
server computer, or with a link that a logged-in computer shows as a QR code. The
SSH link stays as the second transport, and a person who has no SSH key on a
computer joins it with a code.

1. In **Settings** (**Ctrl+Shift+,**) of Horizon on each computer, open the
   **Server** tab and set the **SSH target** field:

   `server-computer` is a name that `ssh` resolves, such as the `Host` name of your
   own SSH configuration. Horizon adds the command `horizon --server-link` itself.

2. When the `PATH` of the `ssh` login does not hold the binary, set the
   **Command** field to the whole path:

   `/Users/<user>/bin/horizon --server-link`

   The section [The setting of the other computers](#the-setting-of-the-other-computers)
   says why a non-interactive SSH session needs that path.

3. Start Horizon again. The Machines section shows the line `Server: in step`.

## Join a computer with a QR code

The second transport is a listener that you turn on, and one code per computer.
Horizon encrypts nothing on it: the listener accepts only a loopback address, an
address inside `100.64.0.0/10`, or an address inside `fd7a:115c:a1e0::/48`, which
are the addresses of a private overlay network. The overlay carries the
encryption.

1. On the server computer, add the listener and the name to the service, as the
   macOS or the Linux steps of [Run the server at login](#run-the-server-at-login)
   say, and start the service again.

   Result: the log names the socket and the listener. Without `--listen` the
   server opens no port, and an invitation answers `no_listener`.

2. On the server computer, ask for the first invitation:

   ```sh
   HORIZON_SERVER_HOME="$HOME/.horizon-server" /Users/<user>/bin/horizon --server-invite
   ```

   Result: the command prints a link, a code in three groups of four, the minutes
   that it lives, and a QR code as blocks of text.

3. On the new computer, open **Settings** (**Ctrl+Shift+,**) and select the
   **Server** tab. Paste the link, type a name for that computer, and press
   **Join**.

   Result: the tab says `This computer joined the server.`, the settings hold the
   address of the listener, and the sidebar reads `Server: in step`.

4. On a computer that is logged in, press **Add a computer** in the same tab.

   Result: the tab paints a QR code on a white plate, the link, the code and the
   minutes that are left. Another computer joins with the same link and the code.
   One invitation works for one computer, for ten minutes, and for one use.

5. To log a computer out, press **Remove** beside its row in the list of the
   computers, and confirm.

   Result: the login of that computer is gone and its open links end. That
   computer keeps every copy of every machine key and every credential value that
   it already had, so make new keys and new values if it is lost.

## Replace the binary and restart

1. Build the new binary.

2. Restart the service with the new binary:

   ```sh
   launchctl bootout "gui/$(id -u)/net.horizon.server"
   cp target/release/horizon /Users/CHANGE-ME/bin/horizon
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/net.horizon.server.plist
   ```

   On Linux, copy the file over the old path and run
   `systemctl --user restart horizon-server` instead.

   Result: the process ends with `SIGTERM`, removes its socket, closes its store,
   and the service starts the new binary.

3. Read the log again, as step 5 of [Run the server at login](#run-the-server-at-login)
   says.

A change that a computer made while the server was away stays in its queue and
settles when the server returns. An in-flight push is sent again with the same
identifier, so it changes the store one time.

## Move the server to another computer

A person who replaces the computer that runs the server, or who moves the server
from one operating system to another, moves one folder: the server home. The
machines, the keys, the values, the settings and the computers that joined stay
the same, because every one of them is in that folder.

1. Stop the old server, with the stop step of [Run the server at login](#run-the-server-at-login)
   for its system, and read its log until it holds this line:

   ```text
   the Horizon server stopped
   ```

   Result: the process has ended, its socket is gone, and its store is closed.

   Do not copy from a server that runs, and do not start the old server again
   after the move: two servers with one store id and two contents decide the
   machine list twice.

2. On the old computer, copy the files of the store to the server home of the new
   computer. The user that the copy names owns the files, so start the server of
   the new computer as that user:

   ```sh
   ssh CHANGE-ME@server-computer 'mkdir -p ~/.horizon-server && chmod 700 ~/.horizon-server'
   scp -p ~/.horizon-server/server.sqlite ~/.horizon-server/secret.key \
     CHANGE-ME@server-computer:.horizon-server/
   ```

   ```sh
   ssh CHANGE-ME@server-computer 'chmod 600 ~/.horizon-server/server.sqlite ~/.horizon-server/secret.key'
   ```

   Result: the server home of the new computer holds `server.sqlite` and
   `secret.key`, each at mode 600, in a folder of mode 700.

   Use the path of your own server home when `HORIZON_SERVER_HOME` names one.

3. Copy `server.sqlite-wal` and `server.sqlite-shm` too when the folder of a
   stopped server still holds them and they are not empty. A clean stop ends the
   write-ahead log, so `server.sqlite` alone is the whole store, and the two files
   are usually gone. A killed server leaves them, and they belong with their
   `server.sqlite`: a copy of the database without them loses the last changes.

4. Do not copy `server.lock` and `server.sock`, and do not copy the `log` folder.
   The lock says that a server of that home runs, and it is the open file of that
   process: a copied file blocks nothing and a new server takes it. The socket
   belongs to one run of a server, and a start removes a socket file that no
   process listens on. The log is the history of the old computer.

5. Start the server on the new computer, with the service steps of its system and
   the same flags as the old one: `--listen` and `--name` when the computers of
   the listener reach it. Read its log:

   ```text
   the Horizon server answers
   ```

   Result: the line names the new home, its socket and its listener. The server
   runs with the machine list, the keys and the values of the old computer, and
   its store id is the same as the one of the old computer.

6. On each computer that follows this server, open **Settings** (**Ctrl+Shift+,**),
   select the **Server** tab, and set the fields of **This computer** again:

   - **SSH target**: the SSH name of the new computer.
   - **Command**: the path of the binary on the new computer and `--server-link`,
     when the `PATH` of the login does not hold it.
   - **Address**: the listener of the new computer, such as
     `server-computer:7000`, on a computer that joined with a code. Its token still
     works, because the row of that computer and the hash of its token are in the
     moved store.

   Press **Save**, and start Horizon again.

   Result: the sidebar reads `Server: in step`. The machine list is the one of the
   moved store, a change that waited while the server was away settles by itself,
   and a new change settles too.

7. A computer that was new, or that never reached this server, joins as [Join a
   computer with a QR code](#join-a-computer-with-a-qr-code) says. It reads every
   machine and every named secret of the moved store, and a machine key that the
   moved `secret.key` sealed opens there.

### What the computers do around a move

- **Another store id gives one line and two files.** The server names the id of
  its store in its first frame. A computer that reaches a store id that it did
  not see last takes the machine list and the settings of that store. Before it
  takes them, it writes what it holds in two files of its Horizon home:
  `machines/server-before-reset.<time>.json` holds the machine list and the key
  bindings, and `settings.before-server.<time>.json` holds the settings. The
  sidebar shows one line that names the two files. A move keeps the id, because
  it copies one store, so a move shows no such line. A computer that a person
  points at another server does.
- **A computer that holds more than the copy keeps its own records.** The index
  of a computer is its mark in the log of the store. A computer whose mark is
  above the index of the store that it reached is the newer side of that log. It
  writes its machine list and its settings to the same two files, and the sidebar
  says that the server holds an older state than this computer saw. It also sends
  every machine that the copy does not hold to the server again, so the copy
  becomes as new as the computer for those records. No machine leaves the list of
  the computer.
- **The tokens of a join still work.** A token is a row of the moved store, and
  `secret.key` reads it. The name of the computer is its own file
  `machines/server-device` of its Horizon home: a token under another device name
  is not logged in. The **Address** field of each of those computers must reach
  the listener of the new computer, so its service needs `--listen` and the
  `--name` that the links held.
- **The server entry is the one setting that a move changes.** The `server`
  section stays with one computer and is not shared, so no computer learns the
  new place by itself, and each one keeps its old target until a person sets it.

## A continuous copy of the store

The store is one SQLite database, so a tool that copies its write-ahead log
continuously gives a copy that a person can restore. This repository does not
depend on that tool. The following file is a sample of [Litestream], and it is
text only:

```yaml
dbs:
  - path: /Users/CHANGE-ME/.horizon-server/server.sqlite
    replicas:
      - type: s3
        bucket: CHANGE-ME
        path: horizon-server/CHANGE-ME
      # Or a folder of an external disk:
      # - type: file
      #   path: /Volumes/backup/horizon-server
```

The copy excludes nothing, because the key of the secrets is not in the database:
a copy of `server.sqlite` holds no readable secret. Copy `secret.key` with the
same care, and separately, as the section above says.

Rules of the copy:

- Run one replicating process for one database.
- Stop the server before the first start of the replicator.
- Stop the replicator before a restore.
- The server commits with `synchronous=FULL`, so a committed change is on the
  disk before the answer leaves.

A restore stops the server, writes the newest generation over
`server.sqlite`, and starts the server again. Each computer whose index is above
the restored index then pulls the whole state of the restored store.

[Litestream]: https://litestream.io

## What the server holds

The server holds the machine list, the key that each machine uses, every machine
key of the person and every credential value that Horizon keeps in the OS
credential store. Every value is sealed with `secret.key`, and every linked
computer holds a copy. The key of a machine is one value under
`machine-key/<name>`, and a credential value is one value under
`credential/<reference>`.

The server holds no ssh login key of the person, no provider CLI login (az,
gcloud, hcloud, aws), no browser profile, no session cookie, no agent token, no
transcript and no setting of the person. It stores no terminal content either:
when a computer shares its terminals, the server passes what they show and what
is typed in them on to your other computers, in memory only, and writes none of
it to a file. `secret.key` itself never leaves the server home.

## The limits

- **The boundary is the SSH login of the server user, or one token.** Every
  computer that can log in as that user, and every computer that holds a token,
  reads and writes the whole machine list, every machine key and every credential
  value: `secret.key` is a file of that same login.
- **The server does not prove which computer talks to it over SSH.** The device
  name of such a link is a claim. A link of the listener proves the device with
  its token, and a token is one device.
- **The listener has no encryption of its own.** It accepts only the three
  address ranges of a private overlay network, so the overlay must encrypt and
  authenticate every byte. Do not forward its port to a public network.
- **A code is a bearer value for ten minutes.** A person who reads the code of an
  invitation can join that computer to the server, and the server then gives that
  computer every machine key and every credential value.
- **Every linked computer holds a copy of every secret.** A lost computer is a
  computer that holds all of them, so remove its login and replace the keys and
  the credentials.
- **A machine key is one value on every computer.** A key that one computer makes
  is on the others within a few seconds, and a key that one computer deletes is
  gone on all of them. Two computers that change one key at the same time keep the
  first write, and the other computer keeps its own as `<name>.conflict-<time>`.
- **A stopped computer keeps its last list and its last values.** A change that it
  made while the server was away shows `pending` and settles when the link
  returns.
