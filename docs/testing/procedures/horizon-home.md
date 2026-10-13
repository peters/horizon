---
procedure: horizon-home
feature: HORIZON_HOME state root override
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# HORIZON_HOME test procedure

## 1. Purpose

This procedure makes sure that `HORIZON_HOME` moves all Horizon state out of
`~/.horizon`. It also makes sure that Horizon rejects a value that cannot hold
private state, and that `HOME` does not change for the terminals.

## 2. Applicability

- Candidate: a build that reads `HORIZON_HOME`.
- Platforms: Linux. On macOS, tasks H01 to H04 apply.
- This procedure does not test these functions:
  - The browser tools of an agent in a Codex panel. The unit tests examine
    that Codex passes `HORIZON_HOME` to the browser MCP server.
  - The browser tools of an agent in a Grok or Claude Code panel. These agents
    give their MCP servers their whole environment, so `HORIZON_HOME` arrives
    without a registration change.
  - The folder permissions on Windows. Windows has no owner-only mode check.
  - ACLs. Horizon examines only the owner and the permission bits.

## 3. Safety

> **CAUTION:** SET `HOME` TO A TEST DIRECTORY IN TASKS H01 TO H04 AND H06.
> Horizon writes agent skills to the home directory before it opens a window.

> **CAUTION:** STOP ONLY THE FIXTURE OF THIS RUN. Other Horizon processes can
> hold your work.

## 4. Equipment and preconditions

- A frozen candidate `<bin>/horizon` and its SHA-256. Use the
  [device smoke fixture](../../../scripts/device-smoke/README.md) notes.
- The prerequisites of the device smoke fixture.

These names apply in the steps:

| Name | Value |
|---|---|
| `<run>` | A new task-owned directory for this run, outside `$HOME` and outside `/tmp`. |
| `<home>` | The test home directory, `<run>/home`. |
| `<state>` | The state directory of the fixture, `<run>/fixture`. This directory must not exist before H05. |
| `<display>` | The `display` value in the output of the fixture. |

## 5. Setup

1. Make the run directory and the test home directory.

   ```sh
   mkdir -p <run>/home && chmod 700 <run>
   ```

   Result: Only the operator can read `<run>`.

## 6. Tasks

### 6.1 H01: Horizon rejects a relative path

1. Start the candidate with a relative value.

   ```sh
   env -u DISPLAY -u WAYLAND_DISPLAY HOME=<home> HORIZON_HOME=relative <bin>/horizon; echo "exit $?"
   ```

   Result: The output is `error: HORIZON_HOME must be an absolute path, not relative` and `exit 2`.

2. Start a native mode and the browser MCP mode with the relative value.

   ```sh
   cd <run> && for mode in --native-catalog --browser-mcp; do HOME=<home> HORIZON_HOME=relative <bin>/horizon $mode </dev/null; echo "exit $?"; done
   ```

   Result: Each mode shows the same error and `exit 2`. `<run>/relative` does not exist.

3. Start the agent work hook with the relative value.

   ```sh
   HOME=<home> HORIZON_HOME=relative HORIZON_WORK_ROOT= <bin>/horizon --agent-work-hook </dev/null; echo "exit $?"
   ```

   Result: The output is `exit 0`. The hook does not use the Horizon home, and
   its exit codes control the turn of the agent.

### 6.2 H02: Horizon rejects a file, a shared folder and a link

1. Make a file, a folder that other users can read, and a link to a private folder.

   ```sh
   touch <run>/file && mkdir -m 755 <run>/shared && mkdir -m 700 <run>/private && ln -s private <run>/link
   ```

   Result: The file, the folders and the link exist.

2. Start the candidate with the file.

   ```sh
   env -u DISPLAY -u WAYLAND_DISPLAY HOME=<home> HORIZON_HOME=<run>/file <bin>/horizon; echo "exit $?"
   ```

   Result: The output says that the path `is not a directory`, and `exit 2`.

3. Start the candidate with the shared folder.

   ```sh
   env -u DISPLAY -u WAYLAND_DISPLAY HOME=<home> HORIZON_HOME=<run>/shared <bin>/horizon; echo "exit $?"
   ```

   Result: The output says that the folder `is open to other users`, and `exit 2`.

4. Start the candidate with the link, also with a trailing `/`.

   ```sh
   for root in <run>/link <run>/link/; do env -u DISPLAY -u WAYLAND_DISPLAY HOME=<home> HORIZON_HOME=$root <bin>/horizon; echo "exit $?"; done
   ```

   Result: Each output says that the path `is a symbolic link`, and `exit 2`.

### 6.3 H03: Horizon makes a missing folder private

