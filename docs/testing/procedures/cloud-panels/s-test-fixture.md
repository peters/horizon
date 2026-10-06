---
procedure: cloud-panels-s-test-fixture
feature: Cloud panels smoke test, area S (test fixture)
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud panels test procedure, area S: test fixture

## 1. Purpose

This area makes a frozen candidate and starts it in a persistent launcher. It
also makes sure that a Device panel shows a live view and that the candidate
child is the frozen candidate.

## 2. Applicability

- Candidate: a debug build of `<candidate-commit>`.
- Platforms: Linux with Xvfb.
- This area does not test: a cloud function. The other areas use the fixture
  that this area starts.

## 3. Safety

> **CAUTION:** DO NOT BIND THE REAL HOME DIRECTORY INTO THE FIXTURE. The private
> home of the fixture must contain only test files. Real agent logins and keys
> can show in screenshots.

> **CAUTION:** DO NOT PUT THE STATE DIRECTORY ON A TMPFS WITH A USER QUOTA. If
> the quota is full, the candidate can stop and lose the saved session.

## 4. Equipment and preconditions

- The equipment in the [main procedure](../cloud-panels.md#4-equipment-and-preconditions).
- A rootless Docker daemon that only this run uses, with the socket
  `<docker-socket>`, the data root `<docker-data>` and the process ID
  `<docker-pid>`. Do not use the Docker daemon of the operator.
- The [device smoke fixture](../../../../scripts/device-smoke/README.md) and the
  [persistent cloud launcher](../../cloud-workspaces-mvp-smoke.md#persistent-cloud-launcher-for-restart-scenarios)
  notes.
- A clean worktree of the repository.

## 5. Setup

1. Make the run directory on a disk file system outside `$HOME` and outside `/tmp`.

   ```sh
   mkdir -p <run>/bin <run>/launcher && chmod 700 <run>
   ```

   Result: The directory exists and only the operator can read it.

2. Examine the file system of the run directory.

   ```sh
   findmnt -T <run> -o TARGET,FSTYPE,OPTIONS
   ```

   Result: `FSTYPE` is not `tmpfs`. If it is `tmpfs`, use another directory.

## 6. Tasks

### 6.1 S01 — Build and freeze the candidate

1. Fetch the repository and check out the candidate commit in the worktree.

   ```sh
   git fetch origin && git checkout --detach <candidate-commit>
   ```

   Result: `git rev-parse HEAD` shows `<candidate-commit>`.

2. Set a Cargo target directory that only this worktree uses.

   ```sh
   export CARGO_TARGET_DIR="$PWD/target/cloud-smoke"
   ```

   Result: The builds of other worktrees cannot change the candidate.

3. Build the candidate, the device tool, the browser CLI and the `cloud_deploy` example.

   ```sh
   cargo build -p horizon-ui --bin horizon
   cargo build -p horizon-device --features cli
   cargo build -p horizon-browser-cli
   cargo build -p horizon-core --example cloud_deploy
   ```

   Result: The four builds finish without an error.

4. Copy the four executables to `<run>/bin`.

   ```sh
   cp "$CARGO_TARGET_DIR"/debug/{horizon,horizon-device,horizon-browser} "$CARGO_TARGET_DIR"/debug/examples/cloud_deploy <run>/bin/
   ```

   Result: `<run>/bin` contains `horizon`, `horizon-device`, `horizon-browser` and
   `cloud_deploy`.

5. Record the SHA-256 of each copy.

   ```sh
   (cd <run>/bin && sha256sum horizon horizon-device horizon-browser cloud_deploy > SHA256SUMS)
   ```

   Result: `SHA256SUMS` has four lines.

6. Record the commit and the SHA-256 of `horizon` in `<evidence>/candidate.json`.

   Result: The evidence names the frozen candidate.

### 6.2 S02 — Start the persistent launcher

1. Copy `serve.py` and `sandbox.py` from `scripts/device-smoke` to `<run>/launcher`.

   Result: The launcher copy is in a task-owned directory.

2. In the launcher copy, remove `--ephemeral` from the start command of Horizon.

   Result: The candidate keeps its saved session. A cloud needs a saved session.

3. In the launcher copy, use a configuration file below the private data of the fixture.

   Result: Each start of the candidate uses the same configuration.

   > **CAUTION:** BIND ONLY A ROOTLESS DOCKER SOCKET THAT THIS RUN OWNS. A process
   > in the fixture that can use the socket can read every file that the daemon
   > can read. Use a rootless daemon with its own data root, and run only the
   > frozen candidate and synthetic repositories in the fixture.

4. In the launcher copy, bind the socket of a rootless Docker daemon into the fixture.

   ```text
   <docker-socket>
   ```

   Result: The fixture can use the Docker daemon of this run. The fixture makes `/run/user/<uid>` private,
   so it does not show this socket without a bind. Area B sets `docker_host` to
   this socket.

5. In the launcher copy, replace the two lines `if application_done:` and `return` with this restart branch.

   ```python
               if application_done:
                   marker = args.state / 'restart-request'
                   if not marker.exists():
                       return
                   marker.unlink()
                   children.remove(application_process)
                   application_process = spawn('horizon', [str(app), '--config', str(config)])
   ```

   Result: If the file `<state>/restart-request` exists, the launcher deletes it
   and starts the candidate again. Xvfb, D-Bus, VNC and the private data continue.
   The [persistent launcher notes](../../cloud-workspaces-mvp-smoke.md#persistent-cloud-launcher-for-restart-scenarios)
   give the reasons for this branch.

6. Record the difference between the launcher copy and `scripts/device-smoke`.

   ```sh
   diff -u scripts/device-smoke/serve.py <run>/launcher/serve.py > <evidence>/launcher.diff
   ```

   Result: The evidence contains the launcher changes.

7. Do steps 1 to 4 of S05.

   Result: The launcher copy starts an unlocked keyring with the candidate.

8. Look for x11vnc on the host.

   ```sh
   command -v x11vnc
   ```

   Result: If the output is empty, the next step needs `--tools <tools-root>`
   with an unpacked tools root that contains x11vnc.

9. Start the persistent launcher with a new state directory.

   ```sh
   python3 <run>/launcher/serve.py --horizon <run>/bin/horizon \
     --native-view --state <run>/fixture [--tools <tools-root>]
   ```

   Result: The output shows a `vnc_address`. The fixture writes `lab.json` and
   `target.json` in `<state>`. If the start fails, use a new state directory for the next start.

### 6.3 S03 — Show the fixture in a Device panel

1. Read `vnc_address` from `<state>/lab.json`.

   Result: You have the VNC address of the fixture.

2. Send the `device_panel` operation `list`.

   Result: The answer lists the Device panels in your workspace.

3. Send the `device_panel` operation `create` with the VNC address.

   ```json
   {"operation":"create","endpoint":"<vnc_address>"}
   ```

   Result: The answer gives a panel ID.

4. Send the operation `inspect` for this panel ID three times, 2 seconds apart.

   Result: Each answer shows `connection: "connected"`, `image_received` and
   `image_displayed`. The `frame_sequence` value increases.

5. Record the UTC time and the values of each inspection in the evidence.

   Result: The evidence shows a live view.

### 6.4 S04 — Make sure that the frozen candidate runs

1. Find the process ID of the `horizon` child in the process tree of the fixture.

   ```sh
   pstree -p <launcher-pid>
   ```

   Result: You have the child process ID. The launcher process ID can belong to
   `bwrap`.

2. Show the executable of the child.

   ```sh
   readlink /proc/<child-pid>/exe
   ```

   Result: The path is `<run>/bin/horizon`.

3. Calculate the SHA-256 of the executable of the child.

   ```sh
   sha256sum /proc/<child-pid>/exe
   ```

   Result: The value is the same as the `horizon` line in `SHA256SUMS`.

4. Record the child process ID and the SHA-256 in the evidence.

   Result: The evidence connects the run to the frozen candidate.

### 6.5 S05 — Give the fixture a Secret Service

The candidate keeps tailnet auth keys in the Secret Service. The fixture has its
own D-Bus, so the keyring of the operator is not available. Do steps 1 to 4
before the first start of the launcher in S02 step 9. The launcher refuses a
state directory that exists when it starts. A restart through the restart marker
keeps the launcher and its state directory, so it needs no new keyring.

1. Make a synthetic password for the keyring of the fixture.

   ```sh
   head -c 24 /dev/urandom | base64 > <run>/keyring-password && chmod 600 <run>/keyring-password
   ```

   Result: The file contains a random password. Keep it private. It unlocks the
   keyring that later holds the test tailnet auth key. The main cleanup deletes it.

2. In the launcher copy, start `gnome-keyring-daemon` after the D-Bus daemon.

   ```text
   gnome-keyring-daemon --foreground --unlock --components=secrets
   ```

   Result: The launcher starts the keyring on the D-Bus of the fixture.

3. In the launcher copy, write the synthetic password to the standard input of the keyring.

   Result: The keyring reads the password and unlocks the login collection.

4. In the launcher copy, close the standard input of the keyring after the password.

   Result: The keyring continues to run.

5. Make sure that the keyring runs in the fixture.

   ```sh
   pstree -p <launcher-pid> | grep -o 'gnome-keyring-d([0-9]*)'
   ```

   Result: The output shows one keyring process below the launcher of the fixture.

6. In the fixture terminal, store a test value.

   ```sh
   printf smoke-value | secret-tool store --label=smoke smoke probe
   ```

   Result: The command stops without an error. The keyring does not ask for a password.

7. In the fixture terminal, read the test value.

   ```sh
   secret-tool lookup smoke probe
   ```

   Result: The output is `smoke-value`.

8. In the fixture terminal, delete the test value.

   ```sh
   secret-tool clear smoke probe
   ```

   Result: A new lookup shows no value.

9. Do S03 and S04 again.

   Result: The Device panel shows a live view. The child has the frozen SHA-256.

## 7. Pass criteria

- `SHA256SUMS` records the four frozen executables.
- The candidate runs without `--ephemeral` and with a private home.
- Three inspections show a connected Device panel with a `frame_sequence`
  that increases.
- The SHA-256 of `/proc/<child-pid>/exe` is the same as the frozen SHA-256.
- The Secret Service stores and returns a test value without a password prompt.

## 8. Cleanup

Do not stop the fixture. The other areas use it. The
[area X](x-teardown.md) stops the fixture at the end of the run.

1. Make sure that no test value stays in the keyring.

   ```sh
   secret-tool lookup smoke probe
   ```

   Result: The output is empty.

## 9. Record of results

Write the results in the report of the run. Use the
[report template](../../reports/TEMPLATE.md). Keep private evidence out of the
repository.
