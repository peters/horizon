---
procedure: shared-terminals
feature: The terminals of a computer that the other computers of its server show and type in
platforms: [linux]
cost: none
destructive: no
secrets: the SSH login of the server computer and one SSH identity of this run
owner: peters
---

# Shared terminals test procedure

## 1. Purpose

This procedure proves that two computers that follow one Horizon server show
the terminals of each other and type in them. It proves the section
**Computers** of the sidebar, the follow panel and the chips of the title bars.
It proves the name and the two switches of the **Server** tab. It proves what
the window shows when a panel, a computer or the server goes away. It proves
the history of a follow panel, the grid that follows the computer that types,
and the chip of a computer that types. It also proves that a computer with no
server shares nothing, and that a window that the desktop does not show
continues its streams.

## 2. Applicability

- Candidate: a build of `horizon` that has the section **Shared terminals** in
  the **Server** tab of its Settings window.
- Platforms: Linux, with the fixture of this repository. The fixture does not
  run on macOS.
- The design is in
  [the page of the shared terminals](../../architecture/shared-terminals.md).
- This procedure does not test a third computer, the limits of the server, an
  agent panel, a link of the TCP listener, or macOS. ST22 starts a third
  Horizon program on the test computer, not a third computer.

## 3. Safety

> **CAUTION:** LINK ONLY THE TWO FIXTURES OF THIS RUN TO THE TEST SERVER. A
> computer that follows a server shares its terminals with each other computer
> of that server, and those computers can type in them.

> **CAUTION:** USE AN SSH IDENTITY THAT YOU MADE FOR THIS RUN. The fixture
> copies the identity into its private home. Do not give it a key of your own
> work.

> **CAUTION:** TYPE ONLY THE COMMANDS OF THIS PROCEDURE IN A FOLLOW PANEL. The
> keys go to a program of the other computer and run there.

> **CAUTION:** WRITE ONLY THE STATE FOLDERS OF THIS RUN. Start each candidate
> through the fixture, which gives it a private home. Do not start it with your
> own home.

The name of a computer is the host name of the test computer until you set a
name. A screenshot or a recording from before task ST3 shows that host name.
Keep that evidence private.

## 4. Equipment and preconditions

- The test computer: Linux, with the prerequisites of
  [the fixture](../../../scripts/device-smoke/README.md).
- The frozen candidate `horizon` and the `horizon-device` program of the same
  build, with their SHA-256, as the fixture page says. `$smoke_bin` names their
  folder below.
