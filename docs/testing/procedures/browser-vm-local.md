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
It proves that `horizon --browser-vm-local` runs the image with two loopback
ports, that the profile host answers over SSH through the ssh entry that Horizon
writes, that a Device panel shows the desktop, and that the state volume keeps
the files of the profiles across a stop and a start.

## 2. Applicability

- Candidate: the commit under test of `packaging/browser-vm` and
  `crates/horizon-core/src/cloud_runtime/browser_vm/local.rs`.
- Platforms: Linux with a working Docker daemon.
- The continuous integration runs the unit tests of the local browser VM, which
  prove the loopback publishes, the key and the ssh entry against a fake Docker.
  This procedure runs the real container.
- This procedure does not test: a rented browser VM, the MCP tools of an agent
  against the container, and the sign-in to a real account.

## 3. Safety

> **CAUTION:** PUBLISH THE PORTS ON THE LOOPBACK ONLY.
> The VNC server of the image has no password.
> A port on another interface gives every host of the network access to the
> desktop and to the sign-ins of the profiles.
> Horizon publishes `127.0.0.1` only. Do not add a port of your own.

> **CAUTION:** DELETE ONLY THE CONTAINER AND THE VOLUME THAT THIS RUN CREATED.
> Task LCL-DOWN removes the container of the run.
> Task LCL-CLEANUP removes its volume. Another volume holds the sign-ins of
> another person.

> **CAUTION:** DO NOT SIGN IN TO A REAL ACCOUNT IN THIS RUN.
> Use the page `https://example.org/`. The state volume keeps every cookie of
> the run.

## 4. Equipment and preconditions

- A Linux computer with a working Docker daemon.
- The repository at the commit under test.
- A Horizon instance in the workspace of the person, with the tool
  `device_panel`.
- The names of this run. Use them in every step:

  ```sh
  export RUN=1780-local
  export VNC_PORT=5991
  export SSH_PORT=2291
  export VM="--name hz-bvm-$RUN --vnc-port $VNC_PORT --ssh-port $SSH_PORT"
  ```

## 5. Setup

1. Remove the container of an earlier run of this procedure.

   > **CAUTION:** REMOVE ONLY THE CONTAINER OF THIS PROCEDURE.

   ```sh
   horizon --browser-vm-local down $VM
   docker volume rm hz-bvm-$RUN-state 2>/dev/null || true
   ```

   Result: `docker ps --all` names no container `hz-bvm-$RUN`.

2. Start the container and build the image when it is absent.

   ```sh
   horizon --browser-vm-local up $VM --build "$PWD"
   ```

   Result: The command prints these lines:
   - `the container hz-bvm-$RUN runs`;
   - `the ssh target is horizon-browser-vm`;
   - `the Device panel takes 127.0.0.1:$VNC_PORT`.

3. Read the ssh entry that Horizon wrote.

   ```sh
   ssh -G horizon-browser-vm | grep -E '^(hostname|port|identityfile) '
   ```

   Result: The lines name `127.0.0.1`, the port `$SSH_PORT` and the key
   `~/.horizon/cloud/browser-vm/browser-vm-local/id_ed25519`, which is the key
   that the `browser_vm` tool of a running Horizon uses for the same container.

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
   ssh -T horizon-browser-vm 'echo "$HORIZON_BROWSER_ROOT $DISPLAY"; horizon-browser --version'
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

### 6.4 LCL-STAY: The state volume keeps the files of the profiles

1. Write a marker in the profile folder.

   ```sh
   docker exec hz-bvm-$RUN bash -c 'echo keep > /state/browser-root/browser-profiles/marker'
   ```

   Result: The command prints nothing.

2. Stop and remove the container.

   ```sh
   horizon --browser-vm-local down $VM
   ```

   Result: The command prints `the container hz-bvm-$RUN stopped; the state
   volume hz-bvm-$RUN-state keeps the sign-ins`, and `docker ps --all` names no
   container of the run.

3. Start the container again with the same names.

   ```sh
   horizon --browser-vm-local up $VM
   ```

   Result: The command prints the three lines of setup step 2, and it does not
   make a new key, because the key of the run exists.

