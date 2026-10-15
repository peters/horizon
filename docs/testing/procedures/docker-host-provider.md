---
procedure: docker-host-provider
feature: Clouds on an existing Docker host
platforms: [linux, macos]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Docker host provider test procedure

## 1. Purpose

This procedure proves that Horizon creates, stops, resumes and deletes clouds
on a Docker engine that the person already has, through the same deployment
and lifecycle as rented workers. It also proves that a delete removes only
the cloud's own container and volume.

## 2. Applicability

- Candidate: a Horizon checkout that has the `docker-host` provider.
- Platforms: this computer's Docker engine on Linux, and a remote Docker
  host that you reach with SSH. The remote host can be Linux or macOS with
  Docker Desktop.
- Tasks D1 to D5 use the `cloud_deploy` example and a settings file. Tasks D6
  to D9 use the Cloud settings card, New cloud, the cloud card and the MCP
  tools in an isolated Horizon.

## 3. Safety

> **CAUTION:** DO NOT USE THE DEFAULT DOCKER CONFIGURATION OVER SSH ON A MAC.
> A keychain prompt over SSH stops that Mac until a person closes it. Horizon
> uses a private configuration; use one for manual commands too.

> **CAUTION:** DELETE ONLY THE TEST CLOUDS.
> The delete step removes a container and a volume on the host. Use test
> cloud IDs that no other cloud uses.

## 4. Equipment and preconditions

- A Linux computer with Docker and Git, and the Horizon checkout.
- On each Docker host: a stable Docker Engine 28 or later with Linux
  containers. This computer needs 4 free CPUs and 8 GB of free memory, because
  tasks D1 and D5 run two clouds at the same time; a remote host needs 2 CPUs
  and 4 GB.
  Horizon refuses older engines, because they let other hosts reach ports
  published on the loopback.
- For task D3: a remote host that `ssh <host> true` reaches without a
  password prompt. On that host, the SSH user has access to Docker, and
  `docker` is at a known absolute path.
- For tasks D6 to D9: the isolated Horizon desktop of
  [the device-smoke runbook](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel).
- Network access to `ghcr.io` to pull the public worker base image.

## 5. Setup

1. Set `CARGO_TARGET_DIR` to a directory in the checkout's worktree target and
   `TMPDIR` to a private directory with mode 700. Do not use `/tmp`.

   Result: The builds and the test files go to these directories.

2. Run `cargo build -p horizon-core --example cloud_deploy`, then set
   `cloud_deploy="$CARGO_TARGET_DIR/debug/examples/cloud_deploy"`.

   Result: The `cloud_deploy` example is built, and `"$cloud_deploy"` names it.

3. Make a test directory `$T` outside your home, for example under
   `/var/tmp`, so that the fixture of task D6 can read it. Make a Git
   repository in `$T/repo` with one commit. Put this text in `$T/repo/.horizon/cloud.yml` before the commit:

   ```yaml
   version: 1
   default: local
   profiles:
     local:
       provider: docker-host
       image: ghcr.io/peters/horizon-worker-base@sha256:a3abdf1800d95adccdf93438208a206d2353f9a6fe69c4529faea791f73230f7
       cpu: 2
       memory_gb: 4
       gpu: false
       storage: {container_gb: 20, volume_gb: 20}
       bootstrap: {readiness_seconds: 600}
       capabilities: {agents: [claude, codex], browsers: [chromium], desktop: true}
   ```

   Result: The repository has a Docker host profile.

4. Run `ssh-keygen -t ed25519 -N '' -f $T/worker_ed25519`.

   Result: The worker key pair is in `$T`.

5. Write `$T/settings.json` with absolute paths in `$T` for
   `runpod_key_file`, `ssh_identity_file` (the new key) and `docker_config`,
   `registry_pull_auth_id: null` (the image is public), `cpu_flavors: ["cpu3c"]`,
   `gpu_types: []`, and this host list:

   ```json
   "docker_hosts": [{"name": "this-pc", "access": "this-pc"}]
   ```

   Result: The settings name this computer as the only Docker host.

6. Write down the number of containers and volumes on the host:
   `docker ps -aq | wc -l` and `docker volume ls -q | wc -l`.

   Result: You have the baseline counts.

## 6. Tasks

In each command, `cloud_deploy` stands for `"$cloud_deploy"` from setup step 2.

### 6.1 D1: Two clouds on this computer

