# Horizon server

The Horizon server keeps the state that more than one computer of one person
agrees on: the machine list, the key that each machine uses, and every machine key
and provider credential value that Horizon holds. Each computer names the same
server in its settings, so a machine that an agent or the person adds on one
computer shows on the other computers within a few seconds, and it opens with the
same key.

This folder holds one sample: a launchd user agent for macOS. The server is the
same `horizon` binary that the person already runs, with `--server`. It needs no
second program and no account.

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

One computer reaches the server in one of two ways: through the SSH login of the
server computer, or with a link that a logged-in computer shows as a QR code. The
SSH link stays as the second transport, and a person who has no SSH key on a
computer joins it with a code.

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

## Join a computer with a QR code

The second transport is a listener that you turn on, and one code per computer.
Horizon encrypts nothing on it: the listener accepts only a loopback address, an
address inside `100.64.0.0/10`, or an address inside `fd7a:115c:a1e0::/48`, which
are the addresses of a private overlay network. The overlay carries the
encryption.

1. On the server computer, stop the server and start it with the address of the
   overlay and the name that the other computers resolve:

   ```sh
   /Users/<user>/bin/horizon --server --listen 100.64.0.1:7000 --name server-computer
   ```

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
terminal content, no transcript and no setting of the person. `secret.key` itself
never leaves the server home.

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
