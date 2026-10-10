---
procedure: native-ios-deep-links
feature: iOS native deep links
platforms: [linux, macos]
cost: paid device
destructive: yes
secrets: Horizon OS credential-store references
owner: peters
---

# iOS deep link test procedure

## Purpose

Test the declared custom URL scheme on real iPhones through App Automate.
This procedure does not qualify a Simulator as a real device.

## Preconditions

Read [the native runbook](../../architecture/remote-device-testing.md) and
[the matrix procedure](native-app-automate.md). Use the approved native quota,
project contract, private client file, and one immutable IPA file.
Record the candidate commit, executable SHA-256, and IPA SHA-256.
Keep credentials and evidence private. Use an isolated synthetic backend per lane.

## Tasks

1. Run a recipe with `deep_link` on the declared real iPhone matrix.

   Result: The driver sends the declared URL and bundle ID once.

2. Observe each live endpoint through an owned Device panel.

   Result: The first inspection confirms a displayed image. Three timestamped
   inspections show frames that advance. Record later presentation separately.
   If the person moves the viewer outside the canvas, keep that navigation.

3. Examine the English system confirmation, if it appears.

   Result: The host selects **Open** only when the alert has the expected text
   and exactly **Cancel** and **Open** buttons. Permission alerts stay intact.

4. Run a semantic assertion for the requested screen after each deep link.

   Result: Each assertion passes, and the retained step has a screenshot.

5. Examine the driver failure path with the local scripted tests.

   Result: An unsupported command has `app_action_unsupported` and a bounded,
   redacted driver reason. Transport failure does not repeat the URL command.
   The original lifetime also bounds the confirmation check. Use a fixture with
   slow **no such alert** replies. The successful URL command must stay successful.
   Alert checks and any confirmation must finish within 15 seconds.

6. Examine the terminal report and the exact cleanup receipts.

   Result: The report names the actual devices and the tested candidate.
   All owned sessions, uploads, tunnels, backends, and viewers close.

## Runtime limits

The driver needs XCUITest 4.17 or later, Xcode 14.3 or later, and iOS 16.4 or later.
The host recognizes English confirmation text. Other text returns a typed refusal.
A failed URL command can leave a confirmation on the device. Inspect that state
before another action. Do not repeat an uncertain effectful request.

See the [upstream execute methods](https://appium.github.io/appium-xcuitest-driver/latest/reference/execute-methods/).

Horizon selects Appium 2.19.0 for iOS 15 or later. This version uses XCUITest 9.9.6.
Older iOS versions keep the provider default and cannot qualify the deep-link test.
See the [provider version table](https://www.browserstack.com/docs/app-automate/appium/set-up-tests/set-appium-version).