1. Run `cloud_deploy deploy $T/settings.json $T/repo local $T/state/one test-one`.

   Result: The last lines show `Ready: worker=<container ID>`.

2. Run the same command with `$T/state/two` and `test-two`.

   Result: The second cloud is ready too.

3. Run `docker ps --filter label=io.horizon.cloud`.

   Result: The containers `horizon-cloud-test-one` and
   `horizon-cloud-test-two` run. Each publishes port 22 on `127.0.0.1` only,
   and the two ports are different.

### 6.2 D2: Stop and resume on the same port

1. Run `cloud_deploy stop $T/settings.json $T/state/one`, then
   `cloud_deploy reconcile $T/settings.json $T/state/one`.

   Result: The report shows `inactive` and `Resume starts the same worker again.`

2. Run `cloud_deploy resume $T/settings.json $T/state/one`, then
   `cloud_deploy reconnect $T/settings.json $T/state/one`.

   Result: The cloud is ready again. `docker ps` shows the same container on
   the same `127.0.0.1` port as in task D1.

### 6.3 D3: A remote host over SSH

1. Write `$T/settings-remote.json` like `$T/settings.json`, with this host
   list. Use your host's SSH address and docker path:

   ```json
   "docker_hosts": [{"name": "remote", "access": "ssh", "address": "<user@host>", "docker": "/usr/local/bin/docker"}]
   ```

   Result: The settings name the remote host.

2. On the remote host, write down its baseline with a private Docker
   configuration, so that no credential store is asked:
   `d=$(mktemp -d); echo '{"auths":{"horizon.invalid":{}}}' > "$d/config.json"`,
   then `DOCKER_CONFIG="$d" <docker path> -H unix:///var/run/docker.sock ps -aq | wc -l`
   and the same with `volume ls -q`. Remove `$d` after task D4.

   Result: You have the remote baseline counts.

3. Run `cloud_deploy deploy $T/settings-remote.json $T/repo local $T/state/remote test-remote`.

   Result: The cloud is ready.

4. Run `cloud_deploy endpoint $T/settings-remote.json $T/state/remote`. Then
   run `ssh -F none -o StrictHostKeyChecking=yes -o HostKeyAlias=<host_key_alias>
   -o UserKnownHostsFile=<known_hosts> -o "ProxyCommand=<proxy_command>"
   -i <identity_file> -p <port> root@127.0.0.1 uname -m` with the printed values.

   Result: SSH accepts the pinned worker host key through the host. On an ARM
   host, `uname -m` shows `aarch64`: the host runs the arm64 build of the
   worker image.

5. Run `cloud_deploy stop`, `reconcile`, `resume` and `reconcile` with
   `$T/settings-remote.json` and `$T/state/remote`.

   Result: The reports show `inactive` after the stop and `found` after the
   resume. The container keeps the same loopback port.

### 6.4 D4: Delete removes only the cloud's own resources

> **CAUTION:** DELETE ONLY THE TEST CLOUDS OF THIS RUN.
> This step removes a container and a volume on each host. Make sure that each
> state directory is one this run made and that its settings name the host
> this run used.

1. Run `cloud_deploy delete <settings> <state>` for each test cloud.

   Result: Each command shows `Stage: Delete worker` and ends without an
   error.

2. Count the containers and volumes on each host again.

   Result: The counts are the same as the baseline counts from setup step 6
   and, for the remote host, from task D3 step 2.
   No container or volume with the label `io.horizon.cloud` remains for the
   test clouds.

### 6.5 D5: Automatic tests

1. Run `cargo test -p horizon-cloud docker_host`.

   Result: All tests pass. They use a simulated Docker engine.

2. Run `cargo test -p horizon-core --test docker_host_live -- --ignored`.

   Result: The test passes and removes its two clouds.

### 6.6 D6: Add and check a host in Cloud settings

1. Run `cargo build -p horizon-ui --bin horizon`. Start that binary with
   `python3 scripts/device-smoke/serve.py --horizon "$CARGO_TARGET_DIR/debug/horizon"
   --native-view --state $S`, where `$S` is a new directory outside your home
   (the fixture masks the home), and show it in a native VNC Device panel.
   Inside the fixture, your home is `$S/data/home`. Run
   `mkdir -p -m 700 $S/data/home/.horizon/cloud`, then make a key there with
   `ssh-keygen -t ed25519 -N '' -f $S/data/home/.horizon/cloud/worker_ed25519`.
   Write the settings of setup step 5 without `docker_hosts` to
   `$S/data/home/.horizon/cloud/settings.json`, with paths as the fixture sees
   them: `$HOME/.horizon/cloud/...` with your real `$HOME` value.

   Result: Horizon starts on the isolated desktop and reads those settings.

