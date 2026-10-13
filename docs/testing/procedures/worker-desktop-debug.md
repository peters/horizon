---
procedure: worker-desktop-debug
feature: Debug Horizon on the cloud worker desktop (serve.py --worker-desktop)
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [Hetzner API token file, SSH identity file]
owner: peters
---

# Worker desktop debug test procedure

## 1. Purpose

This procedure makes sure that `serve.py --worker-desktop` builds and starts a
debug Horizon on the desktop of a real cloud worker. It also makes sure of
these items:

- The low build priority keeps the browser panels of the cloud usable.
- The debug Horizon writes nothing in `/workspace/home`, also not in
  `/workspace/home/.horizon`.
- The debug Horizon has no worker identity variables.
- **Add desktop viewer** shows the debug Horizon, and the stop is clean.

## 2. Applicability

- Candidate: each change to `scripts/device-smoke/worker_desktop.py`,
  `scripts/device-smoke/x11_windows.py`, the `--worker-desktop` branch of
  `serve.py`, the `HORIZON_HOME` override or `HORIZON_AGENT_INTEGRATIONS`.
- Platforms: a Hetzner quick start cloud, watched from a client Horizon in the
  local fixture.
- This procedure does not test: a RunPod worker, a GPU worker or a worker image
  with a Rust toolchain. Task W02 installs the toolchain on the quick start worker.

## 3. Safety

> **CAUTION:** TASKS W01 TO W08 RENT A HETZNER SERVER AND A VOLUME. Hetzner
> bills them until you delete them in W09.

> **CAUTION:** DELETE ONLY THE CLOUD THAT THIS RUN MADE. Other clouds in the
> same project can hold the work of other people.

