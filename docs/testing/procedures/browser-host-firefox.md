---
procedure: browser-host-firefox
feature: A browser panel that uses the Firefox of a browser host
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser host with Firefox test procedure

## 1. Purpose

This procedure proves that a browser panel can use the Firefox of another
machine, that the picture and the input cross the link, that the link comes back
to the same page, and that every refusal gives its words.

## 2. Applicability

- Candidate: any build of Horizon that serves the `firefox` engine of
  `browser_host`.
- Platforms: Linux and macOS. The host is a second user account or a second
  folder on the same computer, so no rented machine is necessary.
- This procedure does not test: a real far link with packet loss, a real
  Firefox of a Windows host, or the container lane of
  `packaging/browser-machine`. The container lane needs a Linux machine with
  Docker.

## 3. Safety

> **CAUTION:** DO NOT USE THE PROFILE OF A PERSON FOR THIS TEST. A profile holds
> cookies and sign-ins. Use a profile name that you make for this test only, such
> as `fftest`.

> **CAUTION:** DELETE THE TEST PROFILE AFTER THE RUN. It holds the pages that you
> opened and the files that you downloaded. Remove its folder, not the folder of
> a profile that a person uses.

## 4. Equipment and preconditions

- One computer with Horizon, Firefox, `geckodriver` and `ssh` to itself.
- The version of `horizon` that this procedure tests, on the host path of the
  second account or the second folder.
- `ssh` to `127.0.0.1` as the test account, with no password.
- A slow link stand-in: `tc` needs root, so this procedure uses the stand-in
  program of the option below instead.

## 5. Setup

The host of this procedure is one machine that serves itself. Two ways give a
second home folder; use one of them.

**A second user account (Linux).**

1. Make the account and a key for it.

   ```sh
   sudo useradd -m -s /bin/bash fftest
   sudo -u fftest ssh-keygen -t ed25519 -N '' -f /home/fftest/.ssh/id_ed25519
   sudo -u fftest sh -c 'cat ~/.ssh/id_ed25519.pub >> ~/.ssh/authorized_keys'
   sudo -u fftest ssh -o StrictHostKeyChecking=accept-new 127.0.0.1 true
   ```

   Result: the last command ends with no output and exit code 0.

2. Copy the binary of this computer into the home folder of that account. The
   variable `HORIZON_EXECUTABLE` of an agent panel is that binary; do not read a
   `horizon` on `PATH`.

   ```sh
   cp "$HORIZON_EXECUTABLE" /tmp/horizon-fftest
   install -m 0755 /tmp/horizon-fftest /home/fftest/horizon
   sudo -u fftest /home/fftest/horizon --version
   ```

   Result: the last command prints the same version as the command
   `"$HORIZON_EXECUTABLE" --version` on this computer.

**A second home folder (Linux or macOS).**

1. Make a folder that stands in for the home folder of the host.

   ```sh
   mkdir -p /tmp/ffhost-home
   ```

   Result: the folder exists and its mode is 0700 or stricter.

2. Prove that the driver starts in that folder.

   ```sh
   cp "$HORIZON_EXECUTABLE" /tmp/ffhost-home/horizon
   chmod 755 /tmp/ffhost-home/horizon
   ssh 127.0.0.1 'HOME=/tmp/ffhost-home /tmp/ffhost-home/horizon --browser-host-driver --profile fftest --engine firefox'
   ```

   Result: the command prints one line that starts with `{"type":"refusal"` and
   names the greeting of the link, because no hello arrived. Press `Ctrl+C`.

**The fixture of this procedure.** A fixture run writes the entry of the host in
its private settings, so no hand edit of a settings store is necessary.

1. Start the fixture with `--browser-host` and name the engine of that host with
   `--browser-host-engine firefox`; without that option the fixture follows the
   Chromium of the host.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$HORIZON_EXECUTABLE" \
     --native-view --state /tmp/ff-fixture --browser-host fftest@127.0.0.1 \
     --browser-host-profile fftest --browser-host-engine firefox
   ```

   Result: the entry of the fixture names the engine `firefox`, and the panel
   follows the driver of that host.

2. Name the program of the driver with `--browser-host-command` when the
   non-interactive `ssh` of the host finds no `horizon` on its `PATH`, such as
   `/home/fftest/horizon --browser-host-driver`; without that option the entry
   leaves the command to its default.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon "$HORIZON_EXECUTABLE" \
     --native-view --state /tmp/ff-fixture --browser-host fftest@127.0.0.1 \
     --browser-host-profile fftest --browser-host-engine firefox \
     --browser-host-command '/home/fftest/horizon --browser-host-driver'
   ```

   Result: the entry of the fixture holds that command, and the driver of the
   host starts from it.

3. Keep the browser of the host after the panel closes. A session of that
   browser lives until the panel closes it, until the person removes the browser
   or until the machine of the host restarts. To stop it after a bound with no
   link, add `--idle-stop <minutes>` to the command of the entry, as in
   `/home/fftest/horizon --browser-host-driver --idle-stop 120`; without that
   option the browser of the host never stops by itself.

   Result: the session of the host keeps its page with no panel, and it stops
   after the bound of its entry.

