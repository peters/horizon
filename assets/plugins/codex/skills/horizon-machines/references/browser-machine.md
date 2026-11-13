# Browser machine

A browser machine is one browser in a container on a Linux machine. Horizon
browser panels use it over SSH to that machine. Two engines are served: Chromium
through a `DevTools` endpoint on the loopback of the machine, and Firefox behind a
driver of Horizon that runs inside the container. Four files, beside this
reference, make it, and the firefox engine adds the horizon binary as a fifth
input:

| File | Job |
|---|---|
| `browser-machine/Dockerfile` | The image: Chromium, a virtual display, a window manager and a loopback bridge |
| `browser-machine/entrypoint.sh` | Starts the display, the window manager, the bridge and Chromium, and keeps them together |
| `browser-machine/horizon-browser-wrapper.sh` | Installed as `google-chrome` on the destination, so Horizon finds a browser and gets a container |
| `browser-machine/setup.sh` | Installs Docker, builds the image, installs the wrapper and starts the container; also prepares the firefox engine |

An agent copies these four files from this skill directory to the destination.
Do not type them again and do not write them with a here-document.

For the firefox engine, copy the horizon binary of this computer as a fifth
input, and name the command that the setup prints as `browser_host.command`: the
driver of Horizon runs inside the container, so that machine needs no horizon of
its own outside it. The binary must be built for the system and the CPU of the
destination.

## When to use it

Use it when the destination is a rented Linux machine, or a Linux machine with
no browser program and no graphical session, and the person wants a browser
panel of that machine. The container gives Chromium a display that the machine
does not have.

Do not use it:

- On a Mac, or on a Linux desktop with a real Chrome or Chromium. That browser
  is better in every respect, and `setup.sh` stops on such a machine unless you
  pass `--force`.
- To pass a bot check. The container is a Linux browser with software graphics.
  Some sites refuse it. Tell the person this before the setup.
- On a machine with no `apt-get` and no root. Only Debian and Ubuntu work.

The destination needs 3 GB of free disk and one profile name: 1 to 40
characters of `a-z`, `0-9` and `-`.

## The steps

### 1. Probe the machine

Run these lines with the machine key, in the machine-key SSH form of this
skill.

```sh
uname -s; uname -m; id -un; echo "$HOME"
command -v docker || true
command -v google-chrome || command -v chromium || true
df -h "$HOME" | tail -n 1
```

Stop when a Chrome or a Chromium answers: use that real browser instead.

### 2. Copy the four files and run the setup

Copy `browser-machine/Dockerfile`, `browser-machine/entrypoint.sh`,
`browser-machine/horizon-browser-wrapper.sh` and `browser-machine/setup.sh` to
one folder on the destination. Then run the setup as the user of the person's SSH
login (on a rented machine that is usually `root`):

```sh
scp browser-machine/Dockerfile browser-machine/entrypoint.sh \
    browser-machine/horizon-browser-wrapper.sh browser-machine/setup.sh <target>:
ssh <target> 'sh setup.sh --profile <profile>'
```

For the firefox engine, copy the horizon binary of this computer as well, and
name it:

```sh
scp "$(command -v horizon)" <target>:horizon
ssh <target> 'sh setup.sh --profile <profile> --engine firefox --horizon horizon'
```

The setup then ends with a `command=` value instead of a `port=` value. Write it
as `browser_host.command` in step 4, and set the engine of the entry to Firefox.
The horizon binary must be built for the system and the CPU of the destination.

The setup installs `docker.io` when Docker is missing, builds the image, installs
the wrapper as `/usr/local/bin/google-chrome` and starts the container through
that wrapper. A second run builds nothing and starts no second container. The
last line of the output is:

```
browser-machine ready profile=<profile> port=<n>
```

Show that line on the card of the machine.

### 3. Show the SSH entry of the person

Horizon starts its tunnel with the SSH configuration of the person, not with the
machine key. Show this block as text on the card. Do not write the file
`~/.ssh/config` yourself.

```sshconfig
Host bvm-1
    HostName <address>
    User <user that ran setup.sh>
    IdentityFile <private_key_path of the machine key>
    IdentitiesOnly yes
    UserKnownHostsFile <known_hosts_path of the machine key>
```

`User` must be the user that ran the setup: another user has another home folder
and another profile folder. Use the exact `private_key_path` and
`known_hosts_path` of the `machine_key` answer. The known hosts file is
necessary because Horizon runs its tunnel with `BatchMode=yes`, and an unknown
host key fails that connect.

### 4. Tell the person to set the browser host

Only the person can set this, in Settings (**Ctrl+Shift+,**) → **General** →
**Browser host**. Show this on the card:

| Field | Value |
|---|---|
| SSH target | `bvm-1` |
| Profile | `<profile>` |
| Engine | Chromium, or Firefox with Horizon on that machine |
| Driver program | `docker exec -i -e HOME=/profile hz-bvm-<profile> /opt/horizon/horizon --browser-host-driver`, for the firefox engine only |

`ssh_target` is the `Host` name of step 3, not the address. The change applies at
the next start of Horizon.

