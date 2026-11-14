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
- This procedure does not test: a real far link with packet loss, or a real
  Firefox of a Windows host.
- The container lane of `packaging/browser-machine` is task C1 and task C2 of
  this procedure. It needs one Linux machine with Docker and with no browser of
  its own; a rented machine is such a machine. Tasks F1 to F5 need no Docker.

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

### 6.4 F3c — a lost link with two panels comes back to its own page

1. Open a second Browser panel of this computer beside the first one.

   Result: the second panel paints the Firefox of the host, and both panels show
   the same profile of that host.

2. Go to `example.com` in the first panel and to `example.org` in the second
   panel. Type `seven` into a field of the first panel.

   Result: each panel shows its own page, and the field of the first panel holds
   `seven`.

3. End the `ssh` child of the first panel, as step 2 of task F3 says.

   Result: only the first panel says that the link is lost. The second panel
   keeps its page and its picture.

4. Wait for the countdown of the first panel.

   Result: the first panel shows `example.com` again, and the field still holds
   `seven`. The second panel still shows `example.org` and is not reloaded.

5. Close the second panel, and then the first panel.

   Result: the page of each panel ends, and the browser of the host stops with
   the last page.

### 6.4b F6 — a private page of the host leaves after its bound

The session of a browser host closes a private page whose link is gone, ten
minutes after the link went, and its context goes with it. A shared page of the
same session stays.

1. Open one private panel of the host (**Private panel** of a panel menu), and
   note the time.

2. End the `ssh` child of that panel, as task F3 says.

   Result: the panel says that the link is lost.

3. CAUTION: THIS STEP RENTS NO COMPUTE, BUT IT ENDS A PAGE OF THE HOST. Close
   the panel of this computer.

4. Wait ten minutes. Read the log of the host, which is
   `~/.horizon/browser-profiles/<profile>/host-firefox.log` on that machine.

   Result: the log holds one line that the private page had no link for the
   bound, and one line that the user context is removed. A shared panel of the
   same profile that you opened in step 1 still shows its page, and a shared
   panel that you open in this step attaches to its page and not to a new one.

5. Open a private panel of the same host again, and read its page.

   Result: the new page is a new page, and it reads no cookie of the private page
   of step 1.

The bound of a session of a host is ten minutes. The driver takes
`--private-unlinked <seconds>` for a check that cannot wait for it, and the
checks of the repository use it:
`browser::host_driver::tests::session_of_links::a_private_page_with_no_link_goes_after_the_bound_and_a_shared_page_stays`,
`browser::host_driver::tests::session_of_links::the_bound_of_a_private_page_with_no_link_is_one_constant`, and
`browser::host_driver::tests::session_of_links::a_private_page_with_a_link_keeps_its_context`.

### 6.5 F3b — a browser host on a machine of the machine list

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

### 6.6 F4 — each refusal gives its words

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

### 6.7 F5 — the agent tools reach the page of the host

1. Start an agent panel of this computer, and ask it to take a snapshot of the
   browser panel and to click a link of the page.

   Result: the snapshot names the page of the host, the click changes the page,
   and the picture shows the new page.

2. Open a panel of the host, and ask the agent to call `browser_create` with
   `profile: "private"` and the address of a test page that sets a cookie. Read
   the page title and compare it with a page of the shared profile.

   Result: the private panel shows `Private`, and it reads no cookie of the
   shared profile of the host.

3. Close the private panel, and read the log of the host at
   `~/.horizon/browser-profiles/fftest/host-firefox.log`.

   Result: the log holds the line that the user context of the private panel is
   removed. Open a private panel again, and its page reads no cookie of the
   private page that you closed.

### 6.8 C1 — the container lane on a Linux machine with Docker

These tasks prove the lane that the reference
`horizon-machines/references/browser-machine.md` describes: Firefox and
`geckodriver` in a container, and the driver of this computer in that same
container. The tasks of this section need one Linux machine with Docker, no
browser of its own, and about 5 GB of free disk.

> **CAUTION:** DO NOT RUN THESE TASKS ON A MACHINE THAT A PERSON USES WITH REAL
> BROWSER SIGN-INS. The tasks install Docker, build an image and start a
> container, and the profile of the browser lives in that container.