- The server computer: a Linux or macOS login that the test computer reaches
  with `ssh` and no prompt. The server computer can be the test computer:
  [section 4.1](#41-a-server-computer-on-the-test-computer) starts an `sshd` for
  that.
- One folder of this run for the SSH files, `$SSH_DIR` below. It holds the
  identity `fixture_key`, whose public key the login of the server computer
  accepts, and the file `config` with this text:

  ```
  Host shared-test-server
    HostName <address of the server computer>
    Port <port of its sshd>
    User <user of the login>
    IdentityFile ~/.ssh/fixture_key
    StrictHostKeyChecking accept-new
  ```

- A copy of the candidate on the server computer, and one state folder there.
  `$SERVER_STATE` names that folder below, for example
  `/var/tmp/hz-shared-server`. Use a path of letters, digits and `._/-` only.
- Three state paths on the test computer that do not exist:
  `/var/tmp/hz-shared-base`, `/var/tmp/hz-shared-a` and `/var/tmp/hz-shared-b`.
- A Horizon of your own with the `device_panel` tool, for the live views.
- For task ST15 only: an older build of `horizon` on the server computer, whose
  server answers protocol version 5. Build the parent of the commit that this
  command prints:

  ```sh
  git log --diff-filter=A --format=%H -- crates/horizon-sync/src/live.rs
  ```

These names apply in this procedure:

- Computer A is the candidate of `/var/tmp/hz-shared-a`, and computer B is the
  candidate of `/var/tmp/hz-shared-b`.
- "Run a command in a panel" means: type the command in that panel and press
  Enter.
- "The line of an ended stream" is the line below the grid of a follow panel.
  It starts with `The stream of this terminal ended:`, gives a cause, and ends
  with `Text typed here is not sent.`

Send each click and each key to a candidate with the `horizon-device` program.
Give it the file `target.json` of the state path of that candidate, as the
fixture page says. Take a new screenshot before each click.

### 4.1 A server computer on the test computer

Do this section only if you have no second computer. The commands make the SSH
files of this run and start an `sshd` of your own user on a loopback port. The
configuration is the one that the fixture writes for the `sshd` of its machine
lane. It needs OpenSSH 9.8 or later, which has the two session programs.

1. Unpack an `sshd` into a folder of this run, and make the two keys:

   ```sh
   export SSH_DIR=/var/tmp/hz-shared-ssh
   mkdir -m 700 "$SSH_DIR"
   cd "$SSH_DIR"
   apt-get download openssh-server
   dpkg-deb -x openssh-server_*.deb "$SSH_DIR/openssh-server"
   ssh-keygen -q -t ed25519 -N '' -f "$SSH_DIR/host_key"
   ssh-keygen -q -t ed25519 -N '' -f "$SSH_DIR/fixture_key"
   cp "$SSH_DIR/fixture_key.pub" "$SSH_DIR/authorized_keys"
   ```

   Result: `$SSH_DIR` holds the program `openssh-server/usr/sbin/sshd`, two key
   pairs and `authorized_keys`.

2. Write the configuration of the `sshd`, with a port that no process uses:

   ```sh
   cat > "$SSH_DIR/sshd_config" <<EOF
   Port 17872
   ListenAddress 127.0.0.1
   HostKey $SSH_DIR/host_key
   AuthorizedKeysFile $SSH_DIR/authorized_keys
   PidFile $SSH_DIR/sshd.pid
   UsePAM no
   PasswordAuthentication no
   KbdInteractiveAuthentication no
   StrictModes no
   SshdSessionPath $SSH_DIR/openssh-server/usr/lib/openssh/sshd-session
   SshdAuthPath $SSH_DIR/openssh-server/usr/lib/openssh/sshd-auth
   EOF
   ```

   Result: The file `$SSH_DIR/sshd_config` exists.

3. Start the `sshd` in a terminal and keep it there:

   ```sh
   "$SSH_DIR/openssh-server/usr/sbin/sshd" -D -f "$SSH_DIR/sshd_config" -E "$SSH_DIR/sshd.log"
   ```

   Result: The file `$SSH_DIR/sshd.log` says that the server listens on
   `127.0.0.1` and the port.

4. Write `$SSH_DIR/config` with the text above. Use the address `127.0.0.1`,
   the port of step 2 and your own user name.

   Result: The folder `$SSH_DIR` holds `config` and `fixture_key`.

## 5. Setup

1. Freeze the candidate and record its SHA-256, as the fixture page says.

   Result: `$smoke_bin` holds `horizon`, `horizon-device` and `SHA256SUMS`.

2. On the server computer, copy the candidate to `$SERVER_STATE/horizon` and
   make the server home:

   ```sh
   export SERVER_STATE=/var/tmp/hz-shared-server
   mkdir -p "$SERVER_STATE/server-home"
   chmod 700 "$SERVER_STATE/server-home"
   ```

   Result: The folder `$SERVER_STATE/server-home` exists at mode 700.

3. On the server computer, start the server in a terminal and keep it there:

   ```sh
   HORIZON_SERVER_HOME="$SERVER_STATE/server-home" "$SERVER_STATE/horizon" --server
   ```

   Result: The process stays in the foreground. It writes one line about its
   socket to the standard error.

4. In a terminal of the test computer, set `SERVER_STATE` to the path of the
   server computer:

   ```sh
   export SERVER_STATE=/var/tmp/hz-shared-server
   ```

   Result: The command of the next step holds the paths of the server computer.
   That command line must have 200 characters or fewer.

5. Start computer A.

   > **CAUTION:** THIS STEP LINKS COMPUTER A TO THE TEST SERVER. From this step,
   > the test server gets each terminal of computer A.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --state /var/tmp/hz-shared-a --saved-session --reuse-state \
     --ssh-config "$SSH_DIR/config" --ssh-identity "$SSH_DIR/fixture_key" \
     --server-target shared-test-server \
     --server-command "env HORIZON_SERVER_HOME=$SERVER_STATE/server-home $SERVER_STATE/horizon --server-link"
   ```

   Result: The fixture prints its manifest and stays in the foreground. The
   candidate window shows one workspace with the panels `Live render heartbeat`
   and `Device input test`.

6. Create a Device panel with the `device_panel` tool. Give it the
   `vnc_address` of `/var/tmp/hz-shared-a/lab.json`.

   Result: The Device panel shows the candidate window of computer A.

7. Do the viewer health check of the fixture page for that Device panel.

   Result: The viewer is connected, and its frames advance.

8. Read the section **Machines** of the sidebar of computer A.

   Result: The line is `Server: connecting` and then `Server: in step`.

9. Start computer B with the command of step 5 and the state path
   `/var/tmp/hz-shared-b`.

   > **CAUTION:** THIS STEP LINKS COMPUTER B TO THE TEST SERVER.

   Result: The candidate window of computer B shows the same two panels.

10. Do steps 6 to 8 for computer B, with `/var/tmp/hz-shared-b/lab.json`.

    Result: A second Device panel shows the candidate window of computer B. The
    sidebar of computer B says `Server: in step`.

## 6. Tasks

### 6.1 ST1: The baseline, a computer with no server

1. Start one more fixture with no server option:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
     --native-view --state /var/tmp/hz-shared-base
   ```

   Result: The candidate window shows the panels `Live render heartbeat` and
   `Device input test`.

2. Create a Device panel for the `vnc_address` of
   `/var/tmp/hz-shared-base/lab.json`.

   Result: The Device panel shows the candidate window.

3. Read the sidebar and the title bars of the two panels.

   Result: The sidebar has no section **Computers**. No title bar has a chip
   that starts with `seen by` or with `on`.

4. Open **Settings** (**Ctrl+Shift+,**) and select the **Server** tab.

   Result: The first line says `This computer has no server.` The tab has no
   section **Shared terminals**. The section **Join with a link** shows this
   sentence: `The other computers that you link to this server can see the
   terminals of this computer and type in them. Anyone who gets into one of
   those computers, or into the server, can then run commands here.`

5. Read the sidebar of computer A and of computer B.

   Result: Each section **Computers** shows one computer only.

6. Stop this fixture with Ctrl-C in its terminal.

   Result: The candidate window closes.

7. Close the Device panel of this fixture.

   Result: Two Device panels of this run stay.

### 6.2 ST2: The section Computers and its rows

1. Read the sidebar of computer B below the section **Machines**.

   Result: The section **Computers** shows one computer. Its name is the host
   name of the test computer. Below the name are the rows `Live render
   heartbeat` and `Device input test`.

2. Put the pointer on the name of that computer.

   Result: The hover text is the device name of computer A: `horizon-` and
   eight characters. The file
   `/var/tmp/hz-shared-a/data/home/.horizon/machines/server-device` holds the
   same name.

3. Put the pointer on the row `Device input test`.

   Result: The hover text has two lines. The first line names the kind and the
   title of the panel, for example `Shell: Device input test`. The second line
   ends with `you can type in it`.

4. Read the sidebar of computer A.

   Result: The section **Computers** shows one computer with the same two rows.
   Its hover text is the device name of computer B. Computer A is not in its
   own section.

### 6.3 ST3: The name of a computer

1. On computer A, open **Settings** (**Ctrl+Shift+,**) and select the **Server**
   tab.

   Result: The section **Shared terminals** shows the sentence of ST1 and the
   empty field **Name of this computer** with the hint `the host name`. It
   shows the switches **The other computers see the terminals of this
   computer** and **The other computers type in them**. The two switches are
   on.

2. Type `Computer A` in the field **Name of this computer**.

   Result: The field shows `Computer A`. Each key starts the live link of
   computer A again, so the section of computer B can change for a moment.

3. Press **Save**.

   Result: The status line of the Settings window says `Saved`.

4. On computer B, do steps 1 to 3 with the name `Computer B`.

   Result: The status line says `Saved`.

5. Read the section **Computers** of each computer.

   Result: Computer B shows the name `Computer A`. Computer A shows the name
   `Computer B`. The hover text of each name is the device name, as before.

### 6.4 ST4: A row opens a follow panel

1. On computer B, click the row `Device input test` of `Computer A`.

   Result: A new panel opens in the workspace of computer B and has the focus.
   Its title is `Device input test`. Its title bar shows the chip `on Computer
   A`. Its grid shows the text of the panel `Device input test` of computer A.
   The body of the new panel has the size of the grid of that terminal, so the
   first key that you press in it changes no size of the panel of computer A.

2. Read the title bar of the panel `Device input test` of computer A.

   Result: The title bar shows the chip `seen by Computer B`.

3. On computer B, click the same row again.

   Result: No second panel opens. The follow panel of step 1 has the focus.

4. On computer B, click the title bar of the follow panel with the secondary
   button.

   Result: The menu of the panel opens. It has no entry `Restart` and no entry
   `Reconnect`.

5. Press Escape.

   Result: The menu closes.

### 6.5 ST5: Live output

1. On computer B, click the row `Live render heartbeat` of `Computer A`.

   Result: A follow panel opens with the chip `on Computer A`. It shows lines
   that start with `Live frame heartbeat:`.

2. Compare the newest line of that follow panel with the panel `Live render
   heartbeat` of computer A for ten seconds.

   Result: Each second, the two panels show a new line with the same number.

3. Read the title bar of the panel `Live render heartbeat` of computer A.

   Result: The title bar shows the chip `seen by Computer B`.

### 6.6 ST6: The follower and the owner type in one program

1. On computer B, click in the follow panel of `Device input test`.

   Result: The follow panel has the focus.

2. On computer B, run `echo from-b` in that follow panel.

   Result: The panel of computer A and the follow panel of computer B show the
   command and the line `from-b`.

3. On computer A, click in the panel `Device input test`.

   Result: The panel has the focus.

4. On computer A, run `echo from-a` in that panel.

   Result: The two panels show the command and the line `from-a`.

5. On computer A, type `echo a-` in that panel. Do not press Enter.

   Result: The two panels show `echo a-` at the prompt.

6. On computer B, type `b` in the follow panel.

   Result: The two panels show `echo a-b` at the prompt.

7. On computer B, press Enter in the follow panel.

   Result: The two panels show the line `a-b`. One program got the keys of the
   two computers.

### 6.7 ST7: The grid follows the computer that types

1. On computer A, click in the panel `Device input test`, and run `stty size`
   in that panel.

   Result: The two panels show two numbers: the rows and the columns of the
   panel of computer A.

2. Record the two numbers.

   Result: The record holds the grid of the body of the panel of computer A.

3. On computer B, click in the follow panel of `Device input test`, and run
   `stty size` in that panel.

   Result: The two panels show the rows and the columns of the body of the
   follow panel of computer B. The numbers differ from the record of step 2
   when the two bodies have different sizes.

4. Read the title bar of the panel `Device input test` of computer A.

   Result: The chip says `sized by Computer B`, followed by `seen by Computer
   B`. The panel of computer A draws the grid of the body of computer B from
   its top left corner, and it cuts the cells that do not fit.

5. On computer A, click in the panel `Device input test`, and run `stty size`
   in that panel.

   Result: The two panels show the numbers of step 2 again. The chip of the
   panel no longer says `sized by`.

6. On computer A, drag the resize handle of the panel `Device input test`, and
   make the panel wider and higher.

   Result: The panel of computer A is larger. The follow panel of computer B
   keeps its own size.

7. On computer A, run `stty size` in that panel.

   Result: The two numbers are larger than the numbers of step 2. If the new
   grid is larger than the follow panel, the follow panel shows the top left
   part of the grid.

8. On computer B, run `stty size` in the follow panel.

   Result: The two numbers are the numbers of step 3 again. The chip of the
   panel of computer A says `sized by Computer B` again: the computer that
   typed last gives the program its grid.

9. On computer B, run `seq -s ' ' 1 150` in the follow panel.

   Result: The two panels break each line of numbers after the same column,
   because one program has one grid.

### 6.8 ST8: A program on the alternate screen

1. On computer B, close the follow panel of `Device input test` with the close
   control of its title bar.

   Result: The follow panel closes. The chip `seen by Computer B` goes away from
   the panel `Device input test` of computer A.

2. On computer A, run `seq 1 300 | less` in the panel `Device input test`.

   Result: The panel shows a page of numbers that starts with `1`. The earlier
   commands are not on the screen.

3. On computer B, click the row `Device input test` of `Computer A`.

   Result: A follow panel opens and shows the same page of numbers.

4. On computer A, press `q` in the panel `Device input test`.

   Result: The panel of computer A shows the earlier commands and the prompt
   again.

5. Read the follow panel of computer B.

   Result: In three seconds or less, the follow panel shows the same earlier
   commands and the same prompt as the panel of computer A. It does not stay
   empty.

### 6.9 ST9: The switch for typing

> **CAUTION:** CHANGE THE SWITCHES OF COMPUTER A AND COMPUTER B ONLY. A switch
> changes what the other computers of a server can do on a computer.

1. On computer A, open the **Server** tab of the Settings window.

   Result: The section **Shared terminals** shows the two switches, and they
   are on.

2. Clear the switch **The other computers type in them**.

   Result: The switch is off.

3. Press **Save**.

   Result: In a few seconds, each follow panel of computer B shows the chip `on
   Computer A · view only`.

4. On computer B, put the pointer on the row `Device input test`.

   Result: The second line of the hover text is `On Computer A, view only`.

5. On computer B, run `echo blocked` in the follow panel of `Device input
   test`.

   Result: No new text shows in the panel of computer A or in the follow panel.

6. On computer A, run `echo owner-types` in the panel `Device input test`.

   Result: The two panels show the line `owner-types`.

7. On computer B, close the follow panel of `Device input test`.

   Result: The follow panel closes.

8. On computer B, click the row `Device input test` again.

   Result: A new follow panel opens with the chip `on Computer A · view only`.

9. On computer A, set the switch **The other computers type in them** again.

   Result: The switch is on.

10. Press **Save**.

    Result: In a few seconds, each open follow panel of computer B shows the
    chip `on Computer A`. A panel that is open takes the typing of the newest
    list, so no panel keeps `view only`.

11. On computer B, run `echo typing-back` in the follow panel of `Device input
    test`.

    Result: The two panels show the line `typing-back`.

### 6.10 ST10: The switch of the shared terminals

1. On computer A, clear the switch **The other computers see the terminals of
   this computer** in the **Server** tab.

   Result: The switch is off. The switch **The other computers type in them** is
   disabled.

2. Press **Save**.

   Result: In a few seconds, the two rows of `Computer A` go away from the
   section of computer B. The name `Computer A` stays. No title bar of computer
   A shows a chip `seen by`.

3. Read the two follow panels of computer B.

   Result: Each follow panel keeps its last screen and shows the line of an
   ended stream. The cause is `the other computer closed this terminal or
   shares it no more`.

4. On computer B, run `echo not-sent` in the follow panel of `Device input
   test`.

   Result: No new text shows in a panel of computer A or of computer B.

5. On computer A, set the switch **The other computers see the terminals of this
   computer** again.

   Result: The switch is on, and the second switch is enabled and on.

6. Press **Save**.

   Result: In a few seconds, the two rows of `Computer A` are in the section of
   computer B again. The line of an ended stream goes away from each follow
   panel, and the heartbeat lines advance again. The panels of computer A show
   the chip `seen by Computer B` again.

### 6.11 ST11: The close of a follow panel

1. On computer B, close the follow panel of `Live render heartbeat`.

   Result: The follow panel closes. The row `Live render heartbeat` stays in the
   section.

2. Read the title bar of the panel `Live render heartbeat` of computer A.

   Result: In a few seconds, the chip `seen by Computer B` goes away.

3. On computer B, click the row `Live render heartbeat` again.

   Result: A follow panel opens and shows the heartbeat lines. The chip of
   computer A comes back.

### 6.12 ST12: The close of an owner panel

1. On computer A, close the panel `Device input test` with the close control of
   its title bar. If the candidate asks, accept the close.

   Result: The panel closes.

2. Read the section **Computers** of computer B.

   Result: In a few seconds, the row `Device input test` goes away. The row
   `Live render heartbeat` stays.

3. Read the follow panel of `Device input test` of computer B.

   Result: The follow panel keeps its last screen and shows the line of an
   ended stream. The cause is `the other computer closed this terminal or
   shares it no more`.

4. On computer B, close that follow panel.

   Result: The follow panel closes.

### 6.13 ST13: A restart of the owner

1. Quit the candidate of computer A with the close request of its window, as
   the fixture page says:

   ```sh
   DISPLAY=<display of A> xdotool search --onlyvisible --name Horizon
   DISPLAY=<display of A> xdotool windowfocus <window id>
   DISPLAY=<display of A> xdotool key --window <window id> alt+F4
   ```

   Result: The candidate and its fixture exit. The fixture keeps its private
   home.

2. Read the sidebar and the follow panel of computer B.

   Result: In a few seconds, the section **Computers** goes away. The follow
   panel of `Live render heartbeat` keeps its last screen and shows the line of
   an ended stream. The cause is `the other computer is offline`.

3. Start computer A again with the command of setup step 5. Replace
   `--saved-session --reuse-state` with `--reuse-state --resume-session`.

   Result: The candidate window shows the board of the first run with the panel
   `Live render heartbeat`. The panel `Device input test` is not there. The
   sidebar says `Server: in step`.

4. Close the first Device panel of computer A.

   Result: The Device panel closes.

5. Create a new Device panel for the new `vnc_address` of
   `/var/tmp/hz-shared-a/lab.json`.

   Result: The Device panel shows the candidate window of computer A.

6. Read the sidebar and the follow panel of computer B.

   Result: The section **Computers** shows `Computer A` with the row `Live
   render heartbeat`. The follow panel follows again with no click: the line of
   an ended stream goes away, and the heartbeat lines advance.

### 6.14 ST14: A stop and a start of the server

1. On the server computer, stop the server with Ctrl-C in its terminal.

   Result: The process ends.

2. Read the sidebar of each computer.

   Result: In a few seconds, the section **Machines** says `Server away`. The
   section **Computers** shows the line `Shared terminals: no link` with a
   reason, and no computer.

3. Read the follow panel of computer B and the title bars of computer A.

   Result: The follow panel shows the line of an ended stream. The cause is
   `this computer has no link to its server`. No title bar of computer A shows a
   chip `seen by`.

4. On the server computer, start the server again with the command of setup
   step 3.

   Result: In one minute or less, each sidebar says `Server: in step`. The line
   `Shared terminals: no link` goes away, and each section **Computers** shows
   the other computer.

5. Read the follow panel of computer B.

   Result: The follow panel follows again with no click, and the heartbeat
   lines advance.

### 6.15 ST15: An older server

Do this task only if you have the older build. If not, record `not run`.

1. On the server computer, stop the server with Ctrl-C.

   Result: Each sidebar says `Server away`.

2. Start the older build with the same server home:

   ```sh
   HORIZON_SERVER_HOME="$SERVER_STATE/server-home" <older build>/horizon --server
   ```

   Result: In one minute or less, each sidebar says `Server: in step`.

3. Read the sidebar of each computer for one minute.

   Result: No sidebar has a section **Computers**, and no sidebar shows the line
   `Shared terminals: no link`. The follow panel of computer B shows the line
   of an ended stream with the cause of ST14.

4. Stop the older build with Ctrl-C.

   Result: Each sidebar says `Server away`.

5. Start the candidate server again with the command of setup step 3.

   Result: In one minute or less, each sidebar says `Server: in step`, and each
   section **Computers** shows the other computer.

### 6.16 ST16: The settings and the follow panel after a restart

1. On computer B, clear the switch **The other computers type in them** in the
   **Server** tab.

   Result: The switch is off.

2. Clear the switch **The other computers see the terminals of this computer**.

   Result: The switch is off, and the second switch is disabled.

3. Press **Save**.

   Result: The status line says `Saved`. The section of computer A shows
   `Computer B` with no row.

4. Make sure that computer B shows one follow panel of `Live render heartbeat`.

   Result: The follow panel shows the heartbeat lines.

5. Quit the candidate of computer B with the close request of its window, as in
   ST13 step 1.

   Result: The candidate and its fixture exit.

6. Start computer B again with `--reuse-state --resume-session`, as in ST13
   step 3.

   Result: The candidate window shows the two panels of computer B. It shows no
   follow panel: a follow panel is not restored.

7. Create a new Device panel for the new `vnc_address` of computer B, and close
   the first one.

   Result: The Device panel shows the candidate window of computer B.

8. Open the **Server** tab of the Settings window of computer B.

   Result: The field **Name of this computer** holds `Computer B`. The two
   switches are off, and the second switch is disabled.

9. Read the section **Computers** of computer A.

   Result: The section shows `Computer B` with no row.

10. On computer B, set the two switches again.

    Result: The two switches are on.

11. Press **Save**.

    Result: The section of computer A shows the two rows of `Computer B`.

### 6.17 ST17: A small window and a large window

1. On computer B, click the row `Live render heartbeat` of `Computer A`.

   Result: A follow panel opens with the chip `on Computer A`.

2. Make the candidate window of computer B small:

   ```sh
   DISPLAY=<display of B> xdotool search --onlyvisible --name Horizon windowsize %1 900 700
   ```

   Result: The window is 900 by 700 pixels.

3. Take a screenshot of computer B and examine it.

   Result: The line **Computers**, the name `Computer A` and each row are on
   one line each. No text is drawn on another text. A text that does not fit is
   cut. The chip of the follow panel does not cover its title.

4. Make the candidate window of computer B large:

   ```sh
   DISPLAY=<display of B> xdotool search --onlyvisible --name Horizon windowsize %1 1560 960
   ```

   Result: The window is 1560 by 960 pixels.

5. Take a screenshot of computer B and examine it.

   Result: The section, the rows and the chip show as in step 3. The grid of
   the follow panel starts at the top left corner of its body.

6. Do steps 2 to 5 on computer A, and examine the chip `seen by Computer B`.

   Result: The chip is in the title bar of the panel `Live render heartbeat` at
   the two sizes, and it does not cover the title.

### 6.18 ST18: An owner window that the desktop does not show

The desktop can give a minimized window no frame. The share pump of computer A
must move each byte then.

1. On computer B, click the row `Device input test` of `Computer A`.

   Result: Computer B shows a follow panel for `Device input test`, and the
   follow panel for `Live render heartbeat` of ST17.

2. Record the identifier of the candidate window of computer A:

   ```sh
   WINDOW=$(DISPLAY=<display of A> xdotool search --onlyvisible --name Horizon | head -n 1)
   ```

   Result: `WINDOW` holds one number.

3. Minimize that window:

   ```sh
   DISPLAY=<display of A> xdotool windowminimize "$WINDOW"
   ```

   Result: The Device panel of computer A shows the desktop with no candidate
   window.

4. On computer B, read the follow panel of `Live render heartbeat` for ten
   seconds.

   Result: Each second, the follow panel shows a new line.

5. On computer B, run `echo hidden-b` in the follow panel of `Device input
   test`.

   Result: The follow panel shows the command and the line `hidden-b`.

6. On computer B, run this command in the same follow panel. The program asks
   the terminal of computer A for the position of the cursor:

   ```sh
   printf '\033[6n'; read -r -s -d R -t 5 answer; echo "position ${answer#*[}"
   ```

   Result: In 5 seconds at most, the follow panel shows `position` and two
   numbers with `;` between them.

7. On computer B, close the follow panel of `Live render heartbeat` with the
   close button of its title bar. Then click the row `Live render heartbeat` of
   `Computer A`.

   Result: A follow panel opens. It shows lines that start with
   `Live frame heartbeat:`, and a new line each second.

8. Show the window of computer A again:

   ```sh
   DISPLAY=<display of A> xdotool windowmap "$WINDOW"
   ```

   Result: The Device panel of computer A shows the candidate window. The panel
   `Device input test` shows the line `hidden-b`. The title bar of each panel
   says that Computer B watches it, and the title bar of `Device input test`
   also says `sized by Computer B`, because the person of computer B typed in
   it last.

### 6.19 ST19: The history of a follow panel

This task is a best reading of `draw_history` in
`crates/horizon-core/src/terminal/share.rs`: no run recorded how many lines a
follow panel keeps above its viewport.

1. On computer A, run `seq 1 300` in the panel `Device input test`.

   Result: The panel of computer A shows the last numbers, and the earlier
   numbers are above its viewport.

2. On computer B, close the follow panel of `Device input test` if it is open.
   Then click the row `Device input test` of `Computer A`.

   Result: A new follow panel opens. Its grid shows the same numbers as the
   panel of computer A.

3. On computer B, put the pointer on the grid of the follow panel and turn the
   wheel up.

   Result: The follow panel shows the lines above its viewport, and the oldest
   line that it shows holds `1`. A new follower gets the history of the
   terminal.

4. On computer B, turn the wheel down until the newest line shows again.

   Result: The follow panel shows the newest line of the grid again.

5. On computer A, drag the resize handle of the panel `Device input test`, and
   make the panel wider.

   Result: The follow panel gets a new screen with the new grid, and that grid
   starts at the top left corner of its body.

6. On computer B, turn the wheel up in the follow panel again.

   Result: The follow panel still shows the lines above its viewport, and the
   oldest line still holds `1`. A screen that is not fresh keeps the history of
   each follower.

### 6.20 ST20: The chip of a panel that another computer types in

1. On computer A, read the title bar of the panel `Device input test`.

   Result: The chip says `seen by Computer B`, and it does not say `typed by`.

2. On computer B, run `echo chip` in the follow panel of `Device input test`.

   Result: In 2 seconds or less, the title bar of the panel of computer A holds
   `typed by Computer B`.

3. Wait 3 seconds without a key on either computer, and read that title bar
   again.

   Result: The chip says `seen by Computer B` again, and no part of it says
   `typed by`. The name stays for 2 seconds after the last byte.

4. On computer B, type one key in the follow panel, and do not press Enter.

   Result: The title bar of the panel of computer A holds `typed by Computer B`
   again.

### 6.21 ST21: A computer that vanishes

This task is a best reading: no run recorded the 30 seconds of silence on a
real server. The fixture starts each child in a process group of its own, so
the signal reaches the candidate and its `ssh` child, and the transport stays
open.

1. On computer B, read the section **Computers** of the sidebar.

   Result: The section shows `Computer A` with its two rows.

2. Find the process group of the candidate of computer A:

   ```sh
   APP=$(python3 - <<'PY'
   import json, pathlib
   lab = json.loads(pathlib.Path('/var/tmp/hz-shared-a/lab.json').read_text())
   for pid in lab['pids']:
       try:
           cmd = pathlib.Path(f'/proc/{pid}/cmdline').read_bytes()
       except OSError:
           continue
       if lab['app'].encode() in cmd:
           print(pid)
   PY
   )
   echo "$APP"
   ```

   Result: `APP` holds one number: the child of the fixture that runs the
   candidate.

3. Suspend that child:

   ```sh
   kill -STOP -- "-$APP"
   ```

   Result: The candidate window of computer A stops to change, and the fixture
   stays in the foreground.

4. On computer B, read the section **Computers** each 10 seconds. Do not wait
   more than 45 seconds.

   Result: In about half a minute, the name `Computer A` and its two rows go
   away. Each follow panel of computer B shows the line of an ended stream, and
   the cause is `the other computer is offline`.

5. Continue that child:

   ```sh
   kill -CONT -- "-$APP"
   ```

   Result: In one minute or less, each sidebar shows `Computer A` with its two
   rows again.

6. Read the follow panels of computer B.

   Result: Each follow panel follows again with no click, and the heartbeat
   lines advance.

### 6.22 ST22: Two programs of one computer

This task is a best reading: no run recorded two Horizon programs with one
device name. Both programs start on the test computer, with two state paths and
two displays.

1. Stop the candidate of computer A with Ctrl-C in the fixture terminal.

   Result: The fixture and the candidate stop. The private home stays in the
   state path.

2. Copy the private home of computer A to a third state path:

   ```sh
   mkdir -p /var/tmp/hz-shared-c/data
   cp -a /var/tmp/hz-shared-a/data/home /var/tmp/hz-shared-c/data/home
   ```

   Result: The third state path holds the device name, the settings and the SSH
   files of computer A.

3. Start computer A again with the command of setup step 5.

   Result: The candidate window shows the two panels of computer A, and its
   sidebar says `Server: in step`.

4. Start the third program with the command of setup step 5 and the state path
   `/var/tmp/hz-shared-c`.

   Result: The fixture starts a second Horizon program with the device name of
   computer A, and its sidebar says `Server: in step`.

5. Read the sidebar of computer A.

   Result: In a few seconds, the section **Machines** says `Server: in step`
   and the line `Shared terminals: no link` shows under it, with the reason
   `another Horizon of this computer holds the live link`.

6. Read the section **Computers** of computer B for one minute.

   Result: The section shows one `Computer A` with its two rows. No row goes
   away and comes back, because the program that loses the link waits 60
   seconds before its next attempt.

7. Stop the third program with Ctrl-C in its fixture terminal.

   Result: The fixture and the third program stop.

8. Wait one minute, and read the section **Computers** of computer B.

   Result: Computer A opens a new live link, and the follow panels of computer
   B follow again with no click.

### 6.23 ST23: The owner window closed with Keep Horizon running

The close of the window and its ask are in
[the keep-running procedure](keep-running-when-closed.md). This task proves
that the share continues while the window of the owner is hidden.

1. On computer A, open the Settings window and find the row **When the window
   is closed** in the **General** tab.

   Result: The row shows the three values **Ask**, **Keep Horizon running** and
   **Quit Horizon**.

2. Select **Keep Horizon running**, and press **Save**.

   Result: The row keeps the value **Keep Horizon running**.

3. On computer B, click the row `Live render heartbeat` of `Computer A`.

   Result: A follow panel opens, and a new line shows each second.

4. On computer A, close the main window with `alt+F4`.

   > **CAUTION:** THE NEXT STEP TYPES IN A PROGRAM OF COMPUTER A. The program of
   > a shared terminal runs with the rights of the person of computer A.

   Result: The candidate window goes away at once, and no question shows. The
   fixture stays in the foreground.

5. Read the Device panel of computer A.

   Result: The Device panel shows the desktop of computer A with no candidate
   window.

6. On computer B, read the follow panel of `Live render heartbeat` for ten
   seconds.

   Result: Each second, the follow panel shows a new line. The window of the
   owner draws no frame, so the share pump moves each byte.

7. On computer B, run `echo hidden-share` in the follow panel of `Device input
   test`.

   Result: The follow panel shows the command and the line `hidden-share`. A
   start of Horizon shows the hidden window again, as
   [the keep-running procedure](keep-running-when-closed.md#63-kr-3--enter-keeps-horizon-running)
   says; this task proves the share of the hidden window alone.

8. Stop the fixture of computer A with Ctrl-C in its terminal.

   Result: The candidate ends. Each follow panel of computer B shows the line
   of an ended stream, and the cause is `the other computer is offline`.

## 7. Pass criteria

- ST1: a computer with no server shows no section **Computers**, no chip and no
  section **Shared terminals**, and the join shows the sentence.
- ST2: each computer lists the other computer and its two panels, and not
  itself.
- ST3: the name of the settings is the name that the other computer shows.
- ST4: a row opens one follow panel, the two chips show, and the menu of the
  follow panel has no restart entry.
- ST5: the follow panel shows the output of the owner each second.
- ST6: the keys of the two computers go to one program.
- ST7: the program takes the grid of the computer that typed last, the window
  of the owner draws that grid and cuts it, and a key of the owner takes the
  grid of its own body back.
- ST8: the follower shows the screen below the alternate screen after the
  program leaves it.
- ST9: with typing off, the chip says `view only` and the keys of the follower
  do nothing.
- ST10: with the first switch off, the rows go away and each follow panel gives
  the cause. With the switch on again, each follow panel follows again with no
  click.
- ST11: the close of a follow panel removes the chip of the owner.
- ST12: the close of an owner panel ends the stream with its cause.
- ST13: the follow panel says that the owner is offline, and it follows again
  after the restart.
- ST14: the two computers say that the link is lost, and they share again after
  the server starts.
- ST15: an older server keeps the machine list in step and shows no section
  **Computers**.
- ST16: the name and the two switches stay after a restart, and no follow panel
  comes back.
- ST17: no text is drawn on another text at the two window sizes.
- ST18: while the window of the owner is minimized, the follower gets the
  output, its keys go to the program, the terminal of the owner answers the
  question of a program, and a new follow panel gets a screen.
- ST19: a new follower reads the lines above the viewport, and a screen that is
  not fresh keeps them.
- ST20: the chip of a panel names the computer that typed, for 2 seconds.
- ST21: a computer that the desktop stops leaves the list in about half a
  minute, and it returns with a new link.
- ST22: one device name in two programs leaves one entry in the list, it does
  not flap, and the program that loses says so.
- ST23: a window that is hidden keeps its programs and its share.

## 8. Cleanup

1. Stop each fixture with Ctrl-C in its terminal.

   Result: Each candidate window closes, and each fixture stops its display.

2. Close each Device panel of this run.

   Result: No Device panel of this run stays.

3. On the server computer, stop the server with Ctrl-C.

   Result: The process ends.

4. On the server computer, remove the state folder of this run.

   > **CAUTION:** REMOVE ONLY THE STATE FOLDER OF THIS RUN. The folder holds the
   > store and the key of the test server, and its path is the one in
   > `SERVER_STATE`.

   ```sh
   rm -rf "$SERVER_STATE"
   ```

   Result: The server home and the copy of the candidate are gone.

5. On the test computer, remove the three state paths of this run.

   > **CAUTION:** REMOVE ONLY THE STATE PATHS OF THIS RUN. Each path holds a
   > private home with the settings and the SSH identity of this run.

   ```sh
   rm -rf /var/tmp/hz-shared-base /var/tmp/hz-shared-a /var/tmp/hz-shared-b
   ```

   Result: The private homes and the evidence of the run are gone.

6. On the server computer, remove the public key of `fixture_key` from the
   login that accepted it. If you started the `sshd` of section 4.1, stop it
   with Ctrl-C and remove `$SSH_DIR`.

   > **CAUTION:** REMOVE ONLY THE KEY AND THE FOLDER OF THIS RUN. Another line of
   > a key file is the login of another person or of another task.

   Result: The identity of this run opens no login.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
