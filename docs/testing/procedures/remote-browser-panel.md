---
procedure: remote-browser-panel
feature: browser panel backed by a browser host on another machine
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Remote browser panel test procedure

## 1. Purpose

This procedure shows that a browser panel of Horizon uses a browser host on
another machine. It shows the path of the person from the settings entry to a
page in a panel, and it shows the frame of that page, the keyboard input, the
page scroll, the navigation and the fixed limit of the pointer.

## 2. Applicability

- Candidate: a build of `horizon` and `horizon-browser` from this branch or later.
- Platforms: Linux on the computer of the person. macOS uses the same commands.
- This procedure does not test: the egress profile and the deploy entry. Later
  pulls of #1780 add them. It does not test the pointer of the person on a
  remote page, because the MCP contract has no coordinate pointer operation. Use
  the task `R4` for that limit.
- It does not test a drag on a remote page. A drag does not cross the channel:
  the task `R4` states that limit.

## 3. Safety

> **CAUTION:** DO NOT USE A REMOTE MACHINE THAT HOLDS DATA THAT YOU CANNOT LOSE. THE PROCEDURE STARTS AND STOPS A BROWSER HOST ON IT.

## 4. Equipment and preconditions

- Two computers that answer `ssh` without a prompt. One computer can act as
  both when `ssh 127.0.0.1` answers without a prompt.
- `horizon-browser` on the remote machine, on the `PATH` of its login shell.
- A local Chromium on the remote machine.
- The settings store of Horizon, `~/.horizon/settings.sqlite`.
- A workspace of Horizon with free canvas space.
- The unit tests of this change pass:

  ```bash
  cargo test -p horizon-core --lib browser::remote_host
  ```

## 5. Setup

1. On the remote machine, write a host file with one profile and start the host:

   ```bash
   printf 'browser:\n  profiles:\n    mail:\n      backend: chromium\n' > ~/host.yaml
   chmod 600 ~/host.yaml
   horizon-browser host --config ~/host.yaml
   ```

   Result: the process writes `profile-host.json` under `~/.horizon`.

2. On the computer of the person, run the live lane of this change. Name the
   SSH target when it is not `127.0.0.1`:

   ```bash
   HORIZON_BROWSER_REMOTE_SSH_TARGET=<SSH-TARGET> \
     cargo test -p horizon-core --test remote_host_panel -- --ignored
   ```

   Result: the test ends with `test result: ok`.

3. Build Horizon:

   ```bash
   cargo build
   ```

   Result: `target/debug/horizon` exists.

4. On the computer of the person, close Horizon. Name the host in the settings:

   ```bash
   horizon --export-config /tmp/settings.yaml
   ```

   Result: the export holds a `browser_host` section. Add the SSH target of the
   remote machine of step 1 and the profile `mail` to that section, then set it
   with the Settings window YAML tab:

   ```yaml
   browser_host:
     ssh_target: <SSH-TARGET>
     profile: mail
   ```

   The SSH target is one name that `ssh` resolves: an alias of
   `~/.ssh/config`, a `user@host`, or an address.

   Result: Horizon writes the entry to the settings store. A target that starts
   with `-`, holds a space or a control character, and a profile off the
   host-file rule are refused with an error that names `browser_host`.

5. Start Horizon.

   Result: the start reads the entry. A change of the entry applies at the next
   start.

   For a fixture run, name the host with `--browser-host <SSH-TARGET>
   --browser-host-profile mail` of `scripts/device-smoke/serve.py` instead of
   step 4.

## 6. Tasks

### 6.1 R1 — the live lane of the channel

1. Do step 5.2, then read its output.

   Result: the test reports two passed tests. Without an SSH server that answers
   without a prompt, each test writes `skipped:` and the reason, and does nothing.

### 6.2 R2 — a person creates a panel on the host

1. Double-click on empty canvas space inside a workspace.

   Result: the picker `New Terminal` opens.

