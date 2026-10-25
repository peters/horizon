---
procedure: browser-vm-egress
feature: Home egress for a browser profile of the browser image
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser image egress test procedure

## 1. Purpose

This procedure tests the home egress of a browser profile in the image
`packaging/browser-vm`.
The egress is the path that the traffic of the pages takes to the internet.
The procedure proves that the pages of an egress profile leave through the
SOCKS5 proxy of the home machine.
It proves that a profile without a proxy keeps the address of the browser
machine.
It records what the tools and the person see when the proxy is not available.

## 2. Applicability

- Candidate: the image of `packaging/browser-vm` from the commit under test.
- Platforms: Linux on the Docker host and Linux on the home machine. Both use
  OpenSSH. The home machine is also on Tailscale.
- This procedure does not test: a rented machine, the bridge `--remote`, and the
  Horizon panel backend. Later pull requests of issue 1780 test those.
- This procedure does not test a typed refusal for a failed egress. The image
  that this procedure tested answers a page that cannot reach the proxy with
  `state: timed_out` and no reason. Issue #1841 tracks that refusal.

## 3. Safety

> **CAUTION:** THE SOCKS5 LISTENER HAS NO PASSWORD. Anyone who reaches its
> address uses the network of the home machine. Name the tailnet address, as
> task EG-LISTEN shows. Do not use `0.0.0.0` on a machine with a public
> address.

> **CAUTION:** THE SOCKS5 PROXY REACHES EVERY HOST THAT THE HOME MACHINE
> REACHES. Keep the tailnet access rule of the home machine narrow.

> **CAUTION:** PUBLISH THE VNC PORT ON THE LOOPBACK OF THE DOCKER HOST ONLY.
> The VNC server has no password. A published port on another interface gives
> every host of the network access to the desktop and to the sign-ins.

> **CAUTION:** REMOVE ONLY THE CONTAINER, THE VOLUME AND THE IMAGE THAT THIS
> PROCEDURE CREATED. Another volume holds the work and the sign-ins of another
> person.

> **CAUTION:** KEEP A STATE VOLUME THAT HOLDS A SIGN-IN. A removed volume
> deletes every browser profile and every sign-in.

## 4. Equipment and preconditions

- A Linux computer with a working Docker daemon and internet access. It is the
  browser machine.
- A Linux home machine with OpenSSH and Tailscale on the same tailnet as the
  Docker host. The listener check uses `ss` from iproute2.
- An SSH login from the home machine to itself without a prompt. Task EG-LISTEN
  uses that login.
- Two public addresses. The browser machine and the home machine must use
  different addresses, or task EG-EGRESS proves nothing. A rented machine or a
  mobile hotspot gives the second address.
- A public page that shows the address of its client. This procedure uses
  `https://ifconfig.me/ip`.
- A Horizon instance with a Device panel that you can create.
- An SSH server on the Docker host, for the Device panel.

## 5. Setup

1. Define the names of the run.

   ```sh
   export RUN=1780-egress
   export VNC_PORT=5996
   ```

   Result: The shell holds the two values.

2. Build the image from the root of the repository.

   ```sh
   docker build -f packaging/browser-vm/Dockerfile -t horizon-browser-vm:$RUN .
   ```

   Result: The build ends with
   `naming to docker.io/library/horizon-browser-vm:1780-egress`.

> **CAUTION:** PUBLISH THE VNC PORT ON THE LOOPBACK OF THE DOCKER HOST ONLY.
> The VNC server has no password, and a published port on another interface
> gives every host of the network access to the desktop.

3. Start the container with a named volume and the loopback VNC port.

   ```sh
   docker run --detach --name hz-bvm-$RUN \
     --volume hz-bvm-$RUN-state:/state \
     --publish 127.0.0.1:$VNC_PORT:5900 \
     --shm-size 1g \
     horizon-browser-vm:$RUN
   ```

   Result: The command prints the ID of the container.

4. On the home machine, read the tailnet address.

   ```sh
   tailscale ip -4
   ```

   Result: The command prints one address, for example `100.64.0.10`. The rest
   of this procedure uses `100.64.0.10` as that address. Replace it with your
   address in every step that holds it.

5. Write the file `/tmp/host-$RUN.yaml` on the Docker host with this content.

   ```yaml
   browser:
     profiles:
       probe:
         backend: chromium
         args:
           - --no-sandbox
           - --disable-dev-shm-usage
       egress:
         backend: chromium
         args:
           - --no-sandbox
           - --disable-dev-shm-usage
           - --proxy-server=socks5://100.64.0.10:1080
   ```

   Result: The file holds the two profiles. The profile `probe` matches
   `packaging/browser-vm/host.example.yaml` lines 13 to 17. The profile `egress`
   matches the commented block at lines 31 to 36 of that file.

