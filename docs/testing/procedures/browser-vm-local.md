---
procedure: browser-vm-local
feature: The browser image of packaging/browser-vm on the loopback of one computer
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Local browser VM test procedure

## 1. Purpose

This procedure tests the local mode of the browser image.
It proves that `scripts/browser-vm-local.sh` runs the image with two loopback
ports, that the profile host answers over SSH, that a Device panel shows the
desktop, and that the state volume keeps the files of the profiles across a
stop and a start.

## 2. Applicability

- Candidate: the commit under test of `packaging/browser-vm` and
  `scripts/browser-vm-local.sh`.
- Platforms: Linux with a working Docker daemon.
- The continuous integration runs `scripts/test_browser_vm_local.py`, which
  proves the ports and the alias without a Docker daemon. This procedure runs
  the real containers.
- This procedure does not test: a rented browser VM, the MCP tools of an agent
  against the container, and the sign-in to a real account.

## 3. Safety

> **CAUTION:** PUBLISH THE PORTS ON THE LOOPBACK ONLY.
> The VNC server of the image has no password.
> A port on another interface gives every host of the network access to the
> desktop and to the sign-ins of the profiles.
> The script publishes `127.0.0.1` only. Do not add a port of your own.

> **CAUTION:** DELETE ONLY THE CONTAINER AND THE VOLUME THAT THIS RUN CREATED.
> The task LCL-DOWN removes the container of the run.
> Task LCL-PURGE removes its volume. Another volume holds the sign-ins of
> another person.

> **CAUTION:** DO NOT SIGN IN TO A REAL ACCOUNT IN THIS RUN.
> Use the page `https://example.org/`. The state volume keeps every cookie of
> the run.

## 4. Equipment and preconditions

- A Linux computer with a working Docker daemon and the command `ssh`.
- The repository at the commit under test.
- Docker is absent from no step: every task needs it.
- The names of this run. Use them in every step:

  ```sh
  export RUN=1780-local
  export VNC_PORT=5991
  export SSH_PORT=2291
  export STATE=/var/tmp/hz-bvm-$RUN
  ```

- A Horizon instance in the workspace of the person, with the tool
  `device_panel`.

## 5. Setup

1. Remove a container and a state directory of an earlier run of this
   procedure.

   > **CAUTION:** REMOVE ONLY THE NAMES OF THIS PROCEDURE.

   ```sh
   docker rm --force hz-bvm-$RUN 2>/dev/null || true
   rm -rf "$STATE"
   ```

   Result: No container and no state directory of the earlier run exists.

2. Start the container.

   ```sh
   scripts/browser-vm-local.sh up \
     --name hz-bvm-$RUN --image horizon-browser-vm-local:test \
     --vnc-port "$VNC_PORT" --ssh-port "$SSH_PORT" --state "$STATE"
   ```

   Result: The log holds these lines:
   - `wrote the key to $STATE/ssh/id_ed25519`;
   - `the container hz-bvm-$RUN runs`;
   - `the SSH server accepts the key on 127.0.0.1:$SSH_PORT`;
   - `the VNC server greets on 127.0.0.1:$VNC_PORT`.

   The command also prints the block of `~/.ssh/config`.

   The script read the host key of the container through Docker and wrote the
   entry `[127.0.0.1]:$SSH_PORT` in `$STATE/ssh/known_hosts-hz-bvm-$RUN`.

3. Add the block to `~/.ssh/config`.

   ```sh
   scripts/browser-vm-local.sh alias --name hz-bvm-$RUN \
     --ssh-port "$SSH_PORT" --state "$STATE" >> ~/.ssh/config
   ```

   Result: The command `ssh -G hz-bvm-$RUN` names the port `$SSH_PORT`, the key
   in `$STATE/ssh/id_ed25519` and the host file
   `$STATE/ssh/known_hosts-hz-bvm-$RUN`.

## 6. Tasks

### 6.1 LCL-PORTS: Both ports are on the loopback only

