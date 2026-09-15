# Second computer: portable profile import and remote start (2026-09-15)

Part of [#628](https://github.com/peters/horizon/issues/628): a non-secret
remote browser profile exported on one computer is imported on another, the
second computer enters its own credentials, starts a real-device session
through the public MCP tools, releases it, and after a restart without the
stored credential the target is refused until the credential is entered again.
The first computer is the Linux host that ran the earlier evidence; the second
is a rented Windows Server VM with nothing on it but Horizon, the build tools
that produced it and Python for the harness. The macOS leg is still open: the
Mac used for the earlier keychain and safaridriver smokes was unreachable on
this day, so the per-platform box for macOS is not claimed here.

The harness is `scripts/remote-browser-evidence/second_computer_smoke.py`,
with `prepare` on the first computer and `run` on the second. Everything the
run does goes through product paths: `horizon --export-remote-profile`,
`horizon --import-remote-profile`, the OS credential store item Horizon's
keyring adapter addresses, Horizon's own window, and the `browser_*` MCP tools
from an agent identity Horizon injected. The one place the harness stands in
for a person is the binding step: it writes the same `credential_bindings`
entry the Settings > Remote browsers row adds when "OS credential store" is
chosen for an unbound reference, because no pointer is available over a
management channel.

## First computer: export

`prepare` wrote an isolated configuration with the hosted grid's provider,
computer-local `os_keychain` bindings and the `ios_phone` target, then ran
`horizon --config <that> --export-remote-profile <file>`. The file is 738
bytes, starts with `horizon_remote_browser_profile: 1`, and contains no
`credential_bindings`, no slot name, no value and no local path (checked by the
harness before it is handed over). It carries the provider's adapter,
endpoint, authentication shape (`basic` with `user` and `key` references),
limits and the target definition, including the run-unique provider session
name used for the release proof.

## Second computer: Windows Server 2022

Host: `Standard_D4ds_v6` in `northeurope`, Windows Server 2022 Datacenter
Azure Edition (10.0.20348), no public IP, every step through
`az vm run-command`, deleted with its disk and network afterwards. Installed
for the run: Visual Studio 2022 Build Tools (C++ workload), Rust 1.98.1,
Python 3.12.7 (harness only). The source was the `#685` branch at `05d44c8a`
(the content squashed to `main` as 47da2237), fetched as a zip; the Git LFS
font and icon pointers in the archive were hydrated from GitHub's media
endpoint before `cargo build -p horizon-ui --bin horizon`. Nothing else:
`chromedriver`, `geckodriver`, `msedgedriver`, `safaridriver`, `appium`,
`node`, `npm` and `adb` are all absent (recorded by the harness), and no
provider SDK, mobile SDK or browser was installed. The graphics adapter was
the Microsoft Basic Render Driver (software rasterizer) on an interactive
desktop reached through auto-logon, because Horizon's window cannot be
created in session 0.

| Step | Outcome |
| --- | --- |
| `horizon --config config.yaml --import-remote-profile profile.yaml` | exit 0 in 0.6 s: `added 1 provider(s) and 1 target(s), updated 0 provider(s) and 0 target(s)`; the configuration gained the provider and target and still had no bindings |
| Bind `user` and `key` on this computer | `os_keychain` bindings under `remote-browser/browserstack/<reference>` (the row's default slots) |
| Enter the credential on this computer | two Windows Credential Manager items under Horizon's target name (`<origin>|<slot>.horizon-remote-browser`, UTF-8 blob), written with `CredWrite` from the netrc on the VM; never in the profile, the configuration, the logs or the report |
| Horizon started, agent identity injected | yes (the agent panel is a `cmd.exe` probe; see below) |
| `browser_create` with `target: ios_phone` | ready in 31.1 s, `navigation: committed`, `backend: safari`; log: allocating 12:15:42, allocated 12:16:06, `remote device for ios_phone verified: iPhone 16, OS 18.5, physical device` |
| Panel advertises | `remote_target: ios_phone`, `remote_device: iPhone 16, OS 18.5, physical device`, `protocol: web_driver`, network capture unsupported |
| `browser_snapshot` | 40 nodes, title `Horizon mobile fixture` |
| Device probe (`browser_evaluate`) | iPhone OS 18_5 Safari user agent, 5 touch points, 393 x 852, DPR 3 |
| `browser_act fill`, `browser_act click`, `browser_wait`, read result | field holds `Second computer`; `#result` visible after 2.5 s; `result:Second computer` |
| `browser_close`, `browser_list` | `closed: true`; no panels |
| Provider status after close | `done`, iPhone 16 / 18.5, 44 s billed |
| Remove the two Credential Manager items, restart Horizon, `browser_create` again | refused: `credentials_not_ready`: ``credential `user`: credential is missing; enter or unlock it in Settings > Remote browsers``; no value in the reply; `browser_list` empty; `cmdkey /list` shows no Horizon item |

The restart step is the runtime half of "closing Horizon clears session-only
credentials; restart requires explicit re-entry": nothing about the value
survives outside the store the user chose. The session store itself is
process memory (`session_store_clears_replaces_and_deletes`,
`workbench_session_values_are_immediate_and_store_bound`,
`drafts_are_wiped_including_spare_capacity` on `main`); it cannot outlive the
process, and the Remote browsers tab reports the count it holds with a
"Clear all" action.

## Rehearsal on the first computer

The same `run` half was rehearsed on the Linux host beforehand against the
Secret Service (run `run-second-linux-1789470354`): import exit 0, bindings
added, `browser_create` ready in 35.7 s with the same verified identity
(iPhone 16, OS 18.5, physical), fill and submit passed, `browser_close`
returned `closed: true`, the provider reported `done` after 43 s, and the
restart without the stored items was refused as `credentials_not_ready`.

## What the harness had to work around

- Windows agent panels: Horizon wraps every agent command in `$SHELL -ic
  <command>`, a POSIX login-shell convention with no Windows default, so the
  panel launched `/bin/bash` and failed with "The system cannot find the file
  specified". The harness sets `SHELL` to a two-line batch shim that unwraps
  the quoted command and runs a batch probe. Filed as #688; it does not touch
  the remote browser path.
- Session 0: from `run-command` or a SYSTEM task the DX12 swapchain cannot be
  created ("Invalid surface"). The run used an at-logon scheduled task with an
  interactive principal on the auto-logged-on desktop.
- The `select`-based MCP client in `scripts/browser-smoke` does not work on
  Windows pipes; the harness carries a threaded equivalent.

## Reproducing

```
# first computer
python3 scripts/remote-browser-evidence/second_computer_smoke.py prepare \
  --horizon <horizon> --run-name <name> --targets ios_phone
# second computer, with the exported profile copied over
python3 scripts/remote-browser-evidence/second_computer_smoke.py run \
  --horizon <horizon> --run-name <name> --targets ios_phone --profile <profile.yaml>
```

`run` reads the netrc named by `HORIZON_NETRC` (default
`~/.config/horizon-dev/browserstack.netrc`, mode 600 on POSIX), seeds this
computer's OS store (Secret Service, macOS Keychain with an optional
`--keychain`, or Windows Credential Manager) after recording what Horizon's
items held, and puts that back at the end (or removes the items), also when
the run is terminated by a signal. Reports land under `--out` as `report.json` with `rpc-<target>.jsonl`,
`rpc-restart.jsonl`, `horizon.log` and `horizon-restart.log`.
