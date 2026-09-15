# Remote device evidence run

Phase 6 of [#628](https://github.com/peters/horizon/issues/628): drive a real
remote mobile device through Horizon's public `browser_*` MCP tools only, from a
headless Horizon on Linux, and record the result with the provider's own release
confirmation. It is the product path end to end: nothing here talks WebDriver,
and nothing is installed on the host beyond Horizon and the run tooling. The
outcome of the 2026-09-14 runs is in
`docs/testing/2026-09-14-remote-mobile-mcp-live-evidence.md`.

Requirements: a Horizon binary built from the branch under test, Xvfb, openbox,
ImageMagick's `import`, Python 3.11+ with the `gi` Secret bindings, a running
Secret Service (gnome-keyring), and a netrc at
`~/.config/horizon-dev/browserstack.netrc` with mode 600 holding the provider's
automation username and access key as a `machine` entry for the hub host; the
same credential authenticates the REST status call. The credential is read from
that file only and seeded into the Secret Service under the item Horizon's
keychain adapter addresses for the run; afterwards the slots are restored to
whatever they held before (or cleared). It never enters arguments, the
environment of the Horizon process, the generated config, the logs or the
report. The provider session is named `phase6-<target>-run-<epoch>` so the
release proof queries exactly this run's session.

## Second endpoint (optional)

Apple's `safaridriver` on a Mac with an iPhone plugged in is an independent
WebDriver implementation. Start it on the Mac (`safaridriver -p 4444`, with
Remote Automation allowed in Safari on the Mac and enabled on the phone under
Settings > Safari > Advanced), open a tunnel from this host
(`ssh -N -L 4444:127.0.0.1:4444 <mac>`), and set
`HORIZON_SAFARIDRIVER_UDID=<device udid>` (and `HORIZON_SAFARIDRIVER_ENDPOINT`
if not `http://127.0.0.1:4444`). The script then adds a `safaridriver` provider
(generic `webdriver` adapter, no credentials) and an `ios_safaridriver` target
that can be named in `--targets`; its release proof opens and deletes a fresh
session, since the driver allows one session per device.

## Command

```
scripts/remote-browser-evidence/run.sh <horizon binary> --targets ios_phone android_phone
```

`run.sh` starts Xvfb on the first free display from `:99` with a window manager,
cleans up only the processes it started on any exit, and hands the rest to
`live_smoke.py`, which writes an isolated Horizon home and config naming the
targets, starts Horizon with `--ephemeral`, reads the agent identity from the
probe the agent panel writes, and then runs per target: `browser_create` with
`target` and the fixture URL, `browser_snapshot`, a device probe, `fill` and
`click` with a read-back of the form result, drawer open and close, an iframe
query, `scroll`, `browser_close`, `browser_list`, and finally the provider's
REST status of the session. Each device allocation is billed by the provider;
one run is roughly forty seconds of device time per target.

The drawer-close step records its `method`: `driver_click` when the driver's tap closed the drawer, `scripted_click` when only the page's own handler did (an explicit conditional result, see peters/horizon#663). The run exits nonzero, and `report.json` names the shortfall per target, unless every required outcome held: a committed first page, the field holding the typed value, the submitted result, the drawer, the iframe node, a moved page, `closed: true`, an empty `browser_list`, and a terminal session at the provider. The REST status call refuses redirects, so the credential is never resent elsewhere.

Output lands under `~/.cache/horizon-628-spike/phase6/run-<epoch>`:
`report.json` (every step with its outcome), `rpc-<target>.jsonl` (every MCP
request and reply), `horizon.log`, `config.json` (the generated Horizon config),
and five panel screenshots per target.
