---
procedure: registry-docker-context
feature: Preserve Docker daemon selection with private registry credentials
platforms: [linux, macos]
cost: none
destructive: no
secrets: synthetic registry credentials only
owner: peters
---

# Registry Docker context test procedure

## 1. Purpose

Check that a private registry login keeps the selected Docker daemon and plugins.
Check that it excludes other registry logins, credential helpers and proxy credentials.

## 2. Applicability

Use a Unix host with a non-default Docker context, such as rootless Docker.
This procedure tests local image preparation. It rents no compute and changes no installed login.
Windows compiles the helper but does not use the Unix context links or durable cloud journals.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC CREDENTIALS IN AN OWNED REGISTRY FIXTURE.
> Do not change the user's Docker configuration, trust store or daemon settings.

Keep fixture state, temporary trust and registry data separate from active applications.

## 4. Equipment and preconditions

- Candidate source and its supported Rust toolchain.
- A working non-default Docker context on Linux or macOS.
- An owned local registry with separate writer and read-only accounts.
- A committed fixture repository with a small worker Dockerfile.

## 5. Setup

1. Record the candidate and the selected context endpoint.

   ```sh
   docker context inspect --format '{{.Endpoints.docker.Host}}'
   ```

   Result: The endpoint differs from the default socket.
   Keep this context active. Do not set an explicit host that hides the context behavior.

## 6. Tasks

### 6.1 DC-01 — Isolate credentials and retain connection settings

1. Run the registry context regression.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib isolated_registry_config_preserves_the_daemon_context_without_other_credentials
   ```

   Result: Subprocesses use absolute and relative paths to their synthetic Docker configuration.
   The private registry configuration retains its context and plugin locations.
   It contains only the selected registry login.
   Credential helpers, unrelated logins and proxy credentials stay out.
   The original configuration is unchanged.
   Actual preflight subprocesses retain the context and plugins, use only empty
   authentication entries, and receive the selected pull and publishing secrets on stdin.

### 6.2 DC-02 — Prepare an image with the active context

> **CAUTION:** SEND ONLY THE FIXTURE'S SYNTHETIC LOGINS TO ITS REGISTRY.
> Keep temporary trust scoped to these test processes.

1. Configure a private fixture settings file and an explicit registry-bound build profile.

   Result: The profile uses the fixture registry.
   The settings contain dedicated writer and read-only grants and no explicit Docker host.
   If the current provider journal requires a compute key, use a synthetic local key.
   No step uses `deploy` or contacts a compute API.

2. Run `cloud_deploy plan-deploy`, then its `prepare-image` command with the printed plan token.

   Result: The image builds on the active context, passes its worker contract, pushes
   with the writer and verifies its immutable digest with the read-only login.
   It does not try the default socket or use an unrelated registry login.

3. Run the applicable full local matrix.

   Result: Blocking and strict checks pass. Record advisory results separately.
   Hosted checks pass on the exact head.

## 7. Pass criteria

- The selected daemon and plugin locations survive credential isolation.
- Only the selected registry credential enters each private configuration.
- The real local build, push and immutable-image check pass.
- Installed Docker configuration and credentials remain unchanged.

## 8. Cleanup

> **CAUTION:** REMOVE ONLY THE RESOURCES OWNED BY THIS FIXTURE.
> Do not use global image, container or volume cleanup.

1. Remove only the fixture's containers, volume, image tags, temporary passwords and private key.

   Result: The owned resources are absent. The original context still works.

## 9. Record of results

Record the candidate, binary hash, commands, results and cleanup in the pull request.
Keep credentials and private operational details out of the evidence.
State that this proves local Docker behavior, not a live account or compute allocation.