2. Open **Menu > Cloud settings**.

   Result: The **Docker host** card shows **Off**.

3. Select **Use a Docker host for clouds**. Type the name `this-pc`, click
   **This PC**, then click **Check host**.

   Result: The rows show one by one. The check ends with "4 of 4 checks
   passed" or with the warnings and their remedies only.

4. Type the docker path `/nonexistent/docker` and click **Check host**.

   Result: The **Docker access** row shows "docker was not found on this
   computer" and the remedy to set the docker path. **Keep host** stays off.

5. Clear the docker path, check again, click **Keep host**, then click
   **Save settings**.

   Result: The settings file has `docker_hosts` with `this-pc` and no secret.
   When you open Cloud settings again, the row shows the architecture, the
   Docker version and the free CPUs and memory.

### 6.7 D7: Start a cloud on the host from New cloud

1. Open **New cloud** for `$T/repo` and select the profile `local`.

   Result: Only the **YOUR HOST** card shows, with the Docker whale, **No
   rent**, the free capacity and **Fits**. The card is selected. The summary
   shows "Docker host this-pc" and no Hetzner text.

2. Add a profile `gpu-test` with `provider: runpod` and `gpu: true` to the
   working copy of `$T/repo/.horizon/cloud.yml`, and select it.

   Result: No host card shows. New cloud says that Docker hosts run CPU clouds
   only.

3. Remove `gpu-test`. Set `cpu: 512` in the profile `local` and select it.

   Result: The host card shows **Too little free now** and you cannot click
   it. **Start cloud** stays off.

4. Add `build: {context: ., dockerfile: Dockerfile}` to the profile `local`
   and select it.

   Result: The host card shows **Cannot run this profile**, and below it the
   reason: a Docker host runs public images only.

5. Restore the profile `local` and select it. Keep New cloud open. In a
   terminal, add `"docker_hosts_off": true` to `.horizon/cloud/settings.json`
   in the home of the isolated Horizon, as another Horizon would save it, and
   wait 5 seconds.

   Result: The host stays chosen in the summary. **Start cloud** is off and
   says to save a Docker host and turn it on. No rented worker is chosen in
   its place.

6. Remove `"docker_hosts_off": true` again and wait 5 seconds. Click
   **Start cloud**.

   Result: The cloud card shows the Docker whale in place of the cloud mark,
   and the cloud becomes ready.

### 6.8 D8: The cloud card on a host

1. Open **Manage** on the card.

   Result: **Rebuild image & restart…** is off, with "Rebuild is not supported
   on a Docker host yet".

2. Stop, resume and delete the cloud with the card.

   Result: Each step ends without an error. After the delete, the counts of
   setup step 6 are the same again.

### 6.9 D9: Agents

1. Ask `cloud_offers` with `{"min_vcpu": 2}` from an agent panel in the
   isolated Horizon.

   Result: `other_providers` has a section with `provider: docker-host`, an
   offer with `id: this-pc` and `estimated_total: 0`.

2. Run `cargo test -p horizon-core --lib
   companions::lifecycle::tests::a_checked_companion_binds_to_the_checkout_its_deployment_records`.

   Result: The test passes. It binds a companion whose cloud record names a
   Docker host, first as a new binding and then as a binding that already
   exists, and both times gets `cloud_companion_unsupported`. This procedure
   does not make a live companion on a host: the
   [companion clouds procedure](companion-clouds.md) needs two rented clouds.

## 7. Pass criteria

- Tasks D1, D2, D4, D5, D6, D7, D8 and D9 pass.
- Task D3 passes, and SSH reaches the worker through the host with the pinned
  worker host key.

## 8. Cleanup

> **CAUTION:** DELETE ONLY WHAT THIS RUN MADE.
> These steps remove containers, volumes and an image. Make sure that each one
> carries a test cloud ID of this run or was pulled by this run.

1. Delete each test cloud that task D4 did not delete.

   Result: No container or volume with the test cloud IDs remains.

2. On a remote host, remove the worker image if this run pulled it, and
   remove `~/.horizon/docker-config` if this run made it.

   Result: The host has its state from before the run.

3. Remove `$T`.

   Result: The test files are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