6. Put the host file in the state volume and give it the mode 0600.

   ```sh
   docker cp /tmp/host-$RUN.yaml hz-bvm-$RUN:/state/host.yaml
   docker exec hz-bvm-$RUN chmod 600 /state/host.yaml
   ```

   Result: The command `docker exec hz-bvm-$RUN stat -c '%a' /state/host.yaml`
   prints `600`. The profile host refuses a host file that the group or other
   users can write.

7. Restart the container, so that the profile host reads the new host file.

   ```sh
   docker restart hz-bvm-$RUN
   ```

   Result: The log holds `the profile host runs as <PID>, the browser runtime
   root is /state/browser-root`.

## 6. Tasks

### 6.1 EG-VIEW: A Device panel shows the desktop

1. Create a Device panel in Horizon with the SSH route to the Docker host and
   the VNC target `127.0.0.1:5996`.

   Result: The panel shows the desktop of the image, and the frame sequence
   increases.

### 6.2 EG-BASE: A profile without a proxy shows the address of the browser machine

1. Read the host instance of the profile host.

   ```sh
   HOST_INSTANCE=$(docker exec hz-bvm-$RUN \
     sed -n 's/.*"host_instance": "\(.*\)",/\1/p' /state/browser-root/profile-host.json)
   ```

   Result: The shell holds the host instance.

2. Start a connect client with the profile host.

   ```sh
   docker exec -i hz-bvm-$RUN env \
     HORIZON_BROWSER_ROOT=/state/browser-root \
     HORIZON_BROWSER_ACTOR=horizon:egress-run \
     HORIZON_BROWSER_HOST_INSTANCE=$HOST_INSTANCE \
     horizon-browser mcp --connect --inline-screenshots
   ```

   Result: The client answers the MCP messages of your client. Keep it open for
   the other tasks.

3. Call `browser_create` with the target `probe`.

   Result: The answer holds a panel with the ID `profile-probe-` and 12
   hexadecimal digits. The answer holds the backend `chromium`, and
   `navigation` is `not_requested`.

4. Call `browser_navigate` on that panel with the URL
   `https://ifconfig.me/ip`.

   Result: The answer holds `state: committed` and
   `committed_url: https://ifconfig.me/ip`.

5. Call `browser_evaluate` on that panel with the expression
   `document.body.innerText`.

   Result: The answer holds the public address of the Docker host. Record it as
   the address of the browser machine.

6. Look at the Device panel of task EG-VIEW.

   Result: The Chromium window shows the page with the address as a line of
   text.

### 6.3 EG-LISTEN: The home machine offers the SOCKS5 proxy on its tailnet address

The command `ssh -N -D 1080` binds `127.0.0.1` of the home machine by default.
Section 7 of `packaging/browser-vm/README.md` states this. The container runs on
another machine, so it cannot reach that loopback address. The command needs the
tailnet address of the home machine. The container reaches that address through
the network of the Docker host, so the container start needs no option for it.

> **CAUTION:** THE SOCKS5 LISTENER HAS NO PASSWORD. Anyone who reaches the
> named address uses the network of the home machine. Name the tailnet address
> and keep the tailnet access rule of that machine narrow.

1. On the home machine, start the proxy in the background against `localhost`.

   ```sh
   ssh -N -D 100.64.0.10:1080 localhost &
   echo $! > /tmp/egress-tunnel.pid
   ```

   Result: The shell returns the prompt, and `/tmp/egress-tunnel.pid` holds one
   process ID.

2. On the home machine, examine the listener.

   ```sh
   ss -ltn 'sport = :1080'
   ```

   Result: One line holds `100.64.0.10:1080` as the local address. A line that
   holds `127.0.0.1:1080` is a failure, because the container cannot reach it.

### 6.4 EG-EGRESS: A page of the egress profile leaves from the home machine

1. Call `browser_create` on the connect client with the target `egress`.

   Result: The answer holds a panel with the ID `profile-egress-` and 12
   hexadecimal digits, and `navigation` is `not_requested`.

2. Call `browser_navigate` on that panel with the URL
   `https://ifconfig.me/ip`.

   Result: The answer holds `state: committed` and
   `committed_url: https://ifconfig.me/ip`. The answer arrives in about one
   second.

3. Call `browser_evaluate` on that panel with the expression
   `document.body.innerText`.

   Result: The answer holds the public address of the home machine. It differs
   from the address of task EG-BASE.

4. Look at the Device panel of task EG-VIEW.

   Result: The Chromium window shows the page with the address of the home
   machine.

### 6.5 EG-OWN: The container keeps its own address

The tunnel of task EG-LISTEN runs in this task.

1. Examine the page through the proxy from the container.

   ```sh
   docker exec hz-bvm-$RUN chromium --headless --no-sandbox \
     --disable-dev-shm-usage --proxy-server=socks5://100.64.0.10:1080 \
     --dump-dom https://ifconfig.me/ip
   ```

   Result: The output holds the address of the home machine inside the element
   `<pre>`. The command also prints `dbus` error lines on stderr. Ignore them,
   because this command runs without the session bus of the profile host.