## 6. Tasks

### 6.1 F1 — the panel shows the Firefox of the host

1. Start Horizon of this computer.

   Result: the window opens.

2. Open **Settings** (**Ctrl+Shift+,**) → **General** → **Browser host**.

   Result: the section shows **SSH target**, **Profile**, **Engine** and
   **Driver program**.

3. Enter `fftest@127.0.0.1` as **SSH target**, `fftest` as **Profile**, choose
   **Firefox, with Horizon on that machine**, and write `/home/fftest/horizon
   --browser-host-driver` as **Driver program**. Start Horizon again.

   Result: Horizon opens, and the entry holds those four values.

4. Open a Browser panel and navigate to `example.com`.

   Result: the panel paints the page. The picture is a JPEG of the page of that
   machine, and the panel shows the address and the title.

### 6.2 F2 — input reaches the page of the host

1. Click a text field of the page and type `horizon`.

   Result: the field shows `horizon`, one message of the link carries the run of
   text, and the picture shows the text.

2. Scroll one screen with the wheel.

   Result: the page of the panel scrolls, and the picture follows it.

### 6.3 F3 — a cut link comes back to the same page

1. Type text into a form field, and do not send the form.

   Result: the text stands in the field.

2. End the `ssh` child of that panel without closing the panel.

   ```sh
   pkill -f 'browser-host-driver --profile fftest' ; pkill -f 'ssh.*fftest@127.0.0.1'
   ```

   Result: the panel says that the link is lost and counts down to the next
   attempt.

3. Wait for the countdown.

   Result: the panel attaches again, the picture shows the same page, and the
   text that you typed is still in the field. The panel shows no notice that the
   page state is gone, and the log names the reason of the lost link such as the
   status of `ssh` or its words; the log holds no empty reason.

4. Read the latency lines of one press. Start Horizon with
   `HORIZON_BROWSER_LATENCY=1`, press a key, and read the log.

   Result: one `latency frame` line and one `latency action` line of that press,
   with `received_us` larger than `written_us`.

### 6.4 F3b — a browser host on a machine of the machine list

A machine of the list answers to the machine key of this computer and to no key
of the person, so this lane proves that a browser panel of such a machine opens
with that key.

1. Put a machine in the list with an agent: `machine_set` with its name, its
   address and its user, and `machine_key` with the same name.

   Result: `machines` reports the machine, and the machine list of the sidebar
   shows its row.

2. Ask for `browser_host_request` with `machine` set to the name of that machine,
   `profile` `work` and `engine` `firefox`. Press **Use it** on the card, and
   start Horizon again.

   Result: the card names the row **Machine**, the key and the profile, and
   Settings → General → Browser host shows the same machine. The section shows no
   **SSH target**.

3. Open a Browser panel.

   Result: the panel paints the Firefox of that machine through the driver of
   Horizon there, and the log holds `ssh` arguments with `-i` and the key file of
   that machine, and no key of this computer.

4. Move the key of the machine out of the key folder of this Horizon home, and
   open a panel again.

   Result: the panel names the SSH target of the machine and keeps the words of
   `ssh`, which say that the identity file is not accessible and that the login
   was refused. The panel starts no shell and asks no password.

### 6.5 F4 — each refusal gives its words

1. Write a target of a host that has no `horizon`, such as
   `fftest@127.0.0.1` with **Driver program** set to `/home/fftest/missing`, and
   open a panel.

   Result: the panel says that the host runs no horizon for its browser, and it
   says to copy the binary of this computer.

2. Copy a different version of `horizon` into the home folder of the host and
   open a panel again.

   Result: the panel says that the driver of the host is another Horizon and
   speaks another version of the link.

3. Remove `geckodriver` from the path of the host account, and open a panel.

   Result: the panel gives the words of the engine: the browser did not start,
   and `geckodriver` is named.

### 6.6 F5 — the agent tools reach the page of the host

1. Start an agent panel of this computer, and ask it to take a snapshot of the
   browser panel and to click a link of the page.

   Result: the snapshot names the page of the host, the click changes the page,
   and the picture shows the new page.

## 7. Pass criteria

- The panel paints the page of the host, and the input of the person reaches it.
- A cut link attaches again to the same page, and the text of a form field
  survives it. The log of that loss names its reason.
- The browser of the host stays after the panel is closed, and it stops only at
  the idle bound of the entry or at `close`.
- Each refusal names what is missing on the host, in one sentence, and offers
  **Retry**.
- The latency lines of a press exist and are in order.
- A browser host on a machine of the machine list opens a panel with the machine
  key of this computer, and no key of the person.

## 8. Cleanup

1. Close the browser panel and remove the entry of the settings.

   Result: the Browser host section is empty, and a new page opens on this
   computer.

2. Remove the test profile and the second home folder.

   ```sh
   rm -rf /tmp/ffhost-home
   sudo -u fftest rm -rf /home/fftest/.horizon
   sudo userdel -r fftest
   ```

   Result: no folder of the test profile remains.

## 9. Record of results

Put the results in the pull request, with the version of Horizon and the version
of Firefox and `geckodriver` of the host. Keep private evidence out of the
repository.