1. Start the candidate with a folder that does not exist. The candidate stops
   because there is no display.

   ```sh
   env -u DISPLAY -u WAYLAND_DISPLAY HOME=<home> HORIZON_HOME=<run>/new/private <bin>/horizon 2>&1 | grep 'no config found'
   ```

   Result: The line names `<run>/new/private/config.yaml`.

2. Examine the folder modes.

   ```sh
   stat -c '%a %n' <run>/new <run>/new/private
   ```

   Result: Both lines start with `700`.

3. Examine the state folders.

   ```sh
   ls <run>/new/private; ls -A <home>
   ```

   Result: `<run>/new/private` contains `integrations` and `runtime`.
   `<home>` does not contain `.horizon`.

### 6.4 H04: Horizon reads the config only from the override

1. Write a config in the other places that Horizon reads without the override.

   ```sh
   mkdir -p <home>/.horizon <home>/.config/horizon
   echo 'version: 11' > <home>/.horizon/config.yaml
   echo 'version: 11' > <home>/.config/horizon/config.yaml
   echo 'version: 11' > <run>/horizon.yaml
   ```

   Result: Three config files exist.

2. Start the candidate from `<run>` with the private folder of H03.

   ```sh
   cd <run> && env -u DISPLAY -u WAYLAND_DISPLAY HOME=<home> XDG_CONFIG_HOME=<home>/.config HORIZON_HOME=<run>/new/private <bin>/horizon 2>&1 | grep 'config'
   ```

   Result: The output names only `<run>/new/private/config.yaml`. It does not
   name the three files of step 1.

3. Remove the three files of step 1 and `<home>/.horizon`.

   Result: `<home>` does not contain `.horizon`.

### 6.5 H05: The fixture ignores the override of the operator

1. Start the fixture with an override in the shell of the operator.

   ```sh
   HORIZON_HOME=<run>/operator-home python3 scripts/device-smoke/serve.py \
     --horizon <bin>/horizon --native-view --state <state>
   ```

   Result: The output shows a `vnc_address` and a `display`.

2. Find the Horizon child and compare its executable with the frozen candidate.

   ```sh
   pid=$(pgrep -f -- "^<bin>/horizon --config <state>/"); sha256sum /proc/$pid/exe
   ```

   Result: The hash is the same as the hash of `<bin>/horizon`.

3. Examine the environment of the Horizon child and the state folders.

   ```sh
   tr '\0' '\n' < /proc/$pid/environ | grep -c '^HORIZON_HOME='; ls -d <run>/operator-home <state>/data/home/.horizon
   ```

   Result: The count is `0`. Only `<state>/data/home/.horizon` exists.

### 6.6 H06: A Horizon window starts with the override

1. Start a second candidate on the fixture display with a private folder.

   ```sh
   env -u DBUS_SESSION_BUS_ADDRESS -u WAYLAND_DISPLAY DISPLAY=<display> HOME=<home> \
     HORIZON_HOME=<run>/gui-home <bin>/horizon --ephemeral --blank > <run>/gui.log 2>&1 &
   ```

   Result: A second Horizon window opens on the fixture display.

2. Take a screenshot of the fixture display.

   ```sh
   DISPLAY=<display> import -window root <run>/h06.png
   ```

   Result: The screenshot shows the second Horizon window.

3. Examine the environment of the second candidate and the state folders.

   ```sh
   tr '\0' '\n' < /proc/$!/environ | grep -E '^(HOME|HORIZON_HOME)='; stat -c '%a' <run>/gui-home; ls -A <home>
   ```

   Result: `HOME` is `<home>` and `HORIZON_HOME` is `<run>/gui-home`. The mode
   is `700`. `<home>` does not contain `.horizon`.

4. Close the second window with the window manager.

   ```sh
   DISPLAY=<display> xdotool search --all --pid $! --name Horizon windowactivate --sync key alt+F4
   ```

   Result: The second candidate stops. The fixture Horizon continues to run.
   `<home>` contains no `horizon-*` skill directory.

   > **CAUTION:** KEEP `--all` IN THE SEARCH. Without it, xdotool finds every
   > window with the name Horizon, and alt+F4 can close the fixture.

## 7. Pass criteria

- H01 and H02 stop with exit code 2 and the given message.
- H03 and H06 make the override folder with mode 700. No `.horizon` folder
  shows in the test home directory.
- H04 reads the config only from the override folder.
- In H05, the fixture child has the frozen hash and no `HORIZON_HOME`.
- In H06, the window shows and the second candidate keeps the given `HOME`.

## 8. Cleanup

1. Stop the fixture with Ctrl-C in its terminal.

   Result: The fixture removes `<state>/data`.

2. Remove the run directory.

   ```sh
   rm -rf <run>
   ```

   Result: `<run>` does not exist.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
