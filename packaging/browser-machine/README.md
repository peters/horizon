# Browser machine

A browser machine is one browser in a container on a Linux machine. Horizon
browser panels use it over SSH to that machine, so a rented Linux machine
without a desktop can be the browser host of a panel. Two engines are served:

- **Chromium** answers a `DevTools` endpoint on the loopback of the destination,
  and Horizon reaches it through the browser host tunnel. No Horizon program is
  needed on that machine.
- **Firefox** runs behind a driver of Horizon **inside** the container. The
  driver of that container keeps the browser and the page while no computer is
  linked, so the same page comes back after a cut link. The driver is the horizon
  binary of the person's computer, so the image is a distribution whose C library
  is not older than the one that binary needs.

## What it is

Four files, one optional fifth input, and one command on the destination:

| File | Job |
|---|---|
| `Dockerfile` | The image: Chromium, Firefox, geckodriver, a virtual display, a window manager and a loopback bridge; the build needs the network for `apt-get` and for the pinned release of geckodriver |
| `entrypoint.sh` | Starts the display, the window manager, the bridge and Chromium, and keeps them together; the firefox engine starts no display |
| `horizon-browser-wrapper.sh` | Installed as `google-chrome` on the destination, so Horizon finds a browser and gets a container |
| `setup.sh` | Installs Docker, builds the image, installs the wrapper and starts the container; also prepares the firefox engine |
| `horizon` | The fifth input, and only for the firefox engine: the horizon binary of this computer, copied beside the Dockerfile |

Copy the four files to one folder on the destination, and run the setup for
Chromium:

```sh
scp Dockerfile entrypoint.sh horizon-browser-wrapper.sh setup.sh <target>:
ssh <target> 'sh setup.sh --profile work'
```

The last line is `browser-machine ready profile=work port=<n>`. The destination
then has a Chromium that answers a `DevTools` endpoint on its loopback, and
Horizon reaches it through the browser host tunnel
(`docs/testing/procedures/remote-browser-tunnel.md`).

For Firefox, copy the binary of this computer as well and name it:

```sh
scp Dockerfile entrypoint.sh horizon-browser-wrapper.sh setup.sh <target>:
scp "$HORIZON_EXECUTABLE" <target>:horizon
ssh <target> 'sh setup.sh --profile work --engine firefox --horizon horizon'
```

`HORIZON_EXECUTABLE` is the program that runs Horizon, and an agent panel holds
its path. Do not look for a `horizon` on `PATH`: another program of that name
would speak another version of the link, and the driver refuses it.

The setup reads two C libraries before it builds the image: the library that the
copied binary needs, and the library of the image. Run only that check, which
costs no build:

```sh
ssh <target> 'sh setup.sh --engine firefox --horizon horizon --check'
```

Its last line names the two versions. It ends with code 0 when the driver can
start in the image, and with code 3 and three ways forward when it cannot. The
setup runs the same check first, so an image is never built for a driver that
cannot start.

The last line is
`browser-machine ready profile=work engine=firefox command=docker exec -i -e HOME=/profile hz-bvm-work /opt/horizon/horizon --browser-host-driver`.
Write that `command` as `browser_host.command` in the settings, name the
destination as `ssh_target`, name `work` as `profile`, and set the engine of the
entry to Firefox. The `person` then sees the picture of that container's Firefox
in a browser panel.

The binary must be built for the system and the CPU of the destination: the
image builds there and runs there. An `x86_64` Linux binary is the usual one; an
`arm64` destination needs an `arm64` horizon, because the container runs the
architecture of its host.

The binary must also not need a newer C library than the image has. The image is
built on `debian:forky-slim`, which is Debian 14, and its library is 2.43. A
binary that a newer distribution built needs a newer library and does not start
there, and `setup.sh` stops before the image build with the two versions and
three ways forward: the Chromium engine, a horizon binary of a system of library
2.43 or older, or a base image with the newer library. That limit holds because
the driver runs inside the container, whose library this Dockerfile chooses; the
host of the machine takes the library of the image of the provider, which is
older.

## The limits

- **The DevTools port is on the loopback of the destination and it asks for no
  credential.** Every local account of that machine can drive the browser and use
  its sign-ins.
- **The wrapper takes the first browser name that Horizon looks for**
  (`/usr/local/bin/google-chrome`). On a machine with a real Chrome or Chromium,
  use that browser instead: `setup.sh` stops unless you pass `--force`.
- **Chromium runs as root with no sandbox,** because the default Docker seccomp
  profile refuses the user namespaces that the sandbox needs.
- **The graphics are software graphics.** A site can refuse to serve it.
- It is Linux only, and it needs `apt-get` and root (Debian and Ubuntu).
- The profile folder lives on the destination. When the machine is deleted, its
  sign-ins are gone.
- **The firefox engine is new and no live run proved it yet.** Its steps, its
  refusals and its same-page promise were proven on one computer with a second
  user account; the container lane is documented and unproven, and its image was
  measured: Debian 14 packages, C library 2.43, `geckodriver 0.37.1`,
  `firefox-esr 153.5.1esr`, and a Chromium that answers its `DevTools` port. Read
  `docs/testing/procedures/browser-host-firefox.md`.
- The firefox engine copies the horizon binary of this computer into the image.
  A destination of another system or CPU needs a horizon built for it, and both
  builds must be the same version, because the link of a browser host refuses
  another version with words.

## What a live run proved

A fresh Ubuntu 24.04 machine with 2 vCPU and 4 GB, `sh setup.sh --profile work`,
a Horizon panel through the tunnel, a navigation, a `docker stop` and a `docker
start`: see the Proven list in the skill reference
`assets/plugins/codex/skills/horizon-machines/references/browser-machine.md`.

## The two copies

The four files are also shipped as reference files of the `horizon-machines`
skill, under `assets/plugins/{codex,claude-code}/skills/horizon-machines/references/browser-machine/`,
because an agent on the person's computer reads the skill and not this
repository. The copies must be identical:

```sh
diff -r packaging/browser-machine assets/plugins/codex/skills/horizon-machines/references/browser-machine
```
