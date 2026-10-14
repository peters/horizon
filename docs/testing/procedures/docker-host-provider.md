---
procedure: docker-host-provider
feature: Clouds on an existing Docker host (provider module)
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Docker host provider test procedure

## 1. Purpose

This procedure proves that the `horizon-cloud` Docker host module creates,
stops, starts and deletes a worker container on a Docker engine, keeps the
worker's SSH port the same, and changes only the container and volume of its
own cloud.

## 2. Applicability

- Candidate: a Horizon checkout that has `horizon_cloud::docker_host`.
- Platforms: Linux and macOS, with a simulated Docker engine.
- This procedure does not test: deployment of a cloud, the cloud card, the
  Cloud settings card, the New cloud provider choice or the MCP tools. Nothing
  calls the module yet. A later change connects it and extends this
  procedure with live tasks on this computer and on a remote host.

## 3. Safety

No step rents compute, deletes data, sends a secret or changes access. The
tests use a simulated Docker engine and private temporary directories.

## 4. Equipment and preconditions

- The Horizon checkout and a Rust toolchain.
- A POSIX `sh` and `/bin/sleep` for the two Unix-only tests.

## 5. Setup

1. Set `CARGO_TARGET_DIR` to a directory in the checkout's worktree target and
   `TMPDIR` to a private directory with mode 700. Do not use `/tmp`.

   Result: The builds and the test files go to these directories.

## 6. Tasks

### 6.1 M1: Lifecycle on a simulated engine

1. Run `cargo test -p horizon-cloud docker_host`.

   Result: All tests pass. Among them:
   - two clouds on one host get their own names, volumes and SSH ports, also
     when another worker holds the port that a cloud tries first;
   - the SSH port stays the same after a stop and a start, and a start fails
     with a clear message when another program took the port;
   - a host without free CPU or memory refuses before anything is created;
   - a delete removes only the cloud's own container and volume, and a volume
     with the cloud's name that Horizon did not create is never used;
   - a request that runs out of time is reported, and a cloud whose delete
     started does not start again.

### 6.2 M2: Commands and quoting

1. Examine the result of the same command for these tests:
   `host_settings_refuse_shell_syntax`, `workers_are_reached_through_the_host`,
   `remote_arguments_reach_docker_unchanged` and
   `a_command_that_outlives_its_time_is_stopped`.

   Result: Each test passes. Shell syntax in a host setting is refused, each
   remote argument reaches docker unchanged, the private Docker configuration
   is written, and a command that outlives its time is stopped.

## 7. Pass criteria

- Tasks M1 and M2 pass.

## 8. Cleanup

1. Remove the private `TMPDIR` directory if you made it for this run.

   Result: No test files remain.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
