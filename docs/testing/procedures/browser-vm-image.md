---
procedure: browser-vm-image
feature: Browser image with a display, a VNC server and the profile host
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser image test procedure

## 1. Purpose

This procedure tests the image `packaging/browser-vm`.
It proves that the image starts a display, serves VNC on a loopback port,
starts the profile host, opens a page in a browser profile and keeps the
sign-in of that profile after a restart of the container.

## 2. Applicability

- Candidate: the image from the commit under test of `packaging/browser-vm`.
- Platforms: Linux with a working Docker daemon.
- This procedure does not test: a rented machine, the Horizon panel backend,
  and the MCP entry of an agent. Later pull requests of issue 1780 test those.

## 3. Safety

> **CAUTION:** PUBLISH THE VNC PORT ON THE LOOPBACK OF THE DOCKER HOST ONLY.
> The VNC server has no password. A published port on another interface gives
> every host of the network access to the desktop and to the sign-ins.

> **CAUTION:** DO NOT SIGN IN TO A REAL ACCOUNT IN THIS RUN.
> Use a test site. The state volume keeps every cookie of the run.

> **CAUTION:** REMOVE ONLY THE CONTAINER AND THE VOLUME THAT THIS PROCEDURE
> CREATED. Another container or volume holds the work of another person.

## 4. Equipment and preconditions

- A Linux computer with a working Docker daemon.
- The repository at the commit under test.
- A free TCP port on the loopback for VNC, for example `5995`.
- A Horizon instance with a Device panel that you can create.
- An SSH server on the Docker host, for the Device panel.

## 5. Setup

1. Define the names of the run.

   ```sh
   export RUN=1780-image
   export VNC_PORT=5995
   ```

   Result: The shell holds the two values.

2. Build the image from the root of the repository.

   ```sh
   docker build -f packaging/browser-vm/Dockerfile -t horizon-browser-vm:$RUN .
   ```

   Result: The build ends with `naming to docker.io/library/horizon-browser-vm:1780-image`.

3. Start the container with a named volume and the loopback port.

   ```sh
   docker run --detach --name hz-bvm-$RUN \
     --volume hz-bvm-$RUN-state:/state \
     --publish 127.0.0.1:$VNC_PORT:5900 \
     --shm-size 1g \
     horizon-browser-vm:$RUN
   ```

   Result: The command prints the ID of the container.

## 6. Tasks

### 6.1 BVM-START: The container starts the display and the profile host

1. Examine the log of the container.

   ```sh
   docker logs hz-bvm-$RUN
   ```

   Result: The log shows these lines, in this order:
   - `the display :99 runs 1920x1080x24`;
   - `the session bus runs as <PID>`;
   - `the VNC server listens on the container port 5900`;
   - `the profile host runs as <PID>, the browser runtime root is /state/browser-root`.

### 6.2 BVM-RECORD: The profile host writes its record

1. Examine the files of the state volume.

   ```sh
   docker exec hz-bvm-$RUN ls -la /state /state/browser-root
   ```

   Result: The directory holds these entries:
   - `/state/host.yaml` with the mode `-rw-------`;
   - `/state/browser-root/profile-host.json` with the mode `-rw-------`;
   - `/state/browser-root/browser-profiles` with the mode `drwx------`.

2. Read the record.

   ```sh
   docker exec hz-bvm-$RUN cat /state/browser-root/profile-host.json
   ```

   Result: The file holds `host_instance`, `pid` and `started_at`.
   Keep the value of `host_instance` for task BVM-STAY.

### 6.3 BVM-VNC: The VNC port answers on the loopback

1. Ask the published port for the version of the protocol.

   ```sh
   timeout 5 bash -c "exec 3<>/dev/tcp/127.0.0.1/$VNC_PORT; head -c 12 <&3"
   ```

   Result: The command prints `RFB 003.008`.

2. Examine that the published port is the only port of the image.

   ```sh
   docker port hz-bvm-$RUN
   ```

   Result: The output holds the one line `5900/tcp -> 127.0.0.1:5995`.

### 6.4 BVM-VIEW: A Device panel shows the desktop

1. In Horizon, create a Device panel with the SSH route to the Docker host and
   the VNC target `127.0.0.1:5995`.

   Result: The Device panel shows a black desktop of 1920 by 1080 pixels.