1. Read the published ports.

   ```sh
   docker port hz-bvm-$RUN
   ```

   Result: The output holds two lines, and each line starts with `127.0.0.1:`:
   - `2222/tcp -> 127.0.0.1:$SSH_PORT`;
   - `5900/tcp -> 127.0.0.1:$VNC_PORT`.

2. Search the network of the computer for the VNC port.

   ```sh
   ss --listening --tcp --numeric | grep -c "0.0.0.0:$VNC_PORT" || true
   ```

   Result: The command prints `0`.

### 6.2 LCL-SSH: The profile host answers over SSH

1. Ask the container for the version of the profile host.

   ```sh
   ssh -T hz-bvm-$RUN 'echo "$HORIZON_BROWSER_ROOT $DISPLAY"; horizon-browser --version'
   ```

   Result: The output is two lines: `/state/browser-root :99` and
   `horizon-browser <version>`.

2. Read the profile folder of the state volume.

   ```sh
   docker exec hz-bvm-$RUN ls -la /state/browser-root
   ```

   Result: The directory holds `profile-host.json` and `browser-profiles`.

### 6.3 LCL-VIEW: A Device panel shows the desktop

1. Create a Device panel with the loopback endpoint.

   Use the tool `device_panel` with the operation `create` and the endpoint
   `127.0.0.1:$VNC_PORT`.

   Result: The answer names a `panel_id` and `connection` becomes `connected`.

2. Inspect the panel.

   Result: `image_received` is true, `server.desktop_size` is `[1920, 1080]`,
   and `server.name` ends with `:99`.

3. Capture a screenshot of the panel.

   Result: The image shows the desktop of the container.

4. Open a page in the display of the container.

   ```sh
   docker exec --detach --env DISPLAY=:99 hz-bvm-$RUN \
     chromium --no-sandbox --disable-dev-shm-usage https://example.org/
   ```

   Result: After about 10 s, a screenshot of the panel shows the page
   `Example Domain`.

### 6.4 LCL-PIN: A removed volume brings a new host key

This task proves that a start does not fail on the host key of a volume that is
gone.

1. Remove the container and the state volume of this run.

   > **CAUTION:** A REMOVED VOLUME DELETES EVERY PROFILE AND EVERY SIGN-IN IN
   > IT. REMOVE ONLY THE VOLUME OF THIS RUN.

   ```sh
   scripts/browser-vm-local.sh down --name hz-bvm-$RUN
   docker volume rm hz-bvm-$RUN-state
   ```

   Result: The volume is absent from `docker volume ls`.

2. Start the container again with the same names and ports.

   Repeat setup step 2.

   Result: The log holds `the SSH server accepts the key on 127.0.0.1:$SSH_PORT`
   and `the VNC server greets on 127.0.0.1:$VNC_PORT`.

3. Count the entries of the run address in the host file.

   ```sh
   grep -c "^\[127.0.0.1\]:$SSH_PORT " "$STATE/ssh/known_hosts-hz-bvm-$RUN"
   ```

   Result: The command prints `1`.

### 6.5 LCL-STAY: The state volume keeps the files of the profiles

1. Write a marker in the profile folder.

   ```sh
   docker exec hz-bvm-$RUN bash -c 'echo keep > /state/browser-root/browser-profiles/marker'
   ```

   Result: The command prints nothing.

2. Stop and remove the container.

   ```sh
   scripts/browser-vm-local.sh down --name hz-bvm-$RUN
   ```

   Result: The log holds `the container hz-bvm-$RUN stopped and removed`, and
   `the state volume hz-bvm-$RUN-state stays`.

3. Start the container again with the same names.

   ```sh
   scripts/browser-vm-local.sh up \
     --name hz-bvm-$RUN --image horizon-browser-vm-local:test \
     --vnc-port "$VNC_PORT" --ssh-port "$SSH_PORT" --state "$STATE"
   ```

   Result: The log holds the four lines of setup step 2. The key line is
   absent, because the key exists.

4. Read the marker.

   ```sh
   docker exec hz-bvm-$RUN cat /state/browser-root/browser-profiles/marker
   ```

   Result: The output is `keep`.

