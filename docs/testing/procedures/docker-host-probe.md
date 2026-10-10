---
procedure: docker-host-probe
feature: Existing Docker engine admission
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Docker host probe test procedure

## 1. Purpose

This procedure tests the read-only core library probe for an existing Docker engine.
It tests missing requirements, SSH trust, cancellation and storage admission.

## 2. Applicability

- Candidate: a source build with `cloud_runtime::docker_host`.
- Platforms: Linux and macOS controllers. Windows returns a platform blocker.
- This procedure does not test deployment, saved settings, UI or MCP operations.
- Docker Desktop and forwarded Docker contexts do not pass admission.

## 3. Equipment and preconditions

- The repository build prerequisites from `AGENTS.md`.
- OpenSSH on Linux and macOS.
- For the optional live lane, an authorized Linux host with Tailscale SSH access.
- For a successful engine probe, stable Docker Engine 28 or later and measurable free space on its native Linux storage filesystem.
- The Linux engine host must have `uname` and a `stat` tool with filesystem format support for `%a` and `%S`.

## 4. Setup

1. Select the exact candidate checkout.

   Result: `git status` identifies the source under test.

## 5. Tasks

### 5.1 DHP-BASE — Baseline and missing requirements

1. Run the focused tests.

   ```sh
   cargo test -p horizon-core cloud_runtime::docker_host
   ```

   Result: All focused tests pass. Missing credentials produce a blocker with a remedy. A malformed or prerelease engine version cannot pass admission. Zero CPU, memory or free storage blocks admission with or without a selected profile. Unknown architectures block admission even when emulation is selected. ARM64 needs explicit emulation and deployment validation.

### 5.2 DHP-FEATURE — Optional cloud support

1. Run `cargo test -p horizon-core --no-default-features --test docker_host_probe_live`.

   Result: The integration test compiles and has no tests when cloud support is disabled.

### 5.3 DHP-TRUST — Connection identity and compatibility

1. Examine the focused test results for binding and SSH trust cases.

   Result: Invalid hosts, users and credential paths fail validation. Valid Docker context names can contain dots. Key authentication remains the default for older JSON bindings.

2. Examine the Tailscale trust test results.

   Result: Unknown hosts, ambiguous names, expired keys and absent host keys block the probe before SSH. A signed-out client receives a sign-in remedy when its peer map is null or absent.

### 5.4 DHP-STORAGE — Admission and cancellation

1. Examine the storage and cancellation test results.

   Result: Docker Desktop, absent engine metadata and unmeasurable storage cannot pass admission. Cancellation returns an error.

2. Examine the storage transport command.

   Result: `stat -f -c '%a %S'` reports available blocks and the fundamental block size. The output has no source or mount paths. The probe does not read container files or list the engine's storage directory. A directory listing permission is not required. See the [filesystem format reference](https://uutils.org/coreutils/docs/utils/stat.html).

3. Examine the host-kernel and storage-path regression tests.

   Result: A different host operating system or kernel blocks storage admission. A missing kernel is unknown. Invalid storage paths fail before execution. A filesystem query accepts spaces and percent signs in its path. Invalid or overflowing capacity output is unknown.

### 5.5 DHP-IMAGE — Built worker image

1. Examine the built-image regression test result.

   Result: The unchanged pinned image passes its contract check. A build recipe retains the warning for deployment validation.

### 5.6 DHP-LIVE — Optional authorized Tailscale probe

1. If live access is authorized, create a private JSON binding outside the repository.

   ```json
   {
     "id": "test-host",
     "name": "Test host",
     "ssh": {
       "host": "authorized-host",
       "user": "authorized-user",
       "port": 22,
       "authentication": "tailscale"
     }
   }
   ```

   Result: The binding uses an authorized host and Linux account. It contains no private key or auth key.

2. Set `HORIZON_DOCKER_READ_ONLY_HOST` to the private binding path.

   Result: The test reads only that selected binding.

3. Run the live probe.

   ```sh
   cargo test -p horizon-core --test docker_host_probe_live -- --ignored --nocapture
   ```

   Result: The test prints a report. Missing host requirements have remedies. The probe creates no container or volume.

## 6. Pass criteria

- All focused tests pass on the candidate.
- Every missing requirement blocks admission or reports an unknown result.
- SSH keeps pinned host keys unchanged and does not request a password.
- The optional live lane reports current evidence without a software or access change.
- No visual test applies because this change has no UI.

## 7. Cleanup

1. If you ran the live lane, unset `HORIZON_DOCKER_READ_ONLY_HOST`.

   Result: Later test runs do not select that host.

## 8. Record of results

Record the candidate commit, platform, test counts and optional live result in the pull request.
Keep host names, Linux usernames and private logs outside the repository.