### 5. Let the person open a browser panel

Tell the person to start Horizon and to open a browser panel. The panel starts
the container when it is stopped, and it writes the endpoint file. A fresh
container can take 20 seconds.

## How the DevTools port is published

The wrapper publishes one Docker-chosen port on the loopback of the destination:
`127.0.0.1:<n>->9222/tcp`. Chromium keeps port `9223` on the loopback of the
container, and a `socat` bridge carries the published port to it. The wrapper
writes the published port as line 1 of
`<profile folder>/DevToolsActivePort`, which Horizon reads. Chromium writes
`9223` into that same file; the wrapper replaces it after the endpoint answers.

## Stop, start and remove

```sh
docker stop hz-bvm-<profile>       # the browser stops, the profile stays
docker start hz-bvm-<profile>      # the browser starts again, on a new published port
docker logs hz-bvm-<profile>       # the output of Chromium and of the entrypoint
docker rm -f hz-bvm-<profile>      # remove the container, keep the profile files
docker rmi horizon-browser-machine:local
rm -f /usr/local/bin/google-chrome
```

There is no Docker volume to remove: the profile folder is a bind mount. The
wrapper starts the container again at the next connect.

## What persists

- `~/.horizon/browser-profiles/<profile>` on the destination is mounted as
  `/profile` in the container.
- The folder is lost when the machine is deleted. A profile on a rented machine
  is temporary unless the person copies the folder. Its files belong to root.

## The limits

- **The DevTools port is local to the destination, and it is not private.** It
  asks for no credential, and every local account of that machine, and every
  container of its Docker networks, can reach it. Anyone who reaches it drives
  the browser with its sign-ins.
- **Docker is root.** A user in the `docker` group has root on that machine.
  Chromium runs as root with `--no-sandbox`, because the default Docker profile
  refuses the user namespaces of the Chromium sandbox. The page sandbox is off.
- **The wrapper shadows the first browser name that Horizon looks for**
  (`/usr/local/bin/google-chrome`).
- **The graphics are software graphics.** A site can refuse to serve it.
- **Fixed hostname.** The container name and its hostname are `hz-bvm-<profile>`.
  Another hostname on one profile folder makes Chromium report the profile as in
  use by another computer.
- **Shared memory.** The container runs with `--shm-size 1g` and Chromium with
  `--disable-dev-shm-usage`. Without both, a page with many frames crashes.
- **First start.** A fresh container is slower than a running one, and Horizon
  allows 20 seconds for a start.

## Check it by hand

On the destination, with the port from the endpoint file:

```sh
curl -s http://127.0.0.1:<port>/json/version
```

Correct: one JSON line with `"Browser": "Chrome/..."` and a
`"webSocketDebuggerUrl"`. The `webSocketDebuggerUrl` names port `9223`: that is
the port of the container, and Horizon replaces it with the port of its forward.

Three more checks on the destination:

```sh
docker ps --filter name=hz-bvm-<profile> --format '{{.Names}} {{.Status}}'
docker port hz-bvm-<profile>
docker inspect --format '{{.State.Health.Status}}' hz-bvm-<profile>
```

Correct: one container, `Up ... (healthy)`, and one
`127.0.0.1:<port>->9222/tcp` line with the port of the endpoint file.

## Proven, and not proven

A live run on a fresh Ubuntu 24.04 machine with 2 vCPU and 4 GB proved these
lanes:

- `sh setup.sh --profile work` installed `docker.io`, built the image and ended
  with `browser-machine ready profile=work port=32768`, exit 0, in a few minutes.
- The container was healthy. `curl -s http://127.0.0.1:32768/json/version` on the
  destination answered one `Chrome/156` version line, `docker ps` showed
  `127.0.0.1:32768->9222/tcp`, the wrapper was found as
  `/usr/local/bin/google-chrome`, and the endpoint file held the port on line 1.
- A Horizon browser panel with `ssh_target` of that machine and the profile
  `work` connected in less than 2 seconds, painted the page through the
  screencast, and navigated to a public page with the right title. The headed
  Chromium under `Xvfb` therefore gives screencast frames, the `socat` bridge
  carries the `DevTools` WebSocket, and the empty host port form
  (`--publish 127.0.0.1::9222`) works.
- After `docker stop`, a new panel found the old port in the endpoint file and
  reported `attached_host_no_endpoint`. Horizon then ran its start snippet, the
  wrapper ran `docker start`, the published port changed to `32769`, and the panel
  connected 3 seconds later.

These lanes are not proven:

- A sign-in that survives a restart of the container. The profile folder is a
  bind mount, but no run has signed in and then restarted the container.
- A second profile on one machine, which needs a second container name.
- `arm64`. The image packages are architecture-independent, but no run used it.
- The profile lock after a change of the container hostname.
- A site that refuses software graphics, and how it shows in the panel.

## The four files are in this skill directory

The same four files are in the repository under `packaging/browser-machine/`,
with a README for a person. When one copy changes, change the other:
`packaging/browser-machine/Dockerfile` and
`browser-machine/Dockerfile` must stay identical, and the same for the three
scripts.
