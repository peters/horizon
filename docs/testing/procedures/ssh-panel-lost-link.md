---
procedure: ssh-panel-lost-link
feature: Plain SSH panel with a lost link
platforms: [linux, darwin]
cost: none
destructive: no
secrets: none
owner: peters
---

# Plain SSH panel with a lost link test procedure

## 1. Purpose

This procedure tests a plain SSH panel whose link is lost. It tests the line
that the panel shows, the last screen that it keeps, the text that reaches no
shell, the button `Reconnect`, and the rule that no timer starts that panel
again by itself.

## 2. Applicability

- Use a current Horizon candidate on Linux or macOS.
- Use one SSH server on the loopback address of this computer.
- This procedure does not test a terminal of a machine, which reconnects by
  itself, and it does not test an upload, a browser panel or a Device panel.

## 3. Safety

> **CAUTION: USE ONLY THE LOOPBACK SSH SERVER OF THIS RUN.** The server of this
> procedure listens on `127.0.0.1` and starts one container. Do not point the
> test at an account or a computer that you need.

> **CAUTION: ADD ONLY THE KEY OF THE TEST HOST TO `~/.ssh/known_hosts`.** A wrong
> entry can make other hosts unreachable.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- Docker and `ssh-keygen`.
- One free loopback port for the server, for example `2222`.
- One state folder of this run, for example `$HOME/horizon-ssh-lost-link`.
- No `known_hosts` entry for `[127.0.0.1]:2222`.

## 5. Setup

1. Make the folder of the keys and the host key.

   ```sh
   export STATE="$HOME/horizon-ssh-lost-link"
   mkdir -p "$STATE/home"
   chmod 700 "$STATE/home"
   ssh-keygen -q -t ed25519 -N '' -C ssh-link-host -f "$STATE/home/host_key"
   ssh-keygen -q -t ed25519 -N '' -C ssh-link-client -f "$STATE/home/client_key"
   ```

   Result: The folder holds four key files.

2. Start the server on the loopback address, with one user of the test.

   ```sh
   docker run -d --name ssh-link --network host \
     -v "$STATE/home":/keys:ro alpine:3 sh -c 'apk add --no-cache openssh-server >/dev/null && \
       cp /keys/host_key /etc/ssh/ssh_host_ed25519_key && chmod 600 /etc/ssh/ssh_host_ed25519_key && \
       adduser -D -s /bin/sh lab && mkdir -p /home/lab/.ssh && \
       cp /keys/client_key.pub /home/lab/.ssh/authorized_keys && \
       chmod 700 /home/lab/.ssh && chmod 600 /home/lab/.ssh/authorized_keys && \
       chown -R lab /home/lab/.ssh && mkdir -p /run/sshd && \
       exec /usr/sbin/sshd -D -e -p 2222 -o ListenAddress=127.0.0.1'
   ```

   Result: `docker logs ssh-link` holds `Server listening on 127.0.0.1 port 2222`.

3. Trust the key of the test host out of band.

   ```sh
   printf '[127.0.0.1]:2222 %s\n' "$(cut -d' ' -f1,2 "$STATE/home/host_key.pub")" >> ~/.ssh/known_hosts
   ssh-keygen -F '[127.0.0.1]:2222'
   ```

   Result: The second command prints the entry and its fingerprint.

4. Add the host to your SSH configuration, so Horizon starts one plain command.

   ```sshconfig
   Host lab-1
       HostName 127.0.0.1
       Port 2222
       User lab
       IdentityFile ~/horizon-ssh-lost-link/home/client_key
       IdentityAgent none
   ```

   Result: `ssh -T -o BatchMode=yes lab-1 'echo ok'` prints `ok`.

5. Start the candidate with a private home of this run, open **Menu**, then
   **Remote Hosts**, and open a panel of `lab-1`. On Linux, start the candidate
   through
   [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
   and watch the window in a Device panel.

   Result: The panel reaches a shell prompt.

## 6. Tasks

### 6.1 SSH-LOST-01 — The panel has a shell

1. Type the command `echo one` and press Enter.

   Result: The panel prints `one` under the prompt.

2. Cut the link: stop the server of the test.

   ```sh
   docker stop ssh-link
   ```

   Result: The container stops.

### 6.2 SSH-LOST-02 — The panel says that the link is lost

1. Read the panel within 30 seconds.

   Result: A strip at the bottom of the grid says
   `The link to lab-1 is lost. Text typed here is not sent.` The grid keeps the
   last screen, prompt and `one` included. The badge of the panel says
   `Disconnected`.

2. Read the right end of the strip.

   Result: It holds the button `Reconnect`.

3. Wait 60 seconds, and read the strip again.

   Result: The line still says `is lost`, and it does not say
   `Horizon tries lab-1 again`. No attempt starts, because no timer reaches a
   plain SSH panel.

### 6.3 SSH-LOST-03 — Typed text reaches no shell

1. Type `echo two` and press Enter.

   Result: The line stays, and the grid does not print `two`.

2. Start the server again.

   ```sh
   docker start ssh-link
   ```

   Result: `docker logs ssh-link` holds `Server listening on 127.0.0.1 port 2222`
   again.

3. Wait 10 seconds, and read the panel.

   Result: The line still says `is lost`. The panel does not connect by itself.

### 6.4 SSH-LOST-04 — Reconnect starts one attempt

1. Press the button `Reconnect`.

   Result: The strip goes, and the panel reaches a shell prompt again.

2. Type `echo three` and press Enter.

   Result: The panel prints `three`.

3. Read the panel's grid above the new prompt.

   Result: The new shell holds no line with `echo two`. No program ran the text
   of task SSH-LOST-03.

### 6.5 SSH-LOST-05 — A lost link keeps its panel

1. Stop the server again, and read the panel.

   Result: The line `The link to lab-1 is lost. Text typed here is not sent.`
   appears again, and the panel holds its last screen.

2. Close Horizon normally, and start it again with the same private home.

   Result: The panel is there, and it holds the same line. No attempt starts by
   itself.

## 7. Pass criteria

- The line of a lost link names the host and says that typed text is not sent.
- The panel keeps its last screen, and no text of the test reaches a shell.
- `Reconnect` starts one attempt and the panel reaches a shell again.
- No timer starts a plain SSH panel again by itself.
- The panel and its line survive a normal start and stop of Horizon.

## 8. Cleanup

1. Remove the container of this run.

   ```sh
   docker rm -f ssh-link
   ```

   Result: The container is gone.

2. Remove the entry of the test host from `known_hosts`.

   ```sh
   ssh-keygen -R '[127.0.0.1]:2222'
   ```

   Result: The command removes the entry and writes a backup file beside it.

3. Remove the folder of this run and the private Horizon home of the candidate.

   Result: No folder of this run is left.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
