# Docker host clouds

A Docker host is a third provider for CPU clouds, beside RunPod and Hetzner
(epic #1463). It is a Docker engine that you already have: the engine of this
computer, a Linux server that you reach with SSH, or a machine on your tailnet.
Each cloud on a host gets its own container and its own workspace volume. Nothing
is rented, so a cloud on a host has no cost in Horizon.

## Add a host

1. Open **Menu > Cloud settings**.
2. In the **Docker host** card, select **Use a Docker host for clouds**.
   Result: The host editor opens.
3. Type a **Name**, for example `build-box`. Use letters, digits, `-` and `_`.
4. Click how Horizon reaches the host: **This PC**, **SSH** or **Tailscale SSH**.
5. For SSH or Tailscale SSH, type the **Address**, for example `deploy@build-box`.
   Result: You can use an alias from your SSH configuration.
6. If `docker` is not on the PATH of a non-interactive SSH session, type the
   **Docker path**, for example `/usr/local/bin/docker`.
7. Click **Check host**.
   Result: The check rows show one by one. At the end, only the failures and the
   warnings stay in view, each with its remedy.
8. If the check shows no failure, click **Keep host**.
9. Click **Save settings**.
   Result: The card shows the host with its architecture, its Docker version and
   the CPUs and memory that it has free.

The check runs `docker version`, `docker info` and lists the containers that
carry Horizon's owner label. It starts, pulls and installs nothing. The only file
that it writes is Horizon's private Docker configuration, one time:
`~/.horizon/docker-config/config.json` on a remote host, and a file in
Horizon's own cloud folder for This PC. This configuration keeps docker away
from a desktop credential store (see the requirements below).

To change or check a saved host again, click **Edit** on its row. **Remove host**
removes it from the settings. A cloud on a host that you remove or rename cannot
stop, resume or be deleted until you save the host again under its old name.

Clear **Use a Docker host for clouds** to remove your hosts from
`cloud_offers`. The saved hosts stay in the settings, and clouds on them keep
working. A profile that names `provider: docker-host` directly, for example in
`cloud_deploy`, still runs on the saved host.

## Host requirements

- **Docker 28 or later.** Older engines can let other computers on the same
  network reach a port that a container publishes on the loopback. The check
  refuses them.
- **Docker access for the user.** The SSH user must be able to run `docker`
  without `sudo`, for example as a member of the `docker` group.
- **SSH without a prompt.** SSH must sign in with your keys or agent, and the
  host key must be in your `known_hosts` file. Horizon stores no secret and copies
  no key.
- **Tailscale SSH in accept mode.** The tailnet policy rule for the host must use
  `action: accept`. A rule with `action: check` waits for a browser, and the
  check stops after 20 seconds.
- **The docker path.** A non-interactive SSH session often has a short PATH.
  Horizon adds `/usr/local/bin` and `/opt/homebrew/bin`. For other places, type
  the docker path.
- **Linux containers on amd64 or arm64.** The worker image has an amd64 build
  and an arm64 build. Docker selects the build of the host, so an Apple silicon
  Mac with Docker Desktop runs the worker natively.
- **No desktop credential store.** Horizon gives docker on the host a private
  configuration in `~/.horizon/docker-config` with no credential helper. Docker
  Desktop on macOS then never asks the keychain, which over SSH can wait for a
  person at the host. Do not point `DOCKER_CONFIG` at the desktop configuration.

## Start a cloud on a host

**New cloud** shows each saved host as a **YOUR HOST** card before the three
rented starting points. The card shows the profile's size, the Docker whale,
**No rent** and the CPUs and memory that the host has free now.

- **Fits**: click the card to run the cloud on that host.
- **Too little free now**: the host cannot take a cloud of this size until a
  cloud on it stops.
- **Not reachable**: the last read failed. Check the host in Cloud settings.
- **Architecture not supported**: the worker image has no build for the
  architecture of the host. Use an amd64 or arm64 host.
- **Cannot run this profile**: the profile builds its image, has hosted devices
  or has an idle stop. A host pulls public images only and has no hosted
  devices or idle stop.

A GPU profile shows no host card: a Docker host runs CPU clouds only. A profile
with `provider: docker-host` shows only the host cards. When one host fits,
New cloud selects it.

Free capacity is not reserved. Other programs on the host share its CPUs and
memory, and the workspace volume uses the host's disk. Horizon reads the free
capacity again every 30 seconds while New cloud or Cloud settings is open.

## A cloud on a host

The cloud card shows the Docker whale in place of the cloud mark. Stop, Resume
and Delete work as for a rented worker:

- **Stop** stops the container. The container and its volume stay on the host.
- **Resume** starts the same container on the same loopback port.
- **Delete** removes only the container and the volume of this cloud.

**Manage** shows **Rebuild image & restart…** but you cannot click it. A cloud on
a host keeps the image that it started with. To get a new image, delete the cloud
and start it again.

The worker publishes its SSH only on the host's loopback. Horizon reaches it
through the host with an SSH `ProxyCommand`. The Docker API is never exposed.

## Agents

`cloud_offers` lists the saved hosts in `other_providers`, in a section with
`provider: docker-host`. Each offer has the host's name as its `id`, the largest
size that is free now as `vcpu` and `memory_gb`, and an `estimated_total` of 0.
A host that Horizon could not read is in `host_errors`. It also sets
`comparison_incomplete`, because that host can be the cheapest offer.
Adding, checking, switching off and removing a host are settings tasks for the
person in Cloud settings, as the provider keys are. An agent sees the result: a
saved host that answers is an offer, and a host that does not answer is in
`host_errors` with the reason.

A cloud on a Docker host cannot be a companion yet. Its worker is on the host's
loopback, which the worker of another cloud cannot reach. The companion tools
refuse such a cloud with `cloud_companion_unsupported`.
