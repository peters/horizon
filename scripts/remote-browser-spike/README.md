# Remote browser protocol spike

Phase 1 of [#628](https://github.com/peters/horizon/issues/628): prove, on real
hosted mobile devices, that classic W3C WebDriver over HTTPS is enough for a
first usable remote session and record where providers differ. The script is
standard-library Python so the run demonstrates exactly what a Horizon-internal
client must implement: no Selenium, Appium, provider SDK or driver binary on the
Horizon host. It is a measurement tool, not the product path.

Requirements: Python 3.9+, network access, and a netrc file holding the
provider's automation username and access key as exact `machine` entries for both the hub host and the API host (the `default` stanza is ignored). The default is
`~/.config/horizon-dev/browserstack.netrc` with mode 600. The script never reads
credentials from arguments or the environment, sends them only to the configured
hub and API origins, refuses plain HTTP, URL userinfo and query strings, and does
not follow redirects.

## Commands

Offline tests (no network, no credentials, no device time):

```bash
python3 -B -m unittest discover -s scripts/remote-browser-spike/tests -v
```

One real-device run (allocates a billable device for roughly two minutes and
releases it explicitly; the exit code is non-zero if any step failed or ended
in an unknown state):

```bash
python3 -B scripts/remote-browser-spike/spike.py --target ios --out ~/.cache/horizon-628-spike
python3 -B scripts/remote-browser-spike/spike.py --target android --out ~/.cache/horizon-628-spike
```

`--out` must be a private directory: it receives the JSON report, which contains
the provider session id, and the PNG screenshots. Stdout shows only a digest of
the session id. `--hub`, `--api`, `--url` and `--build` override the defaults.

## What one run checks

1. New Session with `realMobile` requested through the provider's namespaced
   options; an allocation timeout is reported as `unknown` and never retried.
2. Provider session metadata as the physical-device evidence, compared against
   the requested device and OS version (a provider may resolve `18` to `18.6`).
3. Navigation to the synthetic fixture at
   <https://peters.github.io/horizon-mobile-fixture/>, script execution,
   viewport and pixel-ratio metrics, a PNG screenshot and its pixel size.
4. Five scroll mechanisms, each reset to the top first: W3C touch swipe, W3C
   wheel action, Appium `mobile: swipe`, Appium `mobile: scroll`, and script.
5. Element find, value entry with the virtual keyboard, click and form result.
6. A fixed-position drawer, an iframe round trip (enter, click, return to top),
   and Appium orientation change and restore.
7. Explicit release, then the provider's session record polled until it reports
   a terminal status. Local success alone never counts as released.

The findings from the 2026-09-14 runs are recorded in
[`docs/testing/2026-09-14-remote-mobile-webdriver-spike.md`](../../docs/testing/2026-09-14-remote-mobile-webdriver-spike.md)
and the contracts they informed in
[`docs/architecture/remote-browser-sessions.md`](../../docs/architecture/remote-browser-sessions.md).
