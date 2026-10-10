---
procedure: cloud-docker-restart
feature: Restart Docker from a cloud failure
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Restart Docker from a cloud failure test procedure

## 1. Purpose

This procedure proves that a cloud failure from a stuck or stopped Docker
names Docker as the cause. It also proves that the failure box offers
Restart Docker, asks before it restarts, and then offers Retry.

## 2. Applicability

- Candidate: a debug build of the pull request head.
- Platforms: Linux with X11, in the isolated fixture of
  [the device smoke runbook](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel).
- This procedure does not test: a restart of a real Docker daemon, the macOS
  and Windows restart commands, and `pkexec` password prompts. Unit tests in
  `docker_daemon/plan.rs` cover the platform decision for each setup.
  Unit tests in `cards/docker.rs` cover Close while a restart runs.

## 3. Safety

> **CAUTION:** DO NOT RESTART THE DOCKER DAEMON OF THIS COMPUTER. Other agents
> and the person use it. Use only the stand-in `docker`, `systemctl` and
> `pkexec` programs of step 5.2, which never call the real programs.

## 4. Equipment and preconditions

- The fixture prerequisites of the device smoke runbook: Xvfb, openbox, bwrap,
  dbus-daemon and x11vnc.
- A Horizon host that shows a native Device panel in your workspace.
- `ffmpeg` and ImageMagick `import` to record the GIF.

## 5. Setup

1. Build the candidate and copy it outside your home directory:
   `cargo build -p horizon-ui`, then copy `target/debug/horizon` to
   `/tmp/horizon-smoke-bin.<task>/horizon`.

   Result: `sha256sum` gives the hash to record with the evidence.

2. Make a tools root at `/tmp/horizon-<task>-tools/root`. Unpack x11vnc into it
   if the computer has no x11vnc. Put three stand-in programs in
   `root/usr/bin`:

   - `docker` answers `--version` and `context inspect` with a rootless socket
     at `$XDG_RUNTIME_DIR/docker.sock`. It waits without end for each other
     command until the file `/tmp/fake-docker/restarted` exists. After that, it
     answers `version` with `29.8.1`.
   - `systemctl` answers `active` for `--user show ... docker.service`. For
     `--user restart docker.service`, it waits 4 seconds and makes the file
     `/tmp/fake-docker/restarted`.
   - `pkexec` writes its arguments to a log and refuses.

   Each program writes its arguments to `/tmp/fake-docker/calls.log`.

   Result: `root/usr/bin` contains `docker`, `systemctl`, `pkexec` and
   `x11vnc`.

3. Start the fixture with the tools root and a failed cloud:
   `HORIZON_CLOUD_FAILURE_PREVIEW=1 python3 scripts/device-smoke/serve.py --horizon <binary> --tools /tmp/horizon-<task>-tools/root --native-view --state /tmp/horizon-<task>-state`.

   Result: The manifest gives `vnc_address`. The fixture shows two synthetic
   failed clouds.

4. Create a Device panel on `vnc_address` with the public `device_panel` tool.
   Examine `connection`, `image_displayed` and an advancing `frame_sequence`.

   Result: The panel shows the fixture desktop live.

## 6. Tasks

### 6.1 DR-1 — The failure names a stuck Docker

1. Open the failed cloud without panels.

   Result: The failure box shows the Docker conflict line. Its meaning says
   that Docker itself may be stuck and to restart Docker. The row of actions
   shows Retry, Copy error and Restart Docker….

### 6.2 DR-2 — The question comes before a restart

1. Click Restart Docker….

   Result: A spinner says that Horizon checks how Docker runs. Then a red
   question says that a restart stops every container in this Docker and
   that Horizon runs `systemctl --user restart docker`.

2. Click Cancel.

   Result: The question closes. `calls.log` has no `systemctl --user restart`
   line.

### 6.3 DR-3 — A confirmed restart reports Docker again

1. Click Restart Docker…, then click Restart Docker in the question.

   Result: A spinner shows the command and the elapsed seconds. After about 4
   seconds it says that Horizon waits for Docker to answer.

2. Wait.

   Result: The box says "Docker 29.8.1 answers again." It shows the cloud's
   Retry button and Close. `calls.log` has one
   `systemctl --user restart docker.service` line and no `pkexec` line.

3. Click the Retry button under the result.

   Result: The cloud starts its retry. The result closes.

### 6.4 DR-4 — The other card shows the same restart

1. Click Restart Docker… on one failed cloud. Look at the other failed cloud.

   Result: Both cards show the same question or progress. Neither card offers
   a second Restart Docker… while the first is open.

## 7. Pass criteria

- Each task gives the result that it states.
- `calls.log` has no line from a real Docker client and no `pkexec` line.
- The real Docker of the computer answers `docker version` after the run,
  with the same uptime as before the run.

## 8. Cleanup

1. Close the Device panel with `device_panel`.

   Result: The panel leaves the workspace.

2. Stop the fixture with Ctrl-C in its terminal.

   Result: The fixture stops its Xvfb, window manager and Horizon.

3. Remove the tools root, the state directory and the frozen binary.

   Result: No task file stays in `/tmp`.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
