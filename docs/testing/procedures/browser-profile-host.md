---
procedure: browser-profile-host
feature: Profile host of horizon-browser
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser profile host test procedure

## 1. Purpose

This procedure tests the profile host of `horizon-browser`, part by part.
A profile host keeps named browser profiles for connect clients.
Each connect client controls only its own pages.
A new page of a profile host on a shared display must not take the keyboard focus from a person.

## 2. Applicability

- Candidate: a source build of `horizon-browser` that has the `host` command.
- Platforms: Linux and macOS. The mode checks of files apply only to Unix. Task PH-VISIBLE needs an X11 display.
- This procedure does not test: remote targets, the Horizon desktop host or a Windows computer.

## 3. Safety

> **CAUTION:** USE A NEW BROWSER RUNTIME ROOT FOR EACH RUN.
> A browser profile keeps cookies and sign-ins. The root of a real profile host contains real sign-ins.

> **CAUTION:** OPEN ONLY THE LOOPBACK TEST SITE IN A BROWSER PROFILE OF THIS PROCEDURE.
> The profile host keeps each sign-in after the run.

## 4. Equipment and preconditions

- The source of the candidate and the Rust toolchain of the repository.
- Chromium or Google Chrome, for the live tasks.
- For task PH-VISIBLE: `Xvfb`, `xmessage` and `xdotool`.
- An MCP client that can call a tool of an MCP server over stdio, for example an agent.
- An empty directory for the run, for example `$HOME/ph-run`.
  If Chromium is a snap package, use a directory in your home directory whose name does not start with a dot.

## 5. Setup

1. Build the candidate.

   ```sh
   cargo build -p horizon-browser-cli
   ```

   Result: The file `target/debug/horizon-browser` is available.

2. Write the host file `$HOME/ph-run/host.yaml` and give it the mode 0600.

   ```yaml
   browser:
     profiles:
       mail:
         backend: chromium
   ```

   Result: `stat -c %a $HOME/ph-run/host.yaml` (Linux) or `stat -f %Lp $HOME/ph-run/host.yaml` (macOS) shows `600`.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 PH-WINDOWS: Background windows of a headed Chromium

1. Run the regressions of the launch and of the first page.

   ```sh
   cargo test -p horizon-browser --lib background
   cargo test -p horizon-browser --lib shared_process_pins_profile_and_launch_settings_across_restarts
   cargo test -p horizon-browser --lib initial_target_claim_preserves_empty_startup_fallback
   ```

   Result: The tests pass. A headed launch with background windows starts Chromium with `--no-startup-window`.
   A headless launch does not get this argument.
   The first page of this launch opens in a new window that does not get the focus.
   A headed shared browser keeps the setting of its first launch. A headless browser ignores the setting.

### 6.2 PH-FILE: The host file

1. Run the regressions of the host file.

   ```sh
   cargo test -p horizon-browser-cli --lib standalone::profile::tests::a_host_file
   cargo test -p horizon-browser-cli --lib standalone::profile::tests::user_js
   cargo test -p horizon-browser-cli --lib standalone::profile::tests::a_preference
   ```

   Result: The tests pass. The profile host refuses a host file with one of these problems:
   - a profile name that is not 1 to 40 of `a-z`, `0-9` and `-`;
   - an engine other than `chromium` or `firefox`, or an unknown field;
   - an argument that the engine manages, for example `--user-data-dir` or `--headless`;
   - a control character in an argument or a preference;
   - more than 32 browser profiles, or more than 256 KiB;
   - write permission for the group or other users.

   A Firefox preference that the host file no longer names gets its default again at the next start.

### 6.3 PH-RULES: The rules of a create

1. Run the regressions of the create rules, the page IDs, the idle pages and the host record.

   ```sh
   cargo test -p horizon-browser-cli --lib standalone::profile::tests
   ```

   Result: The tests pass. These rules apply:
   - A create must name a browser profile of the host file. A create with no target, a remote target or a backend is refused.
   - A duplicate opens only from a page of the same agent.
   - One agent has a maximum of 4 pages. The host has a maximum of 16 pages.
     Only an agent with no page gets the 16th page.
   - A page ID is `profile-<name>-` and 12 hexadecimal digits.
   - A page with no action for the idle limit closes.
   - One profile host runs on a browser runtime root. It keeps its host instance after a restart.

