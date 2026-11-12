---
procedure: remote-browser-tunnel
feature: Browser panel of a browser on another machine, over an SSH forward
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Remote browser tunnel test procedure

## 1. Purpose

This procedure proves that a browser panel of Horizon drives the Chromium of
another machine through one SSH forward, with the same tools, frames and input
as a panel of this computer. It also proves the typed refusals of that path.

## 2. Applicability

- Candidate: a build of `horizon` that has the `browser_host` settings entry.
- Client platforms: Linux and macOS.
- Host platforms: macOS or Linux with Chrome or Chromium, and an `sshd` that
  answers without a prompt.
- This procedure does not test: Firefox and Safari on a host (they are
  refused), file upload and downloads (they are refused or absent on that
  path), and a host without a graphical login session.

## 3. Safety

> **CAUTION:** THE PROCEDURE STARTS A CHROMIUM ON THE HOST MACHINE AND KEEPS IT RUNNING AFTER HORIZON STOPS. That browser holds the sign-ins of the profile. Use a host machine and a profile that you can leave running, and stop the browser yourself in task RBT-10.

> **CAUTION:** SIGN IN ONLY WITH A TEST ACCOUNT. A page that you sign in to stays signed in on the host machine, and every local account of that machine can reach the browser through its loopback port.

## 4. Equipment and preconditions

- Two computers: the client that runs Horizon, and the host that runs the
  browser. One computer can act as both when `ssh 127.0.0.1` answers without a
  prompt.
- The OpenSSH client on the client machine, with a key that the host accepts, so
  that `ssh <SSH-TARGET> true` answers without a prompt.
- Chrome or Chromium on the host machine, as a program on the `PATH` of a
  non-interactive `ssh` command, or as an application bundle in `/Applications`
  on macOS.
- A graphical login session on the host machine. The procedure starts a visible
  browser with no window of its own.
- The settings store of Horizon on the client machine,
  `~/.horizon/settings.sqlite`.
- A workspace of Horizon with free canvas space.
- The unit tests of this path pass:

  ```bash
  cargo test -p horizon-core --lib browser::attached_host
  cargo test -p horizon-browser-control --lib ssh_forward
  ```

## 5. Setup

1. On the host machine, confirm the program and the open port:

   ```bash
   ssh <SSH-TARGET> 'command -v google-chrome || command -v chromium || command -v chromium-browser'
   ```

   Result: the command prints one path.

2. On the client machine, run the live lane of this path. It starts a Chromium on
   the host, paints a page, clicks it, and connects a second time:

   ```bash
   HORIZON_BROWSER_ATTACH_SSH_TARGET=<SSH-TARGET> \
   [HORIZON_BROWSER_ATTACH_PROFILE=<PROFILE>] \
     cargo test -p horizon-core attached_host_panel_drives_a_real_ssh_target -- --ignored --nocapture
   ```

   Result: the test ends with `test result: ok`. A host without a Chromium, or
   with a refused host key, ends the test with the typed reason and the words of
   `ssh`.

3. Build Horizon on the client machine.

   ```bash
   cargo build
   ```

   Result: `target/debug/horizon` exists.

4. On the client machine, close Horizon. Name the host in the settings: open
   Settings (**Ctrl+Shift+,**) → **General** → **Browser host**, and write the
   SSH target and the profile.

   The SSH target is one name that `ssh` resolves: an alias of `~/.ssh/config`, a
   `user@host`, or an address. The profile is 1 to 40 of `a-z`, `0-9` and `-`,
   with a letter or a digit at each end. Without a profile, Horizon uses
   `default`.

   Result: Horizon writes the entry to the store. A target that starts with `-`,
   holds a space or a control character, a profile off the rule above, and a
   profile without a target are refused with an error that names `browser_host`.

5. Start Horizon.

   Result: the start reads the entry. A change of the entry applies at the next
   start.

   For a fixture run, use `scripts/device-smoke/serve.py --native-view
   --browser-host <SSH-TARGET> [--browser-host-profile mail]`, with
   `--ssh-config` and `--ssh-identity` when the fixture must have its own SSH
   configuration. Do not use the `~/.ssh` of the person.

## 6. Tasks

A report uses the ID of each task to give a result.

### 6.1 RBT-1 — a person creates a panel on the host

1. Hold Ctrl on Linux, or Cmd on macOS, and double-click empty canvas space
   inside a workspace.

   Result: the preset picker opens.

2. Pick **Browser**.

   Result: a new browser panel appears. The chrome shows the SSH target where the
   engine menu was, and it shows no engine menu.

3. Click the address bar, type `https://example.com`, and press Enter.

   Result: the panel paints the page. The page is the page of the Chromium on the
   host machine.

### 6.2 RBT-2 — a click and the wheel reach the page

1. Click a link on the page.

   Result: the page opens the link. A press and a release of a pointer are input
   of the panel, as for a panel of this computer.

2. Move the pointer over the page and turn the wheel.

   Result: the page scrolls.

3. Press a key that the page uses, for example `Tab`.

   Result: the focus moves on the page.

### 6.3 RBT-3 — a resize reaches the page

1. Drag the bottom right corner of the panel.

   Result: the page takes the new viewport. The page is not reloaded.

### 6.4 RBT-4 — two panels of one profile share the sign-in

1. Sign in to a site on the page of task RBT-1, with a test account.

   Result: the site shows the signed-in page.