2. Click `Browser` in the group `Tools`.

   Result: a new browser panel appears. The chrome shows the SSH target where the
   engine picker was, and no engine picker.

3. Click the address bar, type `https://example.com`, and press Enter.

   Result: the panel paints the page from the remote machine. A page that the
   host cannot open shows the reason of the host in the panel.

4. Examine the two lines below the chrome.

   Result: the first line is
   `To click this page, open the machine's display in a device panel.` The second
   line is `This panel keeps the browser host that it was created with; a new
   panel follows the settings.`

5. A test creates the same panel without the settings entry:

   ```rust
   let backend = RemoteHostBackend::new("<SSH-TARGET>", Some("mail".to_string()))?;
   let panel = BrowserPanelState::start_remote_host("panel-mail", &BrowserConfig::default(), Some("https://example.com/".into()), &backend)?;
   ```

   Result: `panel.is_remote_host()` is true, and `panel.remote_host_target()`
   gives the SSH target.

6. Call `panel.drain_events()` until `panel.status` is `BrowserStatus::Ready`.

   Result: `panel.frame_slot.latest()` gives a decoded PNG, and `panel.backend()`
   is the engine of the profile of the host.

### 6.3 R3 — the input reaches the page

1. Send a key press:

   ```rust
   panel.try_send(BrowserCommand::Input(BrowserInput::KeyDown {
       physical_key: Some(BrowserKey::Tab), key: BrowserKey::Tab, text: None,
       modifiers: BrowserModifiers::none(), repeat: false, edit_command: None,
   }));
   ```

   Result: `try_send` returns true. The focus moves on the remote page.

2. Send a wheel event. This is a page scroll, and the point in it is not used:

   ```rust
   panel.try_send(BrowserCommand::Input(BrowserInput::Wheel {
       x: 10.0, y: 20.0, delta_x: 0.0, delta_y: -400.0,
       modifiers: BrowserModifiers::none(),
   }));
   ```

   Result: the page of the host scrolls.

3. Send a navigation.

   Result: the next frame of the panel shows the new page, and `panel.url` gives
   its address.

### 6.4 R4 — a click of the person reaches the page, and a drag does not

1. Click a link of the page in the panel of step 6.2.

   Result: the page of the host changes to that link, and the next frame shows
   the new page. The click crossed as one click at its point.

2. Click a control that needs a real pointer position, such as a canvas, a map or
   a video control, or let an agent click one point:

   ```
   browser_act { "panel_id": "<id>", "action": "click", "x": 120, "y": 240 }
   ```

   Result: the point of the page reacts. The answer is `completed`.

3. Ask the agent to click a point outside the page viewport, for example
   `x: 5000`.

   Result: the answer is `invalid_input`, and its message names the point and the
   size of the viewport. The page does not change.

4. Drag inside the page, for example across a slider.

   > **CAUTION:** A DRAG DOES NOT CROSS THE CHANNEL. THE PANEL CLICKS WHERE THE POINTER LET GO.

   Result: the panel clicks the point where the drag ended, and the page does not
   follow the path of the drag. The note of the panel states this limit.

### 6.5 R5 — the pages of the person and of an agent stay apart

An agent that calls `browser_create` keeps its page on the browser of the
computer that runs the agent server. A browser panel that an agent asks Horizon
to create stays on the local browser, so it never takes the profile of the
person.

1. Start one agent panel in a workspace, and let the agent call `browser_create`
   on the same profile of the same host.

2. Ask the agent to call `browser_list`.

   Result: the list holds the page of the agent and not the page of the panel.
   The page of the panel has the actor `horizon:person:browser-panel`.

3. Call `browser_close` on the page of the agent.

   Result: the page of the panel keeps its frame, and its counter advances.

### 6.6 R6 — the host stops

1. Stop the profile host on the remote machine with Ctrl-C.

