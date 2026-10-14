---
applyTo: "crates/horizon-cloud/src/docker_host.rs,crates/horizon-cloud/src/docker_host/**,crates/horizon-core/src/cloud_runtime/providers/docker_host.rs,crates/horizon-core/src/cloud_runtime/ssh.rs"
---

# Trust boundary of the Docker host provider

`horizon_cloud::docker_host` runs clouds on a Docker engine that the person
already has: this computer's engine, or a host that Horizon reaches with
`ssh` or `tailscale ssh`. `cloud_runtime::providers::docker_host` connects it
to the deployment, the cloud lifecycle and the worker SSH connection.

## What is trusted

- The saved host. The person has SSH access to it and Docker access there.
  Horizon stores no secret for it: SSH uses the person's own configuration,
  keys and agent.
- The docker CLI and the engine on that host. Their output is parsed as data
  only. A container or volume that does not carry the cloud's owner label and
  its recorded name is never changed or used.

## What Horizon guarantees

- Every argument of a remote docker command is quoted as one POSIX shell word
  and runs under `sh -c`, whatever the login shell of the host is. Host names,
  SSH addresses and docker paths are validated before any command is built, so
  they cannot become options or shell syntax.
- Horizon never uses `docker -H ssh://` and never opens the Docker API on a
  network. The remote CLI talks to `unix:///var/run/docker.sock` on the host.
- The docker CLI uses a private configuration with one placeholder login, on
  every host. A configuration without a login makes the CLI use a credential
  helper, such as the macOS keychain, and a keychain prompt over SSH waits for
  a person at the host.
- A worker is created or started only on a stable Docker Engine 28 or later,
  which keeps ports published on the loopback private. Earlier engines let
  hosts on the same network segment reach them.
- The worker publishes its SSH only on the host's loopback, on a port that
  stays the same for the life of the container. Horizon reaches it through
  the host with an SSH `ProxyCommand`, and pins the worker host key as for
  other providers.
- A cloud records the saved host and the access and address it was created
  through. A host that is removed, or saved again under the name for another
  engine, is refused for that cloud's lifecycle, so a delete never reports
  success from an engine that never had the cloud.
- Each docker call has a time limit and stops when the operation is cancelled.
- A delete removes only the cloud's container and workspace volume, each with
  the cloud's owner label and recorded name, and examines that both are gone.

## What Horizon does not guarantee

- The worker's limits are Docker limits. `--cpus` is a quota, so the worker
  sees all CPUs of the host. A Docker volume has no size, so the profile's
  volume size is not enforced and disk space is not checked.
- The capacity check reads the free CPU and memory once, at create time.
  Clouds that are created at the same time on one host can overcommit it.
- The host pulls only public images: its docker CLI has no registry login, and
  Horizon refuses a profile with a build or a private registry binding.
- A person or program with Docker access on the host can read, change or
  remove the worker and its volume.
