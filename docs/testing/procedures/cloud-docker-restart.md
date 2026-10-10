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
  Unit tests in `cards/docker.rs` cover Close while a restart runs, and that a
  container name already in use offers no restart.

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
   if the computer has no x11vnc. Make the three stand-in programs in
   `root/usr/bin` with the commands below. They keep their files in
   `<state>/fake-docker`, because the fixture gives Horizon a private `/tmp`
   but shows the state directory at its own path. They read that directory
   from `FAKE_DOCKER_DIR`, which Horizon passes on to the programs it starts.
   Use the absolute path of `<state>`.

   ```bash
   bin=/tmp/horizon-<task>-tools/root/usr/bin
   export FAKE_DOCKER_DIR=<state>/fake-docker
   mkdir -p "$bin" "$FAKE_DOCKER_DIR"
   cat > "$bin/docker" <<'EOF'
   #!/bin/sh
   dir=${FAKE_DOCKER_DIR:?}
   echo "docker $*" >> "$dir/calls.log"
   while [ "$1" = --config ] || [ "$1" = --host ]; do shift 2; done
   case "$1 $2" in
     "--version "*) echo "Docker version 29.8.1, build stand-in" ;;
     "context inspect") printf 'rootless\tunix://%s/docker.sock\n' "$XDG_RUNTIME_DIR" ;;
     "desktop version") exit 1 ;;
     *)
       until [ -e "$dir/restarted" ]; do sleep 1; done
       if [ "$1" = version ]; then echo 29.8.1; fi ;;
   esac
   EOF
   cat > "$bin/systemctl" <<'EOF'
   #!/bin/sh
   dir=${FAKE_DOCKER_DIR:?}
   echo "systemctl $*" >> "$dir/calls.log"
   case "$*" in
     "--user show --property=ActiveState --value docker.service") echo active ;;
     "--user restart docker.service") sleep 4; touch "$dir/restarted" ;;
     *) exit 1 ;;
   esac
   EOF
   cat > "$bin/pkexec" <<'EOF'
   #!/bin/sh
   echo "pkexec $*" >> "${FAKE_DOCKER_DIR:?}/calls.log"
   exit 126
   EOF
   chmod 755 "$bin/docker" "$bin/systemctl" "$bin/pkexec"
   ```

   - `docker` skips leading `--config` and `--host` options. It answers
     `--version`, and `context inspect` with a rootless socket at
     `$XDG_RUNTIME_DIR/docker.sock`, and fails `desktop version`. It waits
     without end for each other command until the file `restarted` exists.
     After that, it answers `version` with `29.8.1`.
   - `systemctl` answers `active` only for the user unit. For
     `--user restart docker.service`, it waits 4 seconds and makes the file
     `restarted`. The system unit shows as not running.
   - `pkexec` writes its arguments to the log and refuses.

   Each program writes its arguments to `<state>/fake-docker/calls.log`. None
   of them calls a real program.

   Result: `root/usr/bin` contains `docker`, `systemctl`, `pkexec` and
   `x11vnc`.

3. Start the fixture with the tools root and the stuck-Docker preview of a
   debug build, with `DOCKER_HOST` unset, as Horizon honors it as the Docker CLI
   does. Run it in the shell of step 2, which exports `FAKE_DOCKER_DIR`:
   `env -u DOCKER_HOST HORIZON_CLOUD_DOCKER_STUCK_PREVIEW=1 python3 scripts/device-smoke/serve.py --horizon <binary> --tools /tmp/horizon-<task>-tools/root --native-view --state <state>`.
   The preview adds two synthetic clouds whose image build failed, each with
   `Docker did not answer docker version within 5 s` as its last output line.
   No record binds a worker, so no provider is asked.

   Result: The manifest gives `vnc_address`. The fixture shows two synthetic
   failed clouds named Synthetic stuck Docker.

4. Write synthetic cloud settings to
   `<state>/data/home/.horizon/cloud/settings.json` with mode `0600`, as in
   [the cloud settings procedure](cloud-settings-replace-key.md). The file
   paths in it need not exist.

   Result: Restart Docker… can read which Docker the cloud uses.

5. Create a Device panel on `vnc_address` with the public `device_panel` tool.
   Examine `connection`, `image_displayed` and an advancing `frame_sequence`.

   Result: The panel shows the fixture desktop live.

## 6. Tasks

### 6.1 DR-1 — The failure names a stuck Docker

1. Open one of the failed clouds.

   Result: The failure box shows the line that Docker did not answer. Its
   meaning says that Docker stopped answering and to restart Docker. The row
   of actions shows Retry and Copy error, and Restart Docker… on its own row.

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