1. Copy the four files of the reference and the binary of this computer to one
   folder of the machine, and read the two C libraries before anything is built.

   ```sh
   scp browser-machine/Dockerfile browser-machine/entrypoint.sh        browser-machine/horizon-browser-wrapper.sh browser-machine/setup.sh <target>:
   scp "$HORIZON_EXECUTABLE" <target>:horizon
   ssh <target> 'sh setup.sh --engine firefox --horizon horizon --check'
   ```

   Result: the last line starts with `setup: the driver needs glibc `, names the
   library of the binary and the library of the image, and ends with
   `the driver can start there`. The exit code is 0.

2. Run the setup of the firefox engine.

   ```sh
   ssh <target> 'sh setup.sh --profile fftest --engine firefox --horizon horizon'
   ```

   Result: the log holds the same two library versions, then the build, then one
   line `browser-machine ready profile=fftest engine=firefox command=docker exec -i -e HOME=/profile hz-bvm-fftest /opt/horizon/horizon --browser-host-driver`.
   The code is 0.

3. Prove that the image holds the browser of that driver.

   ```sh
   ssh <target> 'docker run --rm --entrypoint sh horizon-browser-machine:local -c \
     "ldd --version | head -n 1; /usr/local/bin/geckodriver --version | head -n 1; firefox-esr --version"'
   ```

   Result: the C library of step 1, a line with `geckodriver 0.37`, and a line
   with `Mozilla Firefox`. A container leaves no such program behind.

4. Write the browser host of this machine in Horizon with the request, and press
   **Use it** on the card.

   Ask for `browser_host_request` with the machine of the list as `machine`, the
   profile `fftest`, `engine` `firefox`, and the `command` of step 2 as `command`.

   Result: the card holds the rows **Browser**, **Profile**, **Machine** and
   **Driver command**, and the row **Driver command** is the whole command of
   step 2. Settings, General, Browser host shows the same command in **Driver
   program**.

5. Open a browser panel and navigate to `example.com`.

   Result: the panel paints the page of the container. The log of that machine
   holds one `docker exec` of the driver, and the container holds a `firefox-esr`
   process and a `geckodriver` process.

6. Type text into a field of a page, end the link of the panel, and wait.

   ```sh
   pkill -f 'ssh.*<target>'
   ```

   Result: the panel counts down to the next attempt and then shows the same
   page, with the text of the field. The container holds no second
   `geckodriver`.

7. Stop the container and open a panel again.

   ```sh
   ssh <target> 'docker stop hz-bvm-fftest'
   ```

   Result: the panel says that the link to the browser is lost and starts the
   driver again, and a new page opens at the same address. The sign-ins of the
   profile stay, because the profile folder is the mount of the machine.

### 6.9 C2 — the refusal of a binary that the image cannot run

1. Change the base image of the Dockerfile to one with an older C library, and
   read the two libraries again.

   ```sh
   ssh <target> 'sed -i "s/^FROM debian:forky-slim$/FROM debian:13-slim/" Dockerfile'
   ssh <target> 'sh setup.sh --engine firefox --horizon horizon --check'
   ```

   Result: three lines that name the library of the binary, the library of the
   image `debian:13-slim`, and three ways forward: the engine Chromium, a horizon
   binary of a system of that library or older, and a base image with the newer
   library. The exit code is 3, and no image was built.

2. Run the same setup without `--check`.

   Result: the same lines, the last of them `setup: the driver cannot start in the
   image of the browser machine`, the exit code 1, and no image was built.

3. Restore the base image of the Dockerfile.

   ```sh
   ssh <target> 'sed -i "s/^FROM debian:13-slim$/FROM debian:forky-slim/" Dockerfile'
   ```

   Result: the line `FROM debian:forky-slim` is back, and step 1 of task C1 ends
   with code 0 again.

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
- A private page of the host that lost its link leaves the browser of the host
  after ten minutes, with its context, and a shared page of the same session
  stays.
- The container lane reads its two C libraries before the build, builds an image
  whose C library, `geckodriver` and Firefox are named, paints a page of that
  container in a panel, comes back to the same page after a cut link, and refuses
  a binary that the image cannot run with the two versions and code 3.

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
