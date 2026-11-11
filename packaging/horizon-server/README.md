# Horizon server

The Horizon server keeps the machine list that more than one computer of one
person agrees on. Each computer names the same server in its settings, and a
machine that an agent or the person adds on one computer shows on the other
computers within a few seconds.

This folder holds one sample: a launchd user agent for macOS. The server is the
same `horizon` binary that the person already runs, with `--server`. It needs no
second program and no account.

## What the server is

| Part | What it is |
|---|---|
| Program | `horizon --server`, the binary of this repository |
| Server home | `HORIZON_SERVER_HOME`, or `~/.horizon-server` |
| Store | `<server home>/server.sqlite`, mode 0600 |
| Socket | `<server home>/server.sock`, mode 0600 in a directory at mode 0700 |
| Lock | `<server home>/server.lock`, so a second server of the same home refuses |
| Log | The standard error of the process |

The server reads no setting and keeps no file of the person's desktop: its home
is a directory of its own. It answers the links of the computers over the SSH
login of its user. It opens no port.

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

## Install the launchd agent

1. Make the folders of the server home and of its log:

   ```sh
   mkdir -p "$HOME/.horizon-server/log"
   chmod 700 "$HOME/.horizon-server"
   ```

2. Copy `net.horizon.server.plist` to `~/Library/LaunchAgents/`.

3. Replace `CHANGE-ME` in the copy with your user name, so it names the binary
   and the server home.

4. Load the agent:

   ```sh
   launchctl load ~/Library/LaunchAgents/net.horizon.server.plist
   ```

   Result: the agent starts the server at each login and starts it again when it
   ends.

5. Read the state:

   ```sh
   launchctl print "gui/$(id -u)/net.horizon.server" | head -20
   tail -n 20 "$HOME/.horizon-server/log/server.log"
   ```

   Result: the state is `running`, and the log holds the socket line.

## Set the server on each computer

1. In the settings of Horizon on each computer, set the entry `server`:

   ```yaml
   server:
     ssh_target: server-computer
   ```

   `ssh_target` is a name that `ssh` resolves, such as the `Host` name of your own
   SSH configuration. Horizon adds the command `horizon --server-link` itself.

2. When the `PATH` of the `ssh` login does not hold the binary, name the whole
   path:

   ```yaml
   server:
     ssh_target: server-computer
     command: /Users/<user>/bin/horizon --server-link
   ```

3. Start Horizon again. The Machines section shows the line `Server: in step`.

## Replace the binary and restart

1. Build the new binary and copy it over the old path.

2. Restart the agent:

   ```sh
   launchctl kickstart -k "gui/$(id -u)/net.horizon.server"
   ```

   Result: the process ends with `SIGTERM`, removes its socket, closes its store,
   and the agent starts the new binary.

3. Read the log again, as in step 5 of the install section.

A change that a computer made while the server was away stays in its queue and
settles when the server returns. An in-flight push is sent again with the same
identifier, so it changes the store one time.

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

## What the server never holds

The server holds the machine list. It holds no private key, no token, no
terminal content, no transcript, no browser profile, no session cookie and no
setting of the person. The key of a machine is a local binding of each computer.

## The limits

- **The boundary is the SSH login of the server user.** Every computer that can
  log in as that user reads and writes the whole machine list.
- **The server does not prove which computer talks to it.** The device name of a
  link is a claim.
- **A machine that arrived from another computer has no key on this computer.**
  A click on it opens nothing, and the section says `This computer has no key for
  <name> yet`. The public lines of the keys are a later slice.
- **A stopped computer keeps its last list.** A change that it made while the
  server was away shows `pending` and settles when the link returns.
