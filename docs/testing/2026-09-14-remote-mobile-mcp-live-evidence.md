# Remote mobile devices through Horizon's public MCP tools: live evidence (2026-09-14)

Phase 6 of [#628](https://github.com/peters/horizon/issues/628). Horizon ran
headless on a Linux host with two configured remote targets at one hosted
real-device grid (BrowserStack Automate, WebDriver hub over HTTPS with Basic
authentication). An agent identity injected by Horizon drove each device
through the `horizon-browser` MCP server only: no raw WebDriver call, no
provider SDK, no driver binary, no Appium or Selenium on the host. The fixture
is the public static app at <https://peters.github.io/horizon-mobile-fixture/>
(form, drawer, iframe, long list, viewport probe).

The driver is `scripts/remote-browser-evidence/live_smoke.py` (with `run.sh`
for a private Xvfb display and window manager). It seeds the provider credential from a
mode-600 netrc into the Secret Service under the exact item Horizon's keychain
adapter addresses, writes an isolated Horizon config that names the two targets,
starts Horizon with `--ephemeral`, reads the agent panel's identity from the
probe file it writes, and then speaks JSON-RPC to the MCP server the same way an
agent does. Every request and reply is kept in `rpc-<target>.jsonl`, the panel is
photographed at five points, and after `browser_close` the provider's REST API is
asked whether the session is terminal. The credential is removed from the Secret
Service at the end and never appears in the config, the logs, the report, or the
MCP traffic.

The binary under test was built from the create-path branch (#651) with the fill (#653) and click (#654) fixes applied, all three merged since with the same content. Seven runs were made in total: two early attempts that ended before the flow
(the MCP client did not share Horizon's isolated HOME, and a wait call lacked
its `state`) and five complete runs, about four device minutes confirmed
released by the provider. The last one, `run-1789401014`, is the evidence
below. The earlier runs found
two defects in the remote input path, both fixed before the final run:

- The first run left the form field empty on the iPhone: the classic path typed
  through W3C key actions aimed at the focused element, which iOS Safari did not
  turn into text. #653 routes a remote fill through Find Element, Element Clear
  and Element Send Keys.
- With the fill fixed, the iPhone still did not submit: `browser_act click`
  reported `completed`, the on-screen keyboard rose and the name field stayed
  the active element, so the pointer action at the button's page rectangle
  (16, 193) landed on the field above it. A diagnostic run confirmed it (visual
  viewport 659 CSS px before the click, 436 after; `activeElement` still `name`).
  The Pixel submitted through the same path. #654 routes a remote
  single click through Find Element and Element Click, and the final run
  submitted on both devices.

## Devices and outcomes

| Step (MCP tool) | iPhone 16, iOS 18.6 (requested 18), Safari | Google Pixel 9, Android 16.0, Chrome 149 |
| --- | --- | --- |
| `browser_create` with `target` and `url` | ready in 24.2 s, `navigation: committed`, `backend: safari` | ready in 22.5 s, `navigation: committed`, `backend: chromium` |
| Panel advertises | `remote_target: ios_phone`, `protocol: web_driver`, network capture unsupported | `remote_target: android_phone`, `protocol: web_driver`, network capture unsupported |
| `browser_snapshot` | 60 nodes, title `Horizon mobile fixture` | 60 nodes, title `Horizon mobile fixture` |
| Device probe (`browser_evaluate`) | iPhone UA, 5 touch points, screen 393 x 852, DPR 3 | Android UA, 5 touch points, screen 412 x 924, DPR 2.625 |
| `browser_act fill` then read the field | field holds `Horizon 628` | field holds `Horizon 628` |
| `browser_act click` on submit, `browser_wait`, read result | `result:Horizon 628` | `result:Horizon 628` |
| Drawer open and close (`browser_act click`, `browser_wait`) | passed | passed |
| Iframe boundary (`browser_query`) | one `iframe` node, `Embedded frame`, bounds reported | one `iframe` node, `Embedded frame`, bounds reported |
| `browser_act scroll` then read `scrollY` | 600 | 1506 |
| `browser_close` | `closed: true`, panel gone from `browser_list` | `closed: true`, panel gone from `browser_list` |
| Provider status after close | `done`, `iPhone 16` / `18.5`, 41 s billed | `done`, `Google Pixel 9` / `16.0`, 41 s billed |

Timings are single observations from one region and one account. Allocation
dominates: both devices took more than twenty seconds from `browser_create` to a
controllable page, which is why the create path has its own allocation timeout
and why `browser_create` waits for readiness before returning.

Seventeen MCP calls per target: `browser_create`, `browser_snapshot`, five
`browser_evaluate`, five `browser_act`, two `browser_wait`, `browser_query`,
`browser_close` and `browser_list`. No call named a provider, an endpoint, a
capability namespace or a credential; the only remote-specific input was the
configured target name.

## What each acceptance item got

- Real iOS and Android targets loading an HTTPS test application through the
  public MCP tools: both passed.
- Navigation: the create-time URL committed on both devices; `browser_navigate`
  was not exercised separately in this flow.
- Form input and submission: passed on both after the two fixes above.
- Touch scrolling: `browser_act scroll` moves the page on both devices, but on a
  classic WebDriver session it is implemented as a scripted scroll, not a touch
  swipe. Recorded as scripted scroll supported, touch swipe not offered through
  the MCP contract (the phase 1 spike documents which native swipe commands each
  device accepts).
- Modal or drawer: opened and closed on both, verified with `browser_wait` on
  the drawer's visibility.
- Iframe context: the snapshot and query expose the frame as an `iframe` node
  with its bounds; entering the frame is not part of the MCP contract (the
  documented path is `browser_handoff` on the same panel), so frame-internal
  interaction is recorded as not exercised.
- Virtual keyboard: on the iPhone, a tap into the field shrank the visual
  viewport from 659 to 436 CSS px while the layout viewport stayed at 659; on
  the Pixel the visual viewport was 778 CSS px while `innerHeight` reported
  2262. Horizon's coordinate mapping uses the frame size, and the final run's
  clicks landed correctly with the keyboard up on both devices.
- Screenshot output: the panel rendered the device frames throughout (five
  panel screenshots per target in the run directory).
- Explicit release: `browser_close` returned `closed: true` only after the driver
  established the release, and the provider reported the session `done`
  afterwards on both devices. A vanished panel was never taken as proof.
- Unsupported features are advertised, not discovered by failure: the panel
  reports network capture unsupported with a workflow note, and video capture
  is absent from the capability list.

## Not proven here

- Requested versus actual hardware is not verified by the create path (the
  provider resolved iOS `18` to `18.5` on the device and `18.6` in its own
  record); the target's `kind: physical` is configuration, and the schema says
  so.
- Only one endpoint implementation was exercised. The second, self-hosted
  Appium endpoint is a separate item.
- The runs were on Linux; the macOS and Windows keychain smokes are separate.

## Reproducing

```
scripts/remote-browser-evidence/run.sh <horizon binary> --targets ios_phone android_phone
```

Requirements: a netrc at `~/.config/horizon-dev/browserstack.netrc` (mode 600)
with a `machine` entry for the hub host (the same credential authenticates
the provider's REST status call), a Secret Service session (gnome-keyring), Xvfb,
openbox, ImageMagick's `import`, and the `gi` Secret bindings. The run directory
lands under `~/.cache/horizon-628-spike/phase6/run-<epoch>` with `report.json`,
`rpc-<target>.jsonl`, `horizon.log`, the generated config and the screenshots.