4. Read the marker.

   ```sh
   docker exec hz-bvm-$RUN cat /state/browser-root/browser-profiles/marker
   ```

   Result: The output is `keep`.

### 6.5 LCL-ENTRY: A new volume brings a new host key, and the entry follows

1. Stop the container and remove its volume.

   > **CAUTION:** A REMOVED VOLUME DELETES EVERY PROFILE AND EVERY SIGN-IN IN
   > IT. REMOVE ONLY THE VOLUME OF THIS RUN.

   ```sh
   horizon --browser-vm-local down $VM
   docker volume rm hz-bvm-$RUN-state
   ```

   Result: The volume is absent from `docker volume ls`.

2. Start the container again.

   ```sh
   horizon --browser-vm-local up $VM
   ```

   Result: The command prints the three lines of setup step 2, so the new host
   key is trusted without a manual step, and the entry names the same target.

3. Read the ssh version of the container.

   ```sh
   ssh -T horizon-browser-vm 'horizon-browser --version'
   ```

   Result: The output is `horizon-browser <version>`.

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

### 6.7 LCL-STATUS: A status names the container, the ports and the volume

1. Ask for the status of the run.

   ```sh
   horizon --browser-vm-local status $VM
   ```

   Result: The output holds `running: yes`, `vnc: 127.0.0.1:$VNC_PORT`,
   `ssh: 127.0.0.1:$SSH_PORT` and `volume: hz-bvm-$RUN-state`.

### 6.8 LCL-DOWN: A stop closes both ports and keeps the entry

1. Stop and remove the container.

   ```sh
   horizon --browser-vm-local down $VM
   ```

   Result: The command prints the line of task LCL-STAY step 2.

2. Ask the two ports for an answer.

   ```sh
   for port in "$VNC_PORT" "$SSH_PORT"; do
     (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && echo "open $port" || echo "closed $port"
   done
   ```

   Result: The command prints `closed $VNC_PORT` and `closed $SSH_PORT`.

3. Ask the ssh target for the profile host.

   ```sh
   ssh -T horizon-browser-vm true
   ```

   Result: The command fails with `Connection refused`, and the entry stays in
   the ssh configuration for the next start.

## 7. Pass criteria

- Every task of section 6 passes.
- No port of the container is on an interface other than the loopback.
- The ssh entry names the loopback address, the port of the run and the key of
  the run, and it survives a stop.
- The marker file of task LCL-STAY survives the stop and the start.
- The screenshot of task LCL-VIEW shows the desktop of the container and the
  page `Example Domain`.

## 8. Cleanup

1. Close the Device panel of task LCL-VIEW.

   Use the tool `device_panel` with the operation `close` and its `panel_id`.

   Result: The panel is absent from the answer of the operation `list`.

2. Remove the container.

   ```sh
   horizon --browser-vm-local down $VM
   ```

   Result: The container is absent from `docker ps --all`.

3. Remove the state volume of the run.

   > **CAUTION:** A REMOVED VOLUME DELETES EVERY PROFILE AND EVERY SIGN-IN IN
   > IT. REMOVE ONLY THE VOLUME OF THIS RUN.

   ```sh
   docker volume rm hz-bvm-$RUN-state
   ```

   Result: The volume is absent from `docker volume ls`.

4. Keep the ssh entry, because Horizon writes it for the browser VM of this
   computer and the next start uses it again. Remove it only when no local
   browser VM remains:

   > **CAUTION:** REMOVES THE HOST BLOCK AND THE HOST KEYS OF THE BROWSER VM.
   > The next start writes them again.

   ```sh
   awk '/^# >>> horizon browser vm/{skip=1} !skip; /^# <<< horizon browser vm/{skip=0}' \
     ~/.ssh/config > /var/tmp/horizon-ssh-config
   chmod 0600 /var/tmp/horizon-ssh-config
   cat /var/tmp/horizon-ssh-config > ~/.ssh/config
   rm -f /var/tmp/horizon-ssh-config
   ```

   Result: `ssh -G horizon-browser-vm` names no block that Horizon wrote.

5. Remove the images of this run when no other run uses them.

   ```sh
   docker image rm horizon-browser-vm-local:latest horizon-browser-vm:nosshd
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