> **CAUTION:** DO NOT SHOW THE SETTINGS FILE, THE TOKEN FILE OR THE SSH KEY IN
> EVIDENCE. Do not put the server address in public evidence.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  started as a persistent launcher without `--ephemeral`, as in
  [S02](cloud-panels/s-test-fixture.md#62-s02--start-the-persistent-launcher).
- A Device panel that shows a live view of the fixture.
- A client candidate in the fixture, with a cloud settings file that has a
  Hetzner token file, the server type `cx43` and an SSH identity file.
- A synthetic repository without `.horizon`, for example a clone of
  `https://github.com/octocat/Hello-World`, in the private home of the fixture.
- The candidate worktree, `<worktree>`, of the Horizon repository with its Git
  LFS files.
- A private evidence directory, `<evidence>`, and a private resource ledger.

These names apply in the steps:

| Name | Value |
|---|---|
| `<worker>` | The public address of the server, from the deployment record. |
| `<identity>` | The SSH identity file of the cloud settings. |
| `<tree>` | The copy of the candidate on the worker, `/workspace/worktrees/horizon-candidate`. |
| `<state>` | The `state` value in the manifest of W04. |
| `<started>` | The `started_at` value in the manifest of W04. |

## 5. Setup

1. Start the persistent launcher and the Device panel.

   Result: The Device panel shows the client candidate.

## 6. Tasks

### 6.1 W01: Start a quick start cloud

1. In the client, open **Cloud**, then **New cloud**. Select the synthetic
   repository, **Quick start** and Hetzner. Start the cloud.

   Result: The cloud card shows **Ready**. Write the server in the ledger.

### 6.2 W02: Prepare the worker

1. Install the build packages and Rust as root.

   ```sh
   ssh -i <identity> root@<worker> 'apt-get update -qq && apt-get install -y -qq build-essential pkg-config libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libvulkan-dev libgl-dev cmake nasm && curl -fsSL https://sh.rustup.rs | RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo sh -s -- -y --profile minimal --no-modify-path && chmod -R a+rX /usr/local/rustup /usr/local/cargo'
   ```

   Result: `/usr/local/cargo/bin/cargo` exists.

2. Give the sessions the toolchain.

   ```sh
   ssh -i <identity> root@<worker> 'printf "PATH_PREPEND=/usr/local/cargo/bin\nRUSTUP_HOME=/usr/local/rustup\nCARGO_HOME=/workspace/home/.cargo\n" > /etc/horizon-worker/session-env.d/20-rust.env && horizon-worker-session-env apply > /dev/null && echo valid'
   ```

   Result: The output is `valid`.

3. Copy the candidate worktree without `target` and `.git` to `<tree>`, owned by
   the agent account.

   ```sh
   tar -C <worktree> --exclude=./target --exclude=./.git -czf - . | ssh -i <identity> root@<worker> 'mkdir -p <tree> && tar -C <tree> -xzf - && chown -R 10001:10001 <tree>'
   ```

   Result: `<tree>/scripts/device-smoke/serve.py` exists on the worker.

4. Record the state of the browser service's root.

   ```sh
   ssh -i <identity> root@<worker> 'find /workspace/home/.horizon -type f | sort | xargs sha256sum' > <evidence>/horizon-root-before.txt
   ```

   Result: The file lists the files of `/workspace/home/.horizon`.

### 6.3 W03: Measure the browser panels before and during the build

1. On the cloud card, add a Shell panel. In it, start the browser probe of the
   evidence directory. The probe uses `horizon-browser mcp --connect`. It opens
   a browser panel and measures `browser_navigate` and `browser_snapshot` every
   two seconds.

   Result: The probe writes one line for each measurement.

2. Let the probe run for 60 seconds before the build.

   Result: The baseline shows the normal times.

### 6.4 W04: Start the worker desktop mode

1. On the cloud card, add a second Shell panel. In it, start the command.

   ```sh
   cd <tree> && python3 scripts/device-smoke/serve.py --worker-desktop
   ```

   Result: The build starts. The probe continues. Most times stay near the
   baseline. While the build writes to the workspace volume, some actions can
   stop for some seconds, and one or two can fail with `host coordination timed
   out`.

2. Examine the priority of the build.

   ```sh
   ps -o pid,ni,cls,cmd -C rustc | head
   ```

   Result: `NI` is `19` and `CLS` is `IDL`.

3. Wait for the manifest line.

   Result: The Shell panel shows one JSON line. `window` is `true`. `build`
   names `$HORIZON_SESSION_DIR/target/device-smoke`.

4. Stop the probe and keep its output in `<evidence>`.

   Result: The probe output covers the baseline and the full build.

### 6.5 W05: Examine the debug Horizon

1. Compare the hash of the running executable with the manifest.

   ```sh
   sha256sum /proc/<pid>/exe; cat <state>/bin/SHA256SUMS
   ```

   Result: The two hashes are the same.

2. Examine the environment of the debug Horizon.

   ```sh
   tr '\0' '\n' < /proc/<pid>/environ | cut -d= -f1 | grep -E '^(HORIZON|ANTHROPIC|WAYLAND|HOME|DISPLAY)' | sort
   ```

   Result: The names are `DISPLAY`, `HOME`, `HORIZON_AGENT_INTEGRATIONS`,
   `HORIZON_HOME` and `HORIZON_SESSION_DIR`. No worker identity name shows.

3. Examine the value of the switch.

   ```sh
   tr '\0' '\n' < /proc/<pid>/environ | grep '^HORIZON_AGENT_INTEGRATIONS='
   ```

   Result: The output is `HORIZON_AGENT_INTEGRATIONS=off`.

### 6.6 W06: Take the screenshots

1. Call the MCP tool `device_screenshot` of the `horizon-device` server in
   `/workspace/agent-mcp.json`. Its target is `cloud-desktop`.

   Result: The image shows the debug Horizon window.

2. On the cloud card, choose **Add desktop viewer**.

   Result: The viewer in the client shows the debug Horizon window. Clicks in
   the viewer do nothing, because the cloud desktop is read-only.

### 6.7 W07: Make sure that the debug Horizon wrote nothing in the home

1. Look for changed files in the worker home.

   ```sh
   ssh -i <identity> root@<worker> 'find /workspace/home -newermt "<started>"'
   ```

   Result: No path in the output comes from the debug Horizon: no Horizon
   skill, plugin, lease, MCP registration or desktop entry, and no file in
   `/workspace/home/.horizon`. Files of the Shell panels, such as the shell
   history, can show.

2. Compare the files of `/workspace/home/.horizon` with W02 step 4.

   Result: The list and the hashes are the same.

### 6.8 W08: Stop the debug Horizon

1. Press Ctrl-C in the second Shell panel.

   Result: The debug Horizon window closes in less than 15 seconds. The command
   ends. No process from `<state>/bin/horizon` runs.

### 6.9 W09: Delete the cloud

> **CAUTION:** DELETE ONLY THE CLOUD OF THIS RUN.

1. On the cloud card, open **Manage** and choose **Delete cloud resources**.

   Result: The card shows that the cloud is deleted.

2. List the servers, volumes, SSH keys and networks of the Hetzner project.

   Result: No resource of this run shows.

## 7. Pass criteria

- The build runs at nice 19 in the idle classes, in `$HORIZON_SESSION_DIR`.
- During the build, most browser probe times stay near the baseline. Stops of
  some seconds while the build writes to the volume are known.
- The running executable has the frozen hash. No worker identity variable shows.
- `device_screenshot` and **Add desktop viewer** show the debug Horizon.
- The debug Horizon wrote nothing in `/workspace/home`.
- Ctrl-C closes the window and ends the command.
- No resource of the run stays in the Hetzner project.

## 8. Cleanup

1. Stop the persistent launcher with Ctrl-C.

   Result: The fixture removes its private data.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