2. Examine the page without a proxy from the container.

   ```sh
   docker exec hz-bvm-$RUN chromium --headless --no-sandbox \
     --disable-dev-shm-usage --dump-dom https://ifconfig.me/ip
   ```

   Result: The output holds the address of the browser machine, and not the
   address of the home machine.

The image blocks no traffic. A profile gets an egress only from the `args` of
that profile.

### 6.6 EG-OFF: With the home machine off, the page does not load and the tool gives no reason

> **CAUTION:** THE NEXT STEP STOPS THE MACHINE THAT GIVES THE EGRESS. Start it
> again before you use it for other work.

1. Stop the home machine.

   Result: The home machine is off, and its tailnet address accepts no
   connection.

2. Call `browser_navigate` on the panel of task EG-EGRESS with the URL
   `https://ifconfig.me/ip` and the bound `timeout_millis` 20000.

   Result: The answer holds `state: timed_out` and `completed: false`. It holds
   `loading: true` and `committed_url: null`, and `isError` is false. The answer
   arrives after 20.1 s in the run that wrote this procedure. The answer holds
   no reason for the failure.

3. Look at the Device panel of task EG-VIEW.

   Result: The Chromium window shows a blank page. The page does not load.

An agent reads `loading: true` as "the page is still loading". The agent waits
or tries again, and each attempt ends the same way. The answer names neither the
egress nor the proxy. This is the gap of issue #1841.

### 6.7 EG-REFUSED: With the machine up and the tunnel stopped, the refusal names the proxy

> **CAUTION:** THE NEXT STEP STOPS THE SOCKS5 LISTENER. The profile `egress`
> then has no egress. Do not use a private page in that profile in this run.

1. Start the home machine again, and stop the tunnel on it.

   ```sh
   kill "$(cat /tmp/egress-tunnel.pid)"
   ss -ltn 'sport = :1080'
   ```

   Result: The home machine answers on its tailnet address. The command `ss`
   shows no listener, so the port 1080 accepts no connection.

2. Call `browser_navigate` on the panel of task EG-EGRESS with the URL
   `https://ifconfig.me/ip`.

   Result: The answer fails after 0.3 s with this text:
   `browser action <ID> failed (navigation_failed): could not navigate:
   net::ERR_PROXY_CONNECTION_FAILED`.

The text names the proxy and the failed connection. The text holds no address of
the egress. The person sees a blank page in the Device panel.

## 7. Pass criteria

- The profile `probe` shows the address of the browser machine.
- The listener of the home machine answers on its tailnet address.
- A page of the profile `egress` shows the address of the home machine, and that
  address differs from the address of the browser machine.
- The container keeps the address of the browser machine when a command uses no
  proxy.
- With the home machine off, the page does not load and no address of the egress
  appears in the answer.
- With the tunnel stopped on a machine that is up, the answer names the proxy
  with `net::ERR_PROXY_CONNECTION_FAILED`.
- Not met by the image that this procedure tested: a page that cannot reach the
  egress gives a refusal that names the egress. The tool gives `state: timed_out`
  with no reason. Issue #1841 tracks it.

## 8. Cleanup

1. Close the browser panels of the run.

   Result: `browser_close` on the panel of task EG-BASE and on the panel of task
   EG-EGRESS answers. The command `browser_list` shows neither panel.

2. Close the connect client of task EG-BASE.

   Result: The command ends and the terminal shows the prompt.

> **CAUTION:** THE NEXT STEP STOPS THE SOCKS5 LISTENER. The profile `egress`
> then has no egress.

3. Stop the tunnel if it still runs.

   ```sh
   kill "$(cat /tmp/egress-tunnel.pid)" 2>/dev/null || true
   ```

   Result: The command `ss -ltn 'sport = :1080'` prints no line.

4. Remove the process ID file of the tunnel.

   ```sh
   rm -f /tmp/egress-tunnel.pid
   ```

   Result: The file is gone.

> **CAUTION:** THE NEXT STEP DELETES THE HOST FILE OF THE RUN. The container
> writes the shipped example host file again, so the profile list of the run is
> gone.

5. Remove the host file of the run.

   ```sh
   docker exec hz-bvm-$RUN rm -f /state/host.yaml
   docker restart hz-bvm-$RUN
   ```

   Result: The log holds `wrote the example host file to /state/host.yaml`. The
   shipped example still names the profile `probe`.

> **CAUTION:** REMOVE ONLY THE CONTAINER AND THE VOLUME THAT THIS PROCEDURE
> CREATED. A removed volume deletes every browser profile and every sign-in.

6. Stop and remove the container, the volume and the image.

   ```sh
   docker rm -f hz-bvm-$RUN
   docker volume rm hz-bvm-$RUN-state
   docker image rm horizon-browser-vm:$RUN
   ```

   Result: `docker volume ls` and `docker image ls` show no name of the run.

7. Close the Device panel of task EG-VIEW.

   Result: The panel leaves the board. The other panels of the board stay.

## 9. Record of results

Put the results in the pull request that changes the image.
Keep the screenshots of the Device panel and the log files out of the
repository.
