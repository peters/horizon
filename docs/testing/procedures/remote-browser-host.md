---
procedure: remote-browser-host
feature: remote browser host bridge
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Remote browser host bridge test procedure

## 1. Purpose

This procedure proves that the agents of one computer can use the profile host of
another machine, with the same tools, the same results and the same refusals as a
local connect client.

## 2. Applicability

- Candidate: a build of `horizon` and `horizon-browser` from this branch or later.
- Platforms: Linux on the computer of the person. macOS uses the same commands.
- This procedure does not test: a browser panel of the remote host, the egress
  profile, and the deploy entry. Later pulls of #1780 add them.

## 3. Safety

> **CAUTION:** DO NOT USE A REMOTE MACHINE THAT HOLDS DATA THAT YOU CANNOT LOSE. THE PROCEDURE STARTS AND STOPS A BROWSER PROFILE HOST ON IT.

> **CAUTION:** A STAGED FILE EXISTS ON THE REMOTE MACHINE FOR THE DURATION OF THE CALL. USE A TEST FILE THAT HOLDS NO SECRET.

## 4. Equipment and preconditions

- Two computers, or one computer and one container, that answer `ssh` without a
  prompt in both directions that the procedure uses.
- `horizon-browser` on the remote machine, on the `PATH` of its login shell, or
  the path in `HORIZON_BROWSER_REMOTE_EXECUTABLE`.
- A local Chromium for the tests that open pages.
- The third task needs an SSH server on `127.0.0.1` of the test machine that
  answers without a prompt.

## 5. Setup

1. On the remote machine, write a host file with one profile and start the host:

   ```bash
   printf 'browser:\n  profiles:\n    mail:\n      backend: chromium\n' > ~/host.yaml
   chmod 600 ~/host.yaml
   horizon-browser host --config ~/host.yaml
   ```

   Result: the process keeps the profile and writes `profile-host.json` under
   `~/.horizon`.

2. On the computer of the person, close Horizon and set the entry in the store:

   ```bash
   horizon --export-config /tmp/settings.yaml
   ```

   Result: the export holds a `browser_host` section. Add `ssh_target` and the
   optional `profile` of that host to that section, then set it with the
   Settings window YAML tab, or start Horizon one time with the entry.

   ```yaml
   browser_host:
     ssh_target: browser-vm
     profile: mail
   ```

   Result: a start reads the entry. A target that starts with `-`, holds a space
   or a control character, and a profile off the host-file rule are refused with
   an error that names `browser_host`.

3. Start Horizon, then start one agent panel in a workspace.

   Result: the agent reports that it can call the browser tools.

## 6. Tasks

### 6.1 R1 — the agent reaches the remote host

1. Ask the agent to call `browser_create` with the target `mail` and the URL
   `example.com`.

   Result: the tool returns a panel id that starts with `profile-mail-`.

2. Ask the agent to call `browser_create` with the URL `example.com` and no
   target.

   Result: the tool returns a panel id that starts with `profile-mail-`, because
   the bridge adds the profile of the entry. The agent does not name the
   profile, so every page of the entry lands in the profile of the person.

3. Ask the agent to call `browser_create` with a target that the host file does
   not name, such as `chat`.

   Result: the tool returns the reason of the host itself, `target_unknown`, and
   the bridge does not replace the target of the call.

2. Ask the agent to call `browser_list`.

   Result: the list holds that page and no page of another agent.

4. Ask the agent to call `browser_screenshot`.

   Result: the result holds the path of a PNG and an image block with the PNG.

### 6.2 R2 — a file of the person reaches the remote page

1. Write a test file in the work root of the agent:

   ```bash
   printf 'test' > report.txt
   ```

2. Ask the agent to call `browser_query` for `input[type=file]`, then
   `browser_act` with `set_files` and the absolute path of `report.txt`.

   Result: the result lists the name `report.txt` and its size.

3. On the remote machine, examine `~/.horizon/browser-staging`.

   Result: the folder of the call is gone after the call.

### 6.3 R3 — a capture of the remote page arrives inline

1. Ask the agent to call `browser_network` with `operation` `start`, then one
   action on the page, then `operation` `stop`.

   Result: the result holds a resource block with `application/x-ndjson` and a
   non-empty `blob`.

2. Ask the agent to call `browser_video` with `operation` `start`, one action on
   the page, then `operation` `stop`.

   Result: the result holds a resource block with `video/webm`, or a message that
   the file is larger than 8 MiB.

### 6.4 R4 — the parity test

1. Run the parity test on the computer that answers `ssh`:

   ```bash
   cargo test -p horizon-browser-cli --test remote_parity -- --nocapture
   ```

   Result: `a_profile_host_over_ssh_advertises_the_same_capabilities_as_a_local_connect_client`
   passes. A machine without an SSH target prints the reason and skips the test.

### 6.5 R5 — a refusal names its reason

1. Stop the host on the remote machine, then start one agent panel.

   Result: the agent reports `remote browser host <target> is not running`.

2. Stop the host, then start it with the host file of a profile `chat` only.
   Ask the agent to call `browser_create` with no target.

   Result: the result holds the reason of the host itself, `target_unknown`, and
   says that the profile `mail` of the entry is not in the host file. The bridge
   refuses no profile of its own, because the host decides which profiles it
   serves.

3. Ask the agent to call `browser_act` with `set_files` and a path outside the
   work root.

   Result: the result holds `browser set_files failed (attachment path is outside
   the allowed roots ...)` and the remote machine holds no copy.

### 6.6 R6 — a sign-in stays over a restart

1. In the remote page, sign in to a test site, or run the existing profile-host
   test:

   ```bash
   cargo test -p horizon-browser-cli --test standalone_profile two_actors_share -- --ignored
   ```

   Result: the test passes; the page shows the cookie again after a restart of
   the host.

## 7. Pass criteria

- R1 to R6 pass.
- With a `browser_host` entry that names `browser-vm` and `mail`, the `.mcp.json`
  of the Claude plugin holds
  `"args": ["--browser-mcp", "--remote", "browser-vm", "--profile", "mail"]`, and
  the `~/.grok/config.toml` of Grok holds the same arguments.
- With no `browser_host` entry, the `.mcp.json` of the Claude plugin holds
  `"args": ["--browser-mcp"]` and no `--remote`.

## 8. Cleanup

1. Stop the host on the remote machine with its normal stop, then remove the test
   profile only when the procedure created it.

   Result: no host process of the procedure runs.

2. Remove the test file and the test entry from the settings.

   Result: the next start uses the local browser.

## 9. Record of results

Put the results in the pull request, or write a report under
`docs/testing/reports/`.
