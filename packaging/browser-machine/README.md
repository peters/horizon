# Browser machine

A browser machine is one Chromium in a container on a Linux machine. Horizon
browser panels use it over the SSH tunnel of that machine, so a rented Linux
machine without a desktop can be the browser host of a panel.

## What it is

Four files, and one command on the destination:

| File | Job |
|---|---|
| `Dockerfile` | The image: Chromium, a virtual display, a window manager and a loopback bridge |
| `entrypoint.sh` | Starts the display, the window manager, the bridge and Chromium, and keeps them together |
| `horizon-browser-wrapper.sh` | Installed as `google-chrome` on the destination, so Horizon finds a browser and gets a container |
| `setup.sh` | Installs Docker, builds the image, installs the wrapper and starts the container |

Copy the four files to one folder on the destination, and run the setup:

```sh
scp Dockerfile entrypoint.sh horizon-browser-wrapper.sh setup.sh <target>:
ssh <target> 'sh setup.sh --profile work'
```

The last line is `browser-machine ready profile=work port=<n>`. The destination
then has a Chromium that answers a `DevTools` endpoint on its loopback, and
Horizon reaches it through the browser host tunnel
(`docs/testing/procedures/remote-browser-tunnel.md`).

The person sets the browser host in the settings of Horizon: `ssh_target` is the
`Host` name of their own SSH configuration, and `profile` is the profile of the
setup.

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
