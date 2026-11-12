---
procedure: browser-machine
feature: A browser machine: Chromium in a container on a rented Linux machine, as the browser host of a Horizon panel
platforms: [linux]
cost: rents compute
destructive: yes
secrets: the private key and the known-hosts file of the machine key of Horizon
owner: peters
---

# Browser machine test procedure

## 1. Purpose

This procedure proves that the four files of the browser machine prepare a
rented Linux machine that has no browser, and that a Horizon browser panel drives
the Chromium of that machine over the SSH tunnel. It also proves the recovery of
a panel after the container stops.

## 2. Applicability

- Candidate: a build of `horizon` that has the `browser_host` settings entry, and
  the files in `packaging/browser-machine/`.
- Platforms: the destination is Debian or Ubuntu on Linux, x86_64 or arm64. The
  client is Linux or macOS.
- This procedure does not test: a machine that has a real Chrome or Chromium (use
  that browser instead), macOS as the destination, a site that refuses software
  graphics, and a sign-in that survives a restart of the container.

## 3. Safety

> **CAUTION:** THE PROCEDURE RENTS A MACHINE, AND THAT MACHINE COSTS MONEY UNTIL YOU DELETE IT. Rent the smallest machine that the provider offers, and do task BM-8 in the same session.

> **CAUTION:** THE PROCEDURE DELETES THE MACHINE IN TASK BM-8, WITH EVERY FILE ON IT. Use a new machine, and no machine that holds work of the person.

> **CAUTION:** SIGN IN ONLY WITH A TEST ACCOUNT. The profile folder of the machine holds every sign-in that the browser takes, and every local account of that machine can drive the browser.

## 4. Equipment and preconditions

- A provider account that the `horizon-machines` skill can rent from, with a
  machine key. Its `private_key_path` and `known_hosts_path` are the secret
  references of this procedure. Do not write the key.
- A fresh machine: Ubuntu 24.04 or Debian 13, 2 vCPU, 4 GB of memory and 3 GB of
  free disk, with no browser program and no Docker.
- A Horizon build on the client with a free workspace.
- The person who runs Horizon, with a terminal to edit their own
  `~/.ssh/config`, and the **Browser host** part of Settings → General.
- The four files of `packaging/browser-machine/` on the client.

## 5. Setup

1. Probe the destination with the machine key:

   ```sh
   ssh <target> 'uname -s; uname -m; id -un; echo $HOME'
   ssh <target> 'command -v docker || echo no-docker'
   ssh <target> 'command -v google-chrome || command -v chromium || echo no-browser'
   ssh <target> 'df -h $HOME | tail -n 1'
   ```

   Result: `Linux`, one architecture, one user, a home folder, `no-docker`,
   `no-browser`, and at least 3 GB available. Stop and use the real browser of
   that machine when a browser program answers.

2. Copy the four files to one folder on the destination:

   ```sh
   scp packaging/browser-machine/Dockerfile packaging/browser-machine/entrypoint.sh \
       packaging/browser-machine/horizon-browser-wrapper.sh packaging/browser-machine/setup.sh <target>:
   ```

   Result: no error.

3. Run the setup on the destination, as the user of the SSH login:

   ```sh
   ssh <target> 'sh setup.sh --profile work'
   ```

   Result: the output names the install of `docker.io` when Docker was missing, a
   `docker build` that ends with the tag `horizon-browser-machine:local`, the
   install of `/usr/local/bin/google-chrome`, and this last line:

   ```
   browser-machine ready profile=work port=<n>
   ```

   The command exits with code 0, in a few minutes.

4. Read the endpoint on the destination:

   ```sh
   ssh <target> 'sed -n 1p ~/.horizon/browser-profiles/work/DevToolsActivePort'
   ssh <target> 'curl -s http://127.0.0.1:<n>/json/version'
   ```

   Result: the port of step 3, then one JSON line with a `Chrome/` version and a
   `webSocketDebuggerUrl`.

5. Examine the container on the destination:

   ```sh
   ssh <target> 'docker ps --format "{{.Names}} {{.Status}} {{.Ports}}"'
   ```

   Result: one container with the name `hz-bvm-work`, the status `Up ... (healthy)`
   and the ports `127.0.0.1:<n>->9222/tcp`.

6. Tell the person to add this block to their own `~/.ssh/config`, with the
   values of the machine key:

   ```sshconfig
   Host bvm-1
       HostName <address>
       User <user of step 1>
       IdentityFile <private_key_path>
       IdentitiesOnly yes
       UserKnownHostsFile <known_hosts_path>
   ```

   Result: `ssh -T -o BatchMode=yes bvm-1 'echo ok'` on the client prints `ok`.

7. Tell the person to set the browser host in Settings (**Ctrl+Shift+,**) →
   **General** → **Browser host**: **SSH target** `bvm-1` and **Profile** `work`.
   Then start Horizon again.

   Result: the entry is in the settings store, and the start of Horizon reads it.

## 6. Tasks

### 6.1 BM-1 — a browser panel drives the Chromium of the machine

1. Hold Ctrl and double-click empty canvas space inside a workspace, then pick
   **Browser**.

   Result: a browser panel opens, its chrome shows the SSH target `bvm-1` where
   the engine menu was, and its body paints a page. The panel connects in less
   than 20 seconds.

2. Click the address bar, type `https://example.com`, and press Enter.

   Result: the panel paints that page, and its title is the title of that page.

3. Click a link, turn the wheel over the page, and press `Tab`.

   Result: the page takes the click, it scrolls, and the focus moves.