2. Create a second browser panel, and open the same site.

   Result: the site shows the signed-in page, because both panels drive the
   Chromium of the same profile on the host machine.

### 6.5 RBT-5 — a restart keeps the sign-in and leaves the host browser running

1. Close Horizon.

   Result: Horizon ends. The forward to the host ends with it.

2. On the host machine, examine its browser processes:

   ```bash
   ssh <SSH-TARGET> 'pgrep -fl "remote-debugging-port=0"'
   ```

   Result: the Chromium of the profile is still running.

3. Start Horizon again.

   Result: the saved panel connects to the running browser, and its page is
   signed in. A panel that the restore opens starts a new page in the same
   Chromium.

### 6.6 RBT-6 — a host that does not answer is a typed message

1. Name a host that does not answer, in the settings entry, and start Horizon.
   Create a browser panel.

   Result: the panel states
   `the browser host <target> could not be reached over SSH: <words of ssh>; check the SSH target, then use Retry`,
   and it shows the button **Retry**.

2. Correct the entry, restart Horizon, and click **Retry**.

   Result: the panel connects.

### 6.7 RBT-7 — a host without a Chromium names that reason

1. Use a host machine that has no Chrome or Chromium program in the locations
   of section 4. Create a browser panel.

   Result: the panel states
   `has no Chromium program to start: <words of the snippet>; install Chromium or Google Chrome on that machine, then use Retry`,
   with the SSH target in front of that text.

### 6.8 RBT-8 — another engine is refused

1. Examine the chrome of a panel of task RBT-1.

   Result: it shows the SSH target and no engine menu, because the engine of that
   page is the Chromium of the host.

2. Ask an agent to call `browser_create` with `backend: firefox`, or with
   `backend: safari`.

   Result: the create is refused with `unsupported_backend` and
   `a browser host serves Chromium only: the page of that host runs in the Chromium of its profile, so run this panel on the browser of this computer instead`.
   No panel appears.

### 6.9 RBT-9 — a lost browser keeps the panel and says why

1. With the panel of task RBT-1 open on a page, stop the browser of the host
   machine:

   > **CAUTION:** THE COMMAND STOPS EVERY CHROMIUM ON THE HOST MACHINE THAT
   > HORIZON STARTED. Use a host machine that has no other work in a browser.

   ```bash
   ssh <SSH-TARGET> 'pkill -f "remote-debugging-port=0"'
   ```

   Result: the panel does not go away. It stays where it is on the canvas, the
   address bar keeps its last address, and the body of the panel states
   `Browser error: the browser of <SSH-TARGET> in the profile <PROFILE> lost its connection, so this panel keeps its last page; Retry connects again`,
   with the button **Retry**. The body holds no word of the browser engine. The
   sidebar shows no notice, because the panel itself says why.

2. Read the state of that panel as an agent. Call `browser_panel` with its
   `panel_id`.

   Result: the answer holds `failure.code` `attached_host_connection_lost`, and
   `failure.message` holds the sentence of step 1 and then the words of the
   engine in parentheses. `browser_list` still shows the panel. A control call on it, for example `browser_navigate`, fails with
   `browser_unavailable`, because the panel has no browser until the person
   retries.

3. Click **Retry**.

   Result: Horizon starts the browser of the host machine again, opens one new
   page at the address of step 1, and the panel paints that page in the same
   place on the canvas.

4. Stop the browser of the host machine a second time, and close the panel with
   its close button.

   Result: the panel goes, and the panel leaves `browser_list`.

### 6.10 RBT-10 — downloads and file upload have their typed text

1. On the page, start a download of a small file, then call
   `browser_downloads` on the panel.

   Result: `browser_downloads` reports `reporting: false` with its text that the
   session could not observe downloads, and the panel lists no download. The file
   is on the host machine, in the download folder of that browser.

2. Ask an agent to call `browser_act` with `operation: set_files` on a file
   input of that page.

   Result: the answer is the typed code `remote_file_upload_unsupported`, with
   the text that this browser cannot read a file of this computer. The page takes
   no file.

3. Stop the browser of the host machine when the run is done:

   ```bash
   ssh <SSH-TARGET> 'pkill -f "remote-debugging-port=0"'
   ```

   Result: the process of the profile ends. The profile folder stays, with its
   sign-ins.

## 7. Pass criteria

- Every task gives its expected result.
- A panel of the host machine takes a click, a wheel event, a key press and a
  resize, as a panel of this computer does.
- Two panels of one profile share the sign-in, and a restart of Horizon keeps
  it.
- A host that does not answer, a host without a Chromium, another engine, a
  lost browser, a download and a file upload each give the typed text of
  section 6.
- A panel that loses its browser stays on the canvas with its last address, and
  `browser_panel` reports the typed code of that loss for an agent.
- No step runs an `ssh` command on the UI thread, and Horizon shows no frozen
  window while it connects.

## 8. Cleanup

1. Close the panels that the run created, and close Horizon.

   Result: the forwards end. No `ssh` process of the run remains on the client
   machine.

2. Stop the browser of the host machine, when task RBT-10 did not do it.

   Result: `pgrep -fl remote-debugging-port=0` on the host machine prints
   nothing.

3. Remove the settings entry, or set the SSH target that you use.

   Result: a new browser panel runs on this computer again.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