### 6.4 PH-START: Start, refuse and stop a profile host

1. Run the regressions of the `host` command.

   ```sh
   cargo test -p horizon-browser-cli --bin horizon-browser the_profile_host
   cargo test -p horizon-browser-cli --test standalone_profile
   ```

   Result: The tests pass. The `host` command needs one `--config`.
   A bad host file and a second host on the same browser runtime root stop with the code 1.
   A connect client that names no browser profile, an unknown browser profile or a remote target gets a refusal.

2. In a second terminal, start the profile host with a new browser runtime root.

   ```sh
   HORIZON_BROWSER_ROOT=$HOME/ph-run/root target/debug/horizon-browser host --config $HOME/ph-run/host.yaml
   ```

   Result: The file `$HOME/ph-run/root/profile-host.json` has the mode 600.
   It contains `host_instance`, `pid` and `started_at`.
   The folder `$HOME/ph-run/root/browser-profiles/profile-mail` has the mode 700.

3. Send SIGTERM to the `pid` of `profile-host.json`, for example with `kill -TERM <pid>`.
   Then start the profile host again with the command of step 2.

   Result: The first process stops with the code 0. The folder of the browser profile stays.
   The new `profile-host.json` contains the same `host_instance`.

### 6.5 PH-SHARE: Two agents share the cookies of a browser profile

1. Run the live regression. It starts the profile host, two connect clients and a loopback test site.
   If Chromium is a snap package, make the folder `$HOME/ph-run/tmp` and add `TMPDIR=$HOME/ph-run/tmp` before the command.
   The snap cannot use a profile folder under `/tmp`.

   ```sh
   cargo test -p horizon-browser-cli --test standalone_profile -- --ignored
   ```

   Result: The test passes. It proves these results:
   - The page of the second agent gets the cookie that the page of the first agent set.
   - `browser_list` of each agent shows only its own page.
   - A tool call of one agent on the page of the other agent fails.
   - A connect client of another host instance sees no page.
   - The cookie stays after a restart of the profile host.

### 6.6 PH-VISIBLE: A visible page does not take the keyboard focus

1. Start an X11 display with no window manager. Open a window and give it the focus.
   If the display `:77` is already in use, use a free number in all steps of this task.

   ```sh
   Xvfb :77 -screen 0 1280x800x24 &
   sleep 2
   DISPLAY=:77 xmessage -name ph-focus "keyboard owner" &
   DISPLAY=:77 xdotool search --sync --name "^ph-focus$" windowfocus --sync
   DISPLAY=:77 xdotool getwindowfocus getwindowname
   ```

   Result: The last command shows `ph-focus`.

2. Start the profile host with `--visible` on this display.

   ```sh
   env -u WAYLAND_DISPLAY DISPLAY=:77 HORIZON_BROWSER_ROOT=$HOME/ph-run/visible \
     target/debug/horizon-browser host --config $HOME/ph-run/host.yaml --visible
   ```

   Result: No Chromium window opens.
   The command removes `WAYLAND_DISPLAY`, so the browser uses the X11 display and not the desktop of your session.

3. Start a connect client with your MCP client.
   Use the command `target/debug/horizon-browser mcp --connect` and these environment variables:
   - `HORIZON_BROWSER_ROOT=$HOME/ph-run/visible`;
   - `HORIZON_BROWSER_ACTOR=horizon:example/ws/agent`;
   - `HORIZON_BROWSER_HOST_INSTANCE` with the `host_instance` of `$HOME/ph-run/visible/profile-host.json`.

   Result: The MCP client lists the browser tools.

4. Call `browser_create` with `{"target": "mail", "url": "about:blank"}`.
   Then call it again with `"allow_additional": true` in the arguments.

   Result: Each call gives a page ID that starts with `profile-mail-`.
   `DISPLAY=:77 xdotool search --onlyvisible --class chrom` shows a window for each page.
   `DISPLAY=:77 xdotool getwindowfocus getwindowname` still shows `ph-focus`.

## 7. Pass criteria

- Every result in section 6 is as written.
- No step changes a browser profile outside `$HOME/ph-run`.

## 8. Cleanup

1. Send SIGTERM to each profile host of the run. Stop `Xvfb` and `xmessage`.

   Result: Each process stops.

2. Remove the directory `$HOME/ph-run`.

   Result: No browser profile of the run stays on the computer.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
