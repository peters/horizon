# Browser image

This directory holds the image of the browser machine of
[issue 1780](https://github.com/peters/horizon/issues/1780).
The image runs a display, a VNC server and the profile host
`horizon-browser host`.
The profile host keeps the named browser profiles that the browser tools of an
agent use.

## 1. Contents

| Part | Package | Version of the proof build |
|---|---|---|
| Base | `debian:12-slim` | Debian 12 |
| Display | `xvfb` | `2:21.1.7-3+deb12u13` |
| Window manager | `openbox` | `3.6.1-10` |
| VNC server | `x11vnc` | `0.9.16-9` |
| Session bus | `dbus-x11` and `dbus` | `1.14.10-1~deb12u1` |
| Fonts | `fonts-dejavu-core`, `fonts-liberation` | `2.37-6`, `1:1.07.4-11` |
| Browser | `chromium` | `156.0.8078.47-1~deb12u1` |
| Profile host | the binary `horizon-browser` of `horizon-browser-cli` | `0.2.7` |

The image holds 313 Debian packages in total.
Its size is 1.31 GB.
The builder stage also installs `nasm`, because the video encoder `rav1e` of
the browser engine needs it on x86_64.

The `chromium` package gives the command `chromium`.
`horizon-browser` accepts that name for a Chromium backend, as the list
`BINARY_CANDIDATES` in `crates/horizon-browser/src/process.rs` shows.

The container runs as root, so Chromium needs `--no-sandbox`.
Put that argument in the host file, for each profile of a Chromium backend.
`host.example.yaml` shows it.

## 2. Volumes

| Path | Contents |
|---|---|
| `/state` | The state volume. Keep it across a stop and a start of the container. |
| `/state/host.yaml` | The host file of the profile host. The container copies `host.example.yaml` there when the volume has no host file. |
| `/state/browser-root` | The browser runtime root, in `HORIZON_BROWSER_ROOT`. It holds `profile-host.json` and one folder for each browser profile. |

A browser profile keeps its cookies in the state volume.
A sign-in of a profile stays when the container stops and starts again.

The profile host refuses a host file that the group or other users can write.
Give the host file the mode 0600.
The container writes the example with that mode.

The person who signs in must be able to write the host file.
Root in the container can read a host file that a person of the Docker host
writes, because the profile host reads it as root.

## 3. Ports

The image declares one TCP port.

| Port | Use | Reach |
|---|---|---|
| `5900/tcp` | The VNC server of the display | The network of the container, and the published port of the Docker host. |

Publish the port on the loopback of the Docker host only:

```sh
--publish 127.0.0.1:5900:5900
```

The image opens no other port.
The display is a Unix socket, not a port, because Xvfb runs with `-nolisten tcp`.
The DevTools port of Chromium binds the loopback of the container.
The profile host opens no port: a connect client reads the files of the browser
runtime root.

The SSH route of the Device panel uses the SSH server of the Docker host, on
the port 22 of that host.
That port belongs to the Docker host, not to the image.

## 4. Build the image

Build the image from the root of the repository, because the first stage
compiles the profile host from the sources:

```sh
docker build -f packaging/browser-vm/Dockerfile -t horizon-browser-vm .
```

The first stage uses the image `rust:1.99.0-bookworm` and the command
`cargo build --release --locked -p horizon-browser-cli`.
The second stage uses `debian:12-slim`, so the binary links the same C library
that the image has.
The file `Dockerfile.dockerignore` replaces the ignore file of the repository
root for this build.
Without it, the ignore file of the root keeps the sources of the CLI out of the
build context.

A build with a warm layer cache takes about 2 minutes.
A build with no cache takes longer, because it compiles the CLI and its
dependencies.

## 5. Start the container

```sh
docker run --detach --name horizon-browser-vm \
  --volume horizon-browser-vm-state:/state \
  --publish 127.0.0.1:5900:5900 \
  --shm-size 1g \
  horizon-browser-vm
```

The command `--shm-size 1g` gives Chromium more shared memory than the 64 MB
that Docker gives by default.
The argument `--disable-dev-shm-usage` in `host.example.yaml` is a second
guard for a container with the default size.
The egress profile of `host.example.yaml` names a proxy on another machine, so
this command needs no option for it.
Section 7 gives the proxy.

Examine the start:

```sh
docker logs horizon-browser-vm
```

Result: the log shows the display, the session bus, the VNC port and the
process ID of the profile host.

The entrypoint stops the display, the window manager, the VNC server and the
profile host together when the container stops.
The container exits with a code other than 0 when the profile host stops
without that signal.

| Variable | Default | Use |
|---|---|---|
| `HORIZON_BROWSER_VM_STATE` | `/state` | The state directory. |
| `HORIZON_BROWSER_VM_DISPLAY` | `99` | The number of the X display. |
| `HORIZON_BROWSER_VM_VNC_PORT` | `5900` | The port of the VNC server in the container. |
| `HORIZON_BROWSER_VM_SCREEN` | `1920x1080x24` | The size and the depth of the display. |

## 6. Sign in one time

1. Publish the VNC port on the loopback of the Docker host, as section 3 shows.
2. Open a Device panel with the SSH route to the Docker host:

   ```json
   {"ssh": {"host": "<the Docker host>", "vnc": "127.0.0.1:5900"}}
   ```

   Result: The Device panel shows the desktop of the image.
3. Open the browser profile in that desktop and sign in to the sites of your
   work.

   Result: The profile folder in `/state/browser-root/browser-profiles` keeps
   the cookies of the sign-in.

The sign-in stays after a stop and a start of the container, because the state
volume keeps the profile folder.

## 7. Network and egress

The image does not block the network and does not route it through a proxy.
A Chromium in the image reaches the internet as the network of the Docker host
allows, so a profile without a proxy argument uses the network of that host.

A browser profile gets its own egress only from the `args` of that profile in
the host file.
`host.example.yaml` shows a commented profile whose Chromium argument is
`--proxy-server=socks5://100.64.0.10:1080`.
The egress of issue 1780 is the SOCKS5 proxy of the home machine, so the
profile names the tailnet address of the home machine.

To use the profile, remove the comment markers of the block, put it at the
indentation of the profile `probe`, and replace the example address
`100.64.0.10` with the tailnet address of your proxy machine:

```yaml
    egress:
      backend: chromium
      args:
        - --no-sandbox
        - --disable-dev-shm-usage
        - --proxy-server=socks5://100.64.0.10:1080
```

A tailnet address needs no option in the container start of section 5, because
the container reaches it through the network of the Docker host.

Run the proxy on that machine with a listener that accepts the connection of
the container.
The default listener of `ssh -N -D 1080` binds `127.0.0.1`, which another
machine cannot reach, so name the tailnet address of the machine:

```sh
ssh -N -D 100.64.0.10:1080 <the proxy machine>
```

> **CAUTION:** CHOOSE THE ADDRESS OF THE SOCKS5 LISTENER WITH CARE.
> The listener has no password.
> Anyone who reaches the address uses the network of the proxy machine.
> Name the tailnet address as above, and keep the tailnet access rule of that
> machine narrow.
> Do not use `0.0.0.0` on a machine with a public address.

To examine the egress profile from the container:

```sh
docker exec horizon-browser-vm chromium --headless --no-sandbox \
  --disable-dev-shm-usage --proxy-server=socks5://100.64.0.10:1080 \
  --dump-dom https://example.org/
```

Result: the command prints the page `Example Domain`, and the log of the proxy
holds the connection of the container.
A refused connection means that the listener of the proxy does not accept the
container, and the CAUTION above names the reason.

The image needs no proxy rule of its own, and it needs no option in the start
for a profile that names a proxy on another machine.
A profile that uses a proxy on the Docker host itself needs the name
`host.docker.internal` and the option
`--add-host=host.docker.internal:host-gateway` in the start of section 5,
because Docker Engine on Linux adds that name only with the option.
Do not depend on this image to stop a profile that has no proxy.

## 8. Limits

> **CAUTION:** PUBLISH THE VNC PORT ON THE LOOPBACK OF THE DOCKER HOST ONLY.
> The VNC server has no password. A published port on another interface gives
> every host of the network access to the desktop and to the sign-ins of the
> profiles.

> **CAUTION:** KEEP THE STATE VOLUME OF A MACHINE THAT HOLDS A SIGN-IN.
> A removed volume deletes every browser profile and every sign-in.

- The container runs as root, so each Chromium profile needs `--no-sandbox`.
  The sandbox of Chromium does not protect the pages of the profiles.
- The image serves one display. Every browser profile uses that display.
- The image does not run an SSH server. Use the SSH server of the Docker host.
- The image has no credential of its own. The host file holds no credential
  value.
