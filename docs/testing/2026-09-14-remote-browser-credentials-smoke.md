# Remote browser credential entry smoke (2026-09-14)

Phase 2 of [#628](https://github.com/peters/horizon/issues/628), third PR: the
"Remote browsers" settings tab, backed by the session-only store and the OS
credential store adapter. This records the Linux headless evidence and the
steps for the macOS and Windows keychain paths, which CI does not exercise.

## Linux (done, headless)

Environment: Xvfb 1600x1000 with openbox, the debug binary started with
`--config <smoke config> --ephemeral`, a running GNOME Keyring (Secret
Service over the session bus). The smoke config declares two providers:
`device_cloud` (HTTPS, basic authentication, `cloud-user` bound to
`os_keychain`, `cloud-key` bound to `session`) and `local_grid` (loopback
HTTP, bearer token, `grid-token` unbound), plus one target each.

Observed after opening Settings and clicking the "Remote browsers" tab:

- Credential stores card: "OS credential store: available" (the worker thread
  opened the Secret Service store without prompting), "Session-only values
  held in memory: 0".
- `device_cloud` card: endpoint, authentication kind, limits and its target;
  `cloud-user` shows "OS store · missing" with a masked field and a "Save to
  OS store" button; `cloud-key` shows "session-only · missing" with a masked
  field and "Set for this session".
- `local_grid` card: the plain-HTTP loopback warning; `grid-token` shows
  "unbound · missing" with the instruction to add a `credential_bindings`
  entry, and no entry field.
- The render loop stayed responsive while the store was probed; no value was
  written to the YAML buffer.

Pointer clicks reach the window in this lane when the window is activated
first (`xdotool windowactivate --sync`, `mousemove` then `mousemove_relative`
into the target, `click 1`), which the earlier headless notes had ruled out.

## macOS and Windows (to run on a real machine)

1. Build or download the branch binary and start it with the smoke config
   below via `--config <file> --ephemeral`.
2. Open Settings, choose "Remote browsers", and confirm the store line reads
   "OS credential store: available" (macOS Keychain, Windows Credential
   Manager). A locked keychain should read "unavailable" with a reason, not
   hang the window.
3. Type a value for `cloud-user` and press "Save to OS store". Expect
   "present" and "Saved to the OS credential store." within a second, and an
   item named `horizon-remote-browser` with account
   `https://grid.example.net|remote-browser/smoke/username` in the platform
   store. macOS may prompt once for keychain access; the app must stay
   responsive during the prompt.
4. Press "Delete" on that reference. Expect "missing" and the item gone.
5. Type a value for `cloud-key` and press "Set for this session". Expect
   "present" and the session counter at 1. Quit Horizon and start it again:
   the counter is 0 and the state is "missing".
6. Confirm the config file on disk never contains a credential value.

Smoke config:

```yaml
version: 10
browser:
  remote:
    providers:
      device_cloud:
        endpoint: https://grid.example.net/wd/hub
        authentication:
          kind: basic
          username_ref: cloud-user
          password_ref: cloud-key
        credential_bindings:
          cloud-user: { store: os_keychain, slot: remote-browser/smoke/username }
          cloud-key: { store: session }
      local_grid:
        endpoint: http://127.0.0.1:4723/wd/hub
        authentication: { kind: bearer, token_ref: grid-token }
    targets:
      ios_phone:
        provider: device_cloud
        browser_name: safari
        platform_name: iOS
        device: { kind: physical, model: iPhone 16, os_version: "18" }
workspaces:
  - name: smoke
    terminals:
      - name: notes
        kind: editor
        position: [40, 40]
        size: [600, 400]
```