### 6.6 LCL-NO-SSHD: The published image has no SSH server

This task proves that a rented machine opens no SSH port.

1. Build the default target of the image.

   ```sh
   docker build -f packaging/browser-vm/Dockerfile -t horizon-browser-vm:nosshd .
   ```

   Result: The build ends with success.

2. Start it with the variable of the local mode.

   ```sh
   docker run --rm --env HORIZON_BROWSER_VM_SSH=1 \
     --env HORIZON_BROWSER_VM_SSH_PUBKEY=ssh-ed25519-a-key \
     horizon-browser-vm:nosshd
   ```

   Result: The container stops with the status 1, and the log holds
   `this image has no SSH server; build the target browser-vm-local for a local
   container`.

### 6.7 LCL-DOWN: A stop closes both ports

1. Stop and remove the container.

   ```sh
   scripts/browser-vm-local.sh down --name hz-bvm-$RUN
   ```

   Result: The log holds the two lines of task LCL-STAY step 2.

2. Ask the two ports for an answer.

   ```sh
   for port in "$VNC_PORT" "$SSH_PORT"; do
     (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && echo "open $port" || echo "closed $port"
   done
   ```

   Result: The command prints `closed $VNC_PORT` and `closed $SSH_PORT`.

3. Ask the SSH target for the profile host.

   ```sh
   ssh -T hz-bvm-$RUN true
   ```

   Result: The command fails with `Connection refused`.

## 7. Pass criteria

- Every task of section 6 passes.
- No port of the container is on an interface other than the loopback.
- The marker file of task LCL-STAY survives the stop and the start.
- The screenshot of task LCL-VIEW shows the desktop of the container and the
  page `Example Domain`.

## 8. Cleanup

1. Close the Device panel of task LCL-VIEW.

   Use the tool `device_panel` with the operation `close` and its `panel_id`.

   Result: The panel is absent from the answer of the operation `list`.

2. Remove the container.

   ```sh
   scripts/browser-vm-local.sh down --name hz-bvm-$RUN
   ```

   Result: The container is absent from `docker ps --all`.

3. Remove the block of this run from `~/.ssh/config`, and the key, the
   known-host file of the run name and the state directory.

   > **CAUTION:** REMOVE ONLY THE BLOCK AND THE FILES OF THIS RUN.

   ```sh
   awk -v begin="# >>> horizon-browser-vm-local:hz-bvm-$RUN >>>" \
       -v end="# <<< horizon-browser-vm-local:hz-bvm-$RUN <<<" '
     $0 == begin { skip = 1; next }
     $0 == end { skip = 0; next }
     !skip { print }
   ' ~/.ssh/config > /var/tmp/hz-bvm-config && chmod 0600 /var/tmp/hz-bvm-config
   cat /var/tmp/hz-bvm-config > ~/.ssh/config
   rm -f /var/tmp/hz-bvm-config
   rm -rf "$STATE"
   ```

   Result: `ssh -G hz-bvm-$RUN` names no host of this procedure, and the
   blocks of other names stay in `~/.ssh/config`.

4. Remove the state volume.

   > **CAUTION:** A REMOVED VOLUME DELETES EVERY PROFILE AND EVERY SIGN-IN IN
   > IT. REMOVE ONLY THE VOLUME OF THIS RUN.

   ```sh
   docker volume rm hz-bvm-$RUN-state
   ```

   Result: The volume is absent from `docker volume ls`.

5. Remove the images of this run when no other run uses them.

   ```sh
   docker image rm horizon-browser-vm-local:test horizon-browser-vm:nosshd
   ```

   Result: The command prints `Untagged` for each image.

## 9. Record of results

Record the run in a report under `docs/testing/reports/` from
[the report template](TEMPLATE.md). Keep these values:
- the commit under test;
- the hash of the image (`docker image inspect --format '{{.Id}}'`);
- the two lines of `docker port`;
- the screenshot of task LCL-VIEW;
- the version of the profile host of task LCL-SSH.
