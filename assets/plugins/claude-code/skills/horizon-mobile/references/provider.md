# The provider calls of the tool

This page holds the REST and WebDriver calls that `scripts/mobile_session.py`
makes, so that a person can check them and so that a later provider can be added.
It also holds the commands that prove the state of the account.

## The two base addresses of the tool

| Name | Default | Role |
|---|---|---|
| `HORIZON_MOBILE_HUB` | `https://hub-cloud.browserstack.com/wd/hub` | The WebDriver hub of the device |
| `HORIZON_MOBILE_API` | `https://api-cloud.browserstack.com/app-automate` | The REST API of the app and the sessions |

Both names exist for the test of the tool, which sets them to a fake cloud on a
loopback port. An agent does not set them.

## The credential

Every call sends HTTP Basic authentication with the user name and the access key
of `BROWSERSTACK_USERNAME` and `BROWSERSTACK_ACCESS_KEY`. The tool replaces both
values with `<hidden>` in every text that it prints, and an error names the path
of the call and not the host.

## The REST calls

| Call | Answer | Used by |
|---|---|---|
| `GET <API>/devices.json` | a JSON list; each row holds `os`, `device`, `os_version` and `realMobile` | `devices` |
| `POST <API>/upload` | `{"app_url": "bs://<id>"}` | `upload` |
| `GET <API>/sessions/<id>.json` | `{"automation_session": {"status", "duration", "browser_url"}}` | `status`, `start`, `stop` |

The upload is `multipart/form-data` with the field `file` for a file of this
computer, or with the field `url` for a public `https` link. The tool sends no
other field.

`browser_url` is the page of the session. While the session runs, the page holds
a live view of the phone: the tool answers it as `watch`. After `stop`, the page
holds the video and the logs of the session: the tool answers it as `recording`.

## The WebDriver calls

The hub is W3C WebDriver with the Appium extensions. Each answer is a JSON object
with the field `value`.

| Call | Body | Used by |
|---|---|---|
| `POST <HUB>/session` | `capabilities.alwaysMatch` with `platformName`, `appium:app`, `appium:deviceName`, `appium:platformVersion`, `appium:automationName` and `bstack:options` | `start` |
| `DELETE <HUB>/session/<id>` | none | `stop` |
| `GET <HUB>/session/<id>/source` | none | `screen`, `wait` |
| `POST <HUB>/session/<id>/element` | `{"using": <locator>, "value": <text>}` | `tap`, `type` |
| `POST <HUB>/session/<id>/element/<element>/click` | `{}` | `tap`, `type` |
| `POST <HUB>/session/<id>/element/<element>/value` | `{"text": <text>}` | `type` |
| `POST <HUB>/session/<id>/actions` | one W3C touch pointer with a move, a down, a pause, a move and an up | `tap --at`, `swipe` |
| `POST <HUB>/session/<id>/back` | `{}` | `back` |
| `GET <HUB>/session/<id>/window/rect` | none | `swipe` |
| `GET <HUB>/session/<id>/screenshot` | none; the answer is base64 PNG | `screenshot` |
| `POST <HUB>/session/<id>/appium/device/hide_keyboard` | `{}` | `hide-keyboard`, `swipe` |
| `POST <HUB>/session/<id>/execute/sync` | `{"script": "mobile: activateApp", "args": [{"appId": <app>}]}` | `open` |
| `POST <HUB>/session/<id>/execute/sync` | `{"script": "browserstack_executor: {...}", "args": []}` with the action `setSessionStatus` | `stop --passed`, `stop --failed` |

The locators are:

| Locator | Platform | Value |
|---|---|---|
| `id` | Android | the `resource-id` |
| `accessibility id` | iOS | the accessibility identifier |
| `xpath` | both | an `xpath` |
| `-android uiautomator` | Android | `new UiSelector().textContains("TEXT")` |
| `-ios predicate string` | iOS | `label == "TEXT" OR name == "TEXT" OR value == "TEXT"` |
| `-ios class chain` | iOS | the text fields of the screen at `[N]` |

The capabilities of `start` hold `bstack:options` with `projectName`,
`buildName`, `sessionName`, `idleTimeout: 90` and `interactiveDebugging: true`.
`idleTimeout` is the 90 seconds after which the provider ends a session that gets
no command.

The session capabilities of the answer name the app: `appium:appPackage` on
Android and `appium:bundleId` on iOS. The tool keeps that value in the state file
and `open` reads it.

## The commands of the provider that a person runs

These calls need no WebDriver session, so a person can run them from a shell
before and after a test. Use the same two names from the environment and never
write the values in a file or in an issue.

```
curl -s -u "$BROWSERSTACK_USERNAME:$BROWSERSTACK_ACCESS_KEY" \
    https://api-cloud.browserstack.com/app-automate/devices.json
curl -s -u "$BROWSERSTACK_USERNAME:$BROWSERSTACK_ACCESS_KEY" \
    https://api-cloud.browserstack.com/app-automate/plan.json
```

- `devices.json` is the device list of `devices`.
- `plan.json` holds the plan of the account, its device minutes and the parallel
  sessions that it allows. Read it before a run of many sessions, and after a run
  to prove that no session still runs.

The tool answers `status` from `sessions/<id>.json` of the session of the state
file. A state of `running` means that the session bills its minutes: run `stop`.

## A second provider

The tool keeps the provider in four places, so a second provider is a small
change:

1. The two base addresses and the credential: the `hub()` and `api()` functions
   and `credentials()`.
2. The device list: `cmd_devices` reads the shape of the answer of the provider.
3. The two links: `watch` while the session runs and `recording` after `stop`.
   They come from the page of the session.
4. The four extensions: the automation name, the locators, `hide_keyboard`,
   `activateApp` and `setSessionStatus`.

Keep the shape of one command, one JSON object, exit code 1 with
`{"error": ...}`, the state file at `0600`, and the replacement of both
credential values in every printed text. Add a fake cloud of the new provider to
`scripts/test_mobile_session.py`, and keep the tests of the current provider
green: no test reaches a real cloud.