4. Drag the bottom right corner of the panel.

   Result: the page takes the new viewport and the page is not reloaded.

5. On the destination, examine the log of the wrapper:

   ```sh
   ssh <target> 'cat ~/.horizon/browser-profiles/work/chromium.log'
   ```

   Result: one line with `browser-machine: profile=work container=hz-bvm-work
   port=<n>`.

### 6.2 BM-2 — a second panel reuses the browser

1. Close the panel of task BM-1 and open a new browser panel.

   Result: the panel connects in a few seconds. `docker ps` shows no new
   container, and the container uptime did not restart.

### 6.3 BM-3 — a stopped container restarts at the next panel

1. Stop the container on the destination:

   ```sh
   ssh <target> 'docker stop hz-bvm-work'
   ```

   Result: `docker ps` shows no container.

2. Open a new browser panel on the client.

   Result: Horizon starts the browser of that machine again. The panel connects,
   and `docker ps` shows `127.0.0.1:<m>->9222/tcp` with a port `<m>` that can
   differ from `<n>`. The endpoint file holds `<m>` on line 1.

### 6.4 BM-4 — a lost browser keeps the panel, says why, and comes back

1. With a browser panel open on a page, stop the browser of the container:

   > **CAUTION:** THIS COMMAND STOPS EVERY CHROMIUM ON THE DESTINATION THAT HORIZON STARTED. Use a destination with no other browser work.

   ```sh
   ssh <target> 'pkill -f "remote-debugging-port=9223"'
   ```

   Result: the panel does not go away. It keeps its place and its last address,
   and its body states `The link is lost. The panel connects again in N s
   (attempt 1).`, where N counts down from 3. The body holds no word of the
   browser engine.

2. Read the state of the panel as an agent, with `browser_panel`.

   Result: `failure.code` is `attached_host_connection_lost`, and
   `failure.message` names the machine, the profile and the lost connection,
   then the words of the engine in parentheses, such as
   `(CDP connection lost: ...)`. `browser_list` still shows the panel.

3. Do not press a control. Wait 30 seconds.

   Result: the panel connects again by itself. Horizon starts the browser of
   that machine again, and the panel paints one new page at the address of
   step 1. The panel stays in the same place on the canvas. The line above the
   page states `The link was lost, so this page is new: text that you had typed
   into it, its scroll position and its session are gone.`

4. Type text into a field of the page, stop the browser again, and wait 30
   seconds.

   Result: the panel comes back at the same address, and the field is empty.
   The line of step 3 is there again.

### 6.5 BM-5 — a machine without the wrapper names that reason

1. Remove the wrapper on the destination:

   ```sh
   ssh <target> 'rm -f /usr/local/bin/google-chrome'
   ```

2. Open a new browser panel.

   Result: the panel states
   `this machine has no chromium program`, with the SSH target in front of that
   text, and the typed code of the agent answer is `attached_host_no_browser`.

3. Install the wrapper again:

   ```sh
   ssh <target> 'sh setup.sh --profile work'
   ```

   Result: the last line is `browser-machine ready profile=work port=<n>`.

### 6.6 BM-6 — a second setup run changes nothing

1. Run the setup again on the destination.

   ```sh
   ssh <target> 'sh setup.sh --profile work'
   ```

   Result: the output says `the image horizon-browser-machine:local exists: keep
   it`, the last line holds the same port when the container did not restart, and
   `docker ps` shows one container.

### 6.7 BM-7 — the setup refuses a machine that has a browser

1. On a machine that has a real Chromium, run the setup without `--force`.

   Result: the setup stops with `this machine needs no browser machine` and it
   installs no wrapper.

2. The same command with `--force`.

   Result: the setup continues and installs the wrapper.

### 6.8 BM-8 — clean up, and delete the machine

> **CAUTION:** THIS STEP DELETES THE CONTAINER, THE IMAGE, THE WRAPPER, THE PROFILE FOLDER AND THE MACHINE. Every sign-in of that browser is lost, and no file of that machine remains.

1. Remove what the setup made on the destination:

   ```sh
   ssh <target> 'docker rm -f hz-bvm-work'
   ssh <target> 'docker rmi horizon-browser-machine:local'
   ssh <target> 'rm -f /usr/local/bin/google-chrome'
   ssh <target> 'rm -rf ~/.horizon/browser-profiles/work'
   ```

   Result: `docker ps -a` and `docker images` print no browser machine.

2. Delete the machine with the `horizon-machines` skill, in the form of its
   delete step.

   Result: the provider reports no machine, and the overview card of the skill
   shows no row for it.

3. Close the browser panels of the run, and close Horizon.

   Result: no panel of the destination remains, and no `ssh` process of the run
   remains on the client.

## 7. Pass criteria

- The setup prepares a fresh machine with one command, and its last line names
  the profile and the port.
- A browser panel paints, navigates, takes a click, a wheel event, a key press and
  a resize through the tunnel of that machine.
- A second panel reuses the running container, and a stopped container restarts
  at the next panel on a port that the wrapper reads again.
- A lost browser keeps the panel, says why, reports
  `attached_host_connection_lost` to an agent, and comes back by itself at the
  same address in a new page that says its state is gone.
- A machine without the wrapper reports `attached_host_no_browser`.
- No step of the run leaves a machine, a container or a process.

## 8. Cleanup

Task BM-8 is the cleanup. Run it before the end of the session, also when a task
failed.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep the address, the account and the key out of the report.