2. Call `panel.drain_events()` until `panel.status` changes.

   Result: the status is `BrowserStatus::Error` with the text of
   `RemoteHostFailure::STOPPED`. The panel offers Retry, and the note gives the
   SSH target.

3. Start the profile host again, then click `Retry`.

   Result: the channel starts again, the panel shows a new page of the host, and
   it keeps the SSH target and the profile of the entry. Retry never starts the
   browser of this computer.

### 6.7 R7 — an existing panel keeps its host, and a new panel follows the entry

1. With the panel of R2 on screen, name another SSH target in the settings, or
   remove the `browser_host` section.

   Result: the panel keeps its host while Horizon runs, and it keeps painting.

2. Close Horizon and start it again.

   Result: Horizon restores the panel of R2 on the local browser, because the
   saved state does not record the backend of a browser panel yet. The chrome of
   that panel shows the engine picker and neither of the two lines.

3. Create a new browser panel as in steps 6.2.1 and 6.2.2.

   Result: the new panel follows the entry: it shows the second SSH target, or,
   when the entry is removed, it uses the local browser and shows no SSH target.

### 6.8 R8 — a store that locks again shows locked without a restart

This task needs a Linux computer with a Secret Service daemon, for example
GNOME Keyring or `kwalletd` behind `gnome-keyring-daemon`.

1. Open Settings > Remote browsers with a value of a provider in the OS store,
   and keep the section on screen.

   Result: the entry shows the value as present.

2. Lock the default collection of the store, so that every item needs the
   person again.

   ```sh
   busctl --user call org.freedesktop.secrets /org/freedesktop/secrets \
     org.freedesktop.Secret.Service Lock ao 1 \
     /org/freedesktop/secrets/collection/login
   ```

   Result: The call answers with two arrays. Then read the property:

   ```sh
   busctl --user get-property org.freedesktop.secrets \
     /org/freedesktop/secrets/collection/login \
     org.freedesktop.Secret.Collection Locked
   ```

   Result: The answer is `b true`. When `org.freedesktop.secrets` has no
   `collection/login`, read the object list and use the path that holds the
   collection of the session:

   ```sh
   busctl --user call org.freedesktop.secrets /org/freedesktop/secrets \
     org.freedesktop.DBus.Properties Get ss \
     org.freedesktop.Secret.Service Collections
   ```

   The entry of step 1 keeps its state for a moment.

3. Wait 10 s without closing Horizon.

   Result: the entry shows `locked`, not present. Its text keeps the reference
   and names the store.

4. Ask for a remote session of that provider.

   Result: the answer is refused with `credentials_not_ready`, and no session is
   allocated at the provider.

5. Unlock the store, for example by signing in to the session again, and wait
   10 s.

   Result: the entry shows the value as present again, without a restart of
   Horizon.

## 7. Pass criteria

- R1 to R8 pass, and R8 shows `locked` after a relock and no allocation of a
  refused create.
- Retry of a panel whose host stopped keeps the SSH target and the profile, and
  the panel paints a page of that host again.
- With the settings entry, a new browser panel shows the SSH target in its
  chrome, paints a page of that host, and takes the keyboard input and the page
  scroll.
- With the settings entry, the two lines of step 6.2.4 are on the panel, and no
  engine picker is on it.
- Without the settings entry, a new browser panel uses the local browser, shows
  the engine picker, and shows neither of the two lines.

## 8. Cleanup

1. Close the panel with `panel.stop()`.

   Result: the page of the host closes, and `profile-host.json` stays.

2. Stop the profile host with Ctrl-C, and remove `~/host.yaml` on the remote
   machine when you created it for this procedure.

   Result: no process of the host stays.

3. Remove the temporary runtime root of the live lane. It is under the temporary
   directory of the test run.

   Result: no state of the test stays.

4. Remove the `browser_host` entry from the settings, and start Horizon one time.

   Result: a new browser panel uses the local browser again.

## 9. Record of results

Put the results in the pull request, or write a report under
`docs/testing/reports/`.
