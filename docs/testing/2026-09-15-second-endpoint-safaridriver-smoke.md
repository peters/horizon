# Second endpoint: Apple safaridriver on a plugged-in iPhone (2026-09-15)

Phase 5 of [#628](https://github.com/peters/horizon/issues/628): compatibility
evidence from a second, independent WebDriver implementation, not a second
device on the same service. The endpoint is Apple's `safaridriver` (Safari
26.6.2) on a Mac Studio, driving Safari on an iPhone 16 Pro Max running iOS
27.0 plugged into it over USB, reached from the Linux host through an SSH
tunnel (`ssh -N -L 4444:127.0.0.1:4444 fintermac`). Horizon's configuration
names it as a `webdriver` provider at `http://127.0.0.1:4444` with
`authentication: none` and an `ios_safaridriver` target (`device.kind: any`,
`safari:deviceUDID` as its one extension). Nothing was installed for it: no
Appium, no Node service, no provider SDK; the driver ships with Safari.

The same evidence script drove the Horizon flow (`main` at 47c0b042):
`browser_create` with the target, snapshot, device probe, fill, submit, drawer,
iframe query, scroll, `browser_close`, then a fresh direct session as the
release proof (the driver allows one session per device). Because the flow's
input steps failed, the driver was then exercised directly through the tunnel
with the same W3C commands, so the result attributes each behaviour to the
driver or to Horizon.

## What the endpoint needed

- On the Mac: Remote Automation allowed for Safari (already the case on this
  Mac); `safaridriver -p 4444` runs as the logged-in user.
- On the phone: Settings > Safari > Advanced > Remote Automation on (the
  driver refuses with "Remote Automation is turned off" otherwise), Web
  Inspector on, the phone unlocked while a session is created ("device is
  locked" otherwise), and the USB connection stable (the driver's device list
  changed twice during setup as the cable was reseated).
- Session creation took 1.6 s from Horizon's request to a controllable page,
  against 20 to 30 s at the hosted grid.

## Horizon flow on the second endpoint (`run-1789453384`)

| Step (MCP tool) | Outcome |
| --- | --- |
| `browser_create` with `target` and `url` | ready in 3.3 s, `navigation: committed`, `backend: safari` |
| Panel advertises | `remote_target: ios_safaridriver`, `remote_device: unverified hardware`, `protocol: web_driver`, network capture unsupported |
| `browser_snapshot` | passed, title `Horizon mobile fixture` |
| Device probe (`browser_evaluate`) | iPhone UA (frozen at `iPhone OS 18_7` with `Version/27.0`), 440 x 796 CSS px viewport, DPR 3 |
| `browser_act fill` then read the field | accepted by the driver, field still empty |
| `browser_act click` on submit, read result | accepted by the driver, `result:none` |
| Drawer open (`browser_act click`, `browser_wait` on `#drawer.open`) | accepted by the driver, drawer never opened (wait timed out) |
| Iframe boundary (`browser_query`) | passed, one `iframe` node with bounds |
| `browser_act scroll` then read `scrollY` | 600 |
| `browser_close` | `closed: true`, panel gone from `browser_list` |
| Release proof (fresh direct session) | opened and deleted, so the previous session was released |

`remote_device` is `unverified hardware` because the generic adapter reads
Appium-style capabilities, and this driver reports the device as
`safari:deviceName`, `safari:platformVersion` and `safari:useSimulator: false`
instead (follow-up below). The target therefore used `device.kind: any`.

## The driver on its own, same commands

| W3C command through the tunnel | Result on this driver and OS |
| --- | --- |
| New Session, navigate, title, execute script, find element, frame switch | work |
| Element Clear then Element Send Keys on `#name`, then read the value | both return success; the value stays empty; the field becomes the active element |
| Element Click on `#submit`, then read `#result` | returns success; `result:none` |
| Touch pointer action at the button's centre | returns success; no effect |
| Mouse pointer action at the same point | the session is gone afterwards (`invalid session id` on the next command) |
| Key actions | not reached in that session (see above); Horizon's remote path does not use them since #653 |
| Scripted `element.click()` and value assignment through execute script | work: `result:Horizon 628`, drawer opens |
| Take Screenshot | works, 560 KB PNG |
| Window Rect | returns all zeros |
| Get Orientation | `unknown command` |

Read, navigate, script and screenshot paths therefore work across both
endpoint implementations, and Horizon's remote input path (Find Element plus
Element Send Keys and Element Click, the commands the hosted grid honours on a
real iPhone 16) is accepted but has no effect on this driver with iOS 27.0.
That is the driver's behaviour, not Horizon's: the direct probe shows the same
commands doing nothing. Whether this is specific to the iOS 27.0 pre-release
on that phone is not known; it is recorded as unsupported for this
combination, and the scripted click the evidence script already uses as an
explicit fallback for the drawer is the only input path that changed the page.

## What this proves for the acceptance items

- Two independent endpoint implementations (a hosted Appium-based grid and
  Apple's own driver over a tunnel) accept the same provider-neutral
  configuration and the same MCP flow; the differences are recorded as
  explicit support results above.
- Allocation, release and the release proof behave the same way on both:
  `browser_close` returns only once the release is established, and the
  endpoint accepted a fresh session immediately afterwards.
- A generic endpoint that echoes no Appium capabilities cannot satisfy a
  `physical` requirement, as designed; this one echoes Safari's own identity
  fields, which a later change can read.

## Follow-ups

- Read `safari:deviceName`, `safari:platformVersion` and `safari:useSimulator`
  as identity evidence on the generic adapter, so a safaridriver target can
  require `physical` ([#668](https://github.com/peters/horizon/issues/668)).
- The input path on safaridriver with iOS 27.0: rerun when the phone is on a
  released iOS build, and consider a scripted-input fallback for remote
  sessions whose driver accepts element commands without effect (#663 covers
  the Android tap case).
