# Remote mobile WebDriver spike: real iOS and Android evidence (2026-09-14)

Phase 1 of [#628](https://github.com/peters/horizon/issues/628). Four sessions
were run from a Linux host with `scripts/remote-browser-spike/spike.py` against
one hosted real-device grid (BrowserStack Automate, WebDriver hub over HTTPS
with Basic authentication). Two sessions per platform: the first run exposed
three gaps, the second run confirmed the fixes. Roughly ten device minutes were
consumed in total and every session was released and confirmed `done` at the
provider.

No Selenium, Appium, driver binary, mobile SDK or provider SDK was installed on
the Horizon host. The fixture is the public static app at
<https://peters.github.io/horizon-mobile-fixture/> (form, drawer, iframe,
long list, viewport probe).

## Devices and outcomes

| Step | Google Pixel 9, Android 16.0, Chrome 149 | iPhone 16, iOS 18.6 (requested 18), Safari 18.6 |
| --- | --- | --- |
| New Session | passed, 16.5 s | passed, 18.0 s |
| Provider metadata after allocation | `Google Pixel 9` / `16.0` / `chrome_android`, `running` | `iPhone 16` / `18.6` / `iphone`, `running` |
| Physical-device evidence | requested equals actual | major version resolved to a patch release |
| Navigate to fixture | passed, 1.0 s | passed, 2.3 s |
| Execute script | passed, 0.19 s | passed, 0.21 s |
| Screenshot | passed, 0.48 s, 1080 x 2251 px, 84 KB, page area only | passed, 0.75 s, 1178 x 2556 px, 553 KB, full screen with status bar and Safari chrome |
| Window rect | 412 x 924 | 393 x 852 |
| Device pixel ratio, visual viewport | 2.625, 411 x 778 | 3, 393 x 659 |
| Scroll: W3C touch swipe | failed (page did not move) | passed, 5.9 s |
| Scroll: W3C wheel action | failed (accepted, page did not move) | failed (`unknown error`, WebDriverAgent accepts pointer and key sources only) |
| Scroll: `mobile: swipe` | unsupported (`unknown command`) | passed, 5.4 s |
| Scroll: `mobile: scroll` | unsupported (`unknown command`) | passed, 9.2 s |
| Scroll: script `window.scrollBy` | passed | passed |
| Element find, value entry, click, form result | passed, 3.6 s | passed, 2.4 s |
| Virtual keyboard | visual viewport shrank from 834 to 778 CSS px while focused | visual viewport did not shrink; Safari toolbar collapse changed it from 659 to 741 instead |
| Fixed drawer open and displayed | passed | passed |
| Iframe enter, click, return to top | passed, returned via `frame/parent` | passed, returned only via `frame` with `id: null`; `frame/parent` left the context in the frame |
| Orientation get, landscape, restore | passed, 6.8 s | passed, 3.1 s |
| Explicit release, provider status polled | passed, 1.5 s, `done` | passed, 1.7 s, `done` |

Timings are single observations from one region and one account, not a
benchmark. Both platforms were still under 20 seconds from New Session to a
controllable page, which is far slower than a local browser launch and confirms
that allocation needs its own timeout and progress reporting.

## Provider differences recorded

- Authentication is Basic on the hub origin; nothing had to be placed in the
  capabilities map. Device selection and `realMobile` live in one namespaced
  options object. Android returned only `goog:chromeOptions` as an extension
  capability; iOS returned none.
- Requested OS versions resolve to a patch release on iOS (`18` became `18.6`)
  and stay exact on Android (`16.0`). Device identity must be compared by
  component prefix, and the resolved version shown to the user.
- The iOS side runs Appium 1.21 (visible in error stack traces). The Android web
  session behaves like chromedriver with a thin Appium layer: orientation works,
  the `mobile:` scroll and swipe extensions do not exist there.
- Chrome on Android reported `window.innerWidth` and `innerHeight` roughly 2.9
  times larger than the visual viewport while the layout, the screenshot and
  `window/rect` all agreed on 412 CSS pixels. `visualViewport` and the
  screenshot size are the trustworthy bases for coordinate mapping; `inner*`
  is not.
- Screenshot coverage differs. Android returns the page area at device pixels
  (width equals visual viewport width times the pixel ratio). iOS returns the
  full device screen, so page coordinates need a top offset for the status bar
  and Safari toolbar before a screenshot pixel can be mapped to a CSS pixel.
- Touch scrolling is not portable across the two paths. A multi-step W3C touch
  swipe scrolls real iOS Safari but not this Android Chrome path, and the wheel
  source is rejected or ignored on both. Script scrolling worked on both and is
  the portable baseline; touch remains an advertised, verified capability per
  session rather than an assumption.
- Returning from an iframe needs the top-level switch (`frame` with `id: null`)
  on iOS; `frame/parent` is not reliable there.
- Release is fast on both, and the provider's session record reaches `done`
  within one poll. This is the evidence the lifecycle must require; a local
  DELETE that times out stays `release-unknown`.

## What this spike does not prove

- Only one provider was exercised. The self-hosted Appium endpoint and any other
  provider still need the same script run before the compatibility phase can
  claim two independent implementations.
- Network capture, video, native permission sheets, browser dialogs and
  private-network tunnels were not attempted and remain unsupported until
  verified.
- Virtual keyboard detection on iOS through `visualViewport` was inconclusive
  and needs a different signal or an honest unknown.
- Latency and screenshot rates were single observations, not the measured
  interactive latency the acceptance criteria require.

## Reproduction

```bash
python3 -B -m unittest discover -s scripts/remote-browser-spike/tests -v
python3 -B scripts/remote-browser-spike/spike.py --target android --out ~/.cache/horizon-628-spike
python3 -B scripts/remote-browser-spike/spike.py --target ios --out ~/.cache/horizon-628-spike
```

The JSON reports and screenshots stay in the private output directory because
they carry the provider session id; only session digests appear on stdout.