2. Examine the connection of the Device panel.

   Result: The panel reports a connection, a received image, a displayed image
   and a frame sequence that increases.

### 6.5 BVM-PAGE: A browser profile opens a page

1. Read the host instance.

   ```sh
   HOST_INSTANCE=$(docker exec hz-bvm-$RUN \
     sed -n 's/.*"host_instance": "\(.*\)",/\1/p' /state/browser-root/profile-host.json)
   ```

   Result: The shell holds the host instance.

2. Start a connect client with the profile `probe` of the host file.

   ```sh
   docker exec -i hz-bvm-$RUN env \
     HORIZON_BROWSER_ROOT=/state/browser-root \
     HORIZON_BROWSER_ACTOR=horizon:image-run \
     HORIZON_BROWSER_HOST_INSTANCE=$HOST_INSTANCE \
     horizon-browser mcp --connect --inline-screenshots
   ```

   Result: The client answers the MCP messages of your client.

3. Call `browser_create` with the target `probe` and the URL
   `https://example.org`.

   Result: The answer holds a panel with the ID `profile-probe-` and 12
   hexadecimal digits, the backend `chromium` and `visible` set to true.

4. Look at the Device panel of task BVM-VIEW.

   Result: A Chromium window shows the page `Example Domain`.

### 6.6 BVM-STAY: The sign-in stays after a restart

1. Call `browser_evaluate` on the panel with the expression
   `document.cookie = "run=1780; max-age=86400; path=/"; document.cookie`.

   Result: The answer holds `run=1780`.

2. Restart the container.

   ```sh
   docker restart hz-bvm-$RUN
   ```

   Result: The log shows `stop the profile host`, `the browser machine stopped`
   and the start lines of task BVM-START.

3. Read the record again.

   ```sh
   docker exec hz-bvm-$RUN cat /state/browser-root/profile-host.json
   ```

   Result: `host_instance` has the same value as in task BVM-RECORD.

4. Call `browser_create` with the target `probe` and the URL
   `https://example.org`.
   Then call `browser_evaluate` on the new panel with the expression
   `document.cookie`.

   Result: The answer holds `run=1780`. The sign-in of the profile stays.

### 6.7 BVM-STOP: The container stops cleanly and reports a dead host

1. Stop the container.

   ```sh
   docker stop hz-bvm-$RUN
   docker inspect -f '{{.State.ExitCode}}' hz-bvm-$RUN
   ```

   Result: The log ends with `the browser machine stopped`.
   The exit code is `0`.

2. Start the container again and kill the profile host.

   ```sh
   docker start hz-bvm-$RUN
   sleep 8
   HOST_PID=$(docker exec hz-bvm-$RUN sed -n 's/.*"pid": \([0-9]*\),/\1/p' /state/browser-root/profile-host.json)
   docker exec hz-bvm-$RUN kill -9 $HOST_PID
   ```

   Result: The container stops.

3. Examine the exit code.

   ```sh
   docker inspect -f '{{.State.ExitCode}}' hz-bvm-$RUN
   ```

   Result: The exit code is not `0`, and the log holds
   `the profile host stopped with the status <N>`.

## 7. Pass criteria

- The container starts the display, the VNC server and the profile host.
- The profile host writes `profile-host.json` in the state volume.
- The published VNC port answers with the protocol version, and it is the only
  port.
- A Device panel shows the desktop of the image.
- A page of the profile `probe` opens in a Chromium window.
- A cookie of the profile stays after a restart of the container, and the host
  instance does not change.
- The container stops with the code 0 after a termination signal, and with a
  code other than 0 when the profile host dies.

## 8. Cleanup

1. Stop and remove the container, and remove the state volume.

   ```sh
   docker rm -f hz-bvm-$RUN
   docker volume rm hz-bvm-$RUN-state
   ```

   Result: The command `docker volume ls` shows no volume with the name of the
   run.

2. Close the Device panel of task BVM-VIEW.

   Result: The panel leaves the board. The other panels of the board stay.

3. Remove the image of the run.

   ```sh
   docker image rm horizon-browser-vm:$RUN
   ```

   Result: The command `docker image ls` shows no image with the name of the
   run.

## 9. Record of results

Put the results in the pull request that changes the image.
Keep the screenshots of the Device panel and the log files out of the
repository.
