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
- This procedure does not test: the Cloud settings card, the New cloud
  provider choice or the MCP tools. These come in a later change. Until
  then, you add a host to `docker_hosts` in the cloud settings file, and New
  cloud shows the Docker mark only for a profile with `provider: docker-host`.
  Task D6 examines that mark.

## 3. Safety

> **CAUTION:** DO NOT USE THE DEFAULT DOCKER CONFIGURATION OVER SSH ON A MAC.
> A keychain prompt over SSH stops that Mac until a person closes it. Horizon
> uses a private configuration; use one for manual commands too.

> **CAUTION:** DELETE ONLY THE TEST CLOUDS.
> The delete step removes a container and a volume on the host. Use test
> cloud IDs that no other cloud uses.

## 4. Equipment and preconditions

- A Linux computer with Docker and Git, and the Horizon checkout.
- For task D3: a remote host that `ssh <host> true` reaches without a
  password prompt. On that host, the SSH user has access to Docker, and
  `docker` is at a known absolute path.
- Network access to `ghcr.io` to pull the public worker base image.

## 5. Setup

1. Set `CARGO_TARGET_DIR` to a directory in the checkout's worktree target and
   `TMPDIR` to a private directory with mode 700. Do not use `/tmp`.

   Result: The builds and the test files go to these directories.

2. Run `cargo build -p horizon-core --example cloud_deploy`.

   Result: The `cloud_deploy` example is built.

3. Make a test directory `$T`. Make a Git repository in `$T/repo` with one
   commit. Put this text in `$T/repo/.horizon/cloud.yml` before the commit:

   ```yaml
   version: 1
   default: local
   profiles:
     local:
       provider: docker-host
       image: ghcr.io/peters/horizon-worker-base@sha256:f28a0ceb76b559b299000b440a26951bc70a820003d3911c4e130174bef944ec
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
   `cpu_flavors: ["cpu3c"]`, `gpu_types: []`, and this host list:

   ```json
   "docker_hosts": [{"name": "this-pc", "access": "this-pc"}]
   ```

   Result: The settings name this computer as the only Docker host.

6. Write down the number of containers and volumes on the host:
   `docker ps -aq | wc -l` and `docker volume ls -q | wc -l`.

   Result: You have the baseline counts.

## 6. Tasks

In each command, `cloud_deploy` is the example from setup step 2.

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

2. Run `cloud_deploy deploy $T/settings-remote.json $T/repo local $T/state/remote test-remote`.

   Result: The cloud is ready. An ARM host runs the arm64 build of the worker
   image, and `uname -m` in the worker shows `aarch64`.

3. Run `cloud_deploy stop`, `reconcile`, `resume` and `reconcile` with
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

   Result: The counts are the same as the baseline counts from setup step 6.
   No container or volume with the label `io.horizon.cloud` remains for the
   test clouds.

### 6.5 D5: Automatic tests

1. Run `cargo test -p horizon-cloud docker_host`.

   Result: All tests pass. They use a simulated Docker engine.

2. Run `cargo test -p horizon-core --test docker_host_live -- --ignored`.

   Result: The test passes and removes its two clouds.

### 6.6 D6: The Docker mark in New cloud

1. Start the candidate in an isolated Horizon desktop that a native VNC Device
   panel shows, with the settings of setup step 5 and `$T/repo` as the
   repository.

   Result: Horizon starts on the isolated desktop.

2. Open **New cloud** for `$T/repo` and select the profile `local`.

   Result: The provider mark is the blue Docker whale, sharp at the dialog's
   size, in the light and the dark theme. The checks show "Docker host saved;
   nothing is rented". **Start cloud** stays off with "New cloud cannot start a
   cloud on a Docker host yet.", also when a rented provider's offer is chosen.

## 7. Pass criteria

- Tasks D1, D2, D4, D5 and D6 pass.
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
