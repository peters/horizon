---
name: horizon-mobile
description: >-
  Drive one app on one real phone of the BrowserStack App Automate device cloud from your own
  shell, one step at a time, and show the person a card that watches the session. Use it when
  the person asks to test an app on a phone, on Android or iOS, on a real device, to tap
  through a flow of a mobile app, or to prove what a mobile app shows. The card uses block_panel
  (horizon-block). This skill does not build an app and does not write one.
---

# Horizon mobile

Use this skill to run one app on one real phone and to show what happens on the
screen. The tool `scripts/mobile_session.py` of this skill folder speaks the
WebDriver protocol of the device cloud, so one command of your own shell is one
step of the test. `block_panel` draws the card that the person watches, and
`horizon-block` holds the contract of that card.

## The model

- One call of the tool does one thing and prints one JSON object. Look at it,
  decide, and call again.
- The session id is in `./.mobile-session.json` of the folder where you run the
  tool. Run every call in the same folder, and name your screenshot files in
  that folder.
- The state file is `0600` and it holds the app of the session. `stop` deletes it.
- The tool reads `BROWSERSTACK_USERNAME` and `BROWSERSTACK_ACCESS_KEY` from the
  environment. It never prints either of them.
- The provider runs the phone. Horizon starts no emulator, holds no phone and
  holds no provider credential of its own.
- The card shows the state that you read from the provider, the `watch` link of
  the session and the last steps. The person's clicks come back to you through
  `next_click`.

## Preconditions

Do these before the first `start`, and stop and tell the person when one fails.

1. The two named secrets. Ask the person to open **Settings → Secrets** and to
   store the two values under the exact names `BROWSERSTACK_USERNAME` and
   `BROWSERSTACK_ACCESS_KEY`. Both names pass the rule of a named secret: 1 to
   64 characters of `A-Z`, `0-9` and `_`, starting with `A-Z`. An agent panel
   that starts after the person saved them reads them as environment variables.
   A panel that was already open keeps its own environment: ask the person to
   open a new agent panel after a change.
2. An app file that the person names. The file is `.apk` or `.aab` for Android,
   or a signed `.ipa` for iOS. The repository of the app builds it, as its own
   `AGENTS.md` says. This skill does not build, sign or change an app, and you
   never upload an app that the person did not name.
3. The account. Each minute of a session uses the device minutes of the account
   of the person. Read [Cost and clean-up](#cost-and-clean-up) before `start`.

A local backend of the app needs the local tunnel of the provider, which this
skill does not start. Ask the person for a public address of the backend
instead, or test the flow that needs no backend.

## The loop

Run the calls in this order. Every call answers one JSON object; a call that
fails prints `{"error": "<text>"}` and exits 1.

### 1. Find the device

```
python3 -B scripts/mobile_session.py devices --platform android
python3 -B scripts/mobile_session.py devices --platform ios --search iPhone
```

Pick a device and its `os_version` from the answer. The list holds real devices
and leaves out an emulator. Show at most 60 rows per call.

### 2. Upload the app

```
python3 -B scripts/mobile_session.py upload /path/to/app-debug.apk
```

The answer holds the app id, such as `bs://<id>`. An upload of a file of 100 MB
can take a minute. Upload one app for one test run and use its id for every
`start` of that run.

### 3. Start the session

```
python3 -B scripts/mobile_session.py start --platform android \
    --device "Google Pixel 8" --os-version 14.0 --app bs://<id> --name "sign-in check"
```

A start takes about 20 seconds. The answer holds the session, the device, the
app of the session, the elapsed seconds and the `watch` link. Show the card of
[the card](#the-card) now, before the first `screen`.

A second `start` in one folder is refused while a session is open. That is the
guard that keeps two sessions and two bills from one run.

### 4. Look at the screen

```
python3 -B scripts/mobile_session.py screen
```

The answer holds one row for each element that the person can see, with `kind`,
`text`, `id` when the row names an element, and `tap` for a control. It holds
at most 60 rows and the number of rows that it leaves out; `screen --all` lifts
that cap. Use the rows to choose the next step. Read
[the selectors of Android](references/android.md) or
[the selectors of iOS](references/ios.md) when a row is not what you expect.

### 5. Act

```
python3 -B scripts/mobile_session.py tap --text "Sign in"
python3 -B scripts/mobile_session.py tap --id item-4
python3 -B scripts/mobile_session.py tap --at 120,240
python3 -B scripts/mobile_session.py type --text "agent@example.test"
python3 -B scripts/mobile_session.py type --text "a password" --field 2 --enter
python3 -B scripts/mobile_session.py swipe up
python3 -B scripts/mobile_session.py hide-keyboard
python3 -B scripts/mobile_session.py wait --text "Welcome" --seconds 20
python3 -B scripts/mobile_session.py back
python3 -B scripts/mobile_session.py open
python3 -B scripts/mobile_session.py screenshot step-4.png
```

One command takes 0.3 to 2 seconds. Look at the screen again after each act, and
update the card between the steps.

### 6. Stop the session

```
python3 -B scripts/mobile_session.py stop --passed --reason "the form is signed in"
```

Use `--failed --reason "<text>"` when the test finds a fault. The answer holds
the state of the session, its seconds and the `recording` link of the video and
logs. `stop` ends the session and deletes the state file. Put the `recording`
link in your final answer.

## The card

Make one card with `block_panel`. This is the document of a session of two
steps:

```json
{"operation": "create", "title": "Sign in check",
 "document": {"components": [
   {"type": "heading", "text": "com.example.app", "subtitle": "Google Pixel 8, Android 14.0",
    "badge": {"text": "Running", "tone": "ok"}},
   {"type": "kv", "rows": [
     {"label": "Session", "value": "sess-012", "mono": true},
     {"label": "State", "value": "running"},
     {"label": "Step", "value": "2 of 6"}]},
   {"type": "link", "label": "Watch the phone", "url": "https://cloud.example.test/sessions/watch-1"},
   {"type": "text", "text": "1. Screen: Sign in, agent@example.test, secret, Continue.\n2. Type into the first field."},
   {"type": "buttons", "items": [
     {"id": "stop", "label": "Stop", "tone": "danger"},
     {"id": "again", "label": "Run again"}]}]}}
```

The rules of the card:

- The heading is the app and the device. Its badge is the state: `Running` with
  `ok`, `Passed` with `ok`, `Failed` with `danger`, and `Ended` with `warn` when
  the provider ended the session.
- The `kv` holds the session, the state and the step count. Add the elapsed
  seconds only when you read them from `status`.
- The `link` is the `watch` link of `start` or of `status`. It opens the page of
  the provider with a live view of the phone in a browser panel of the workspace.
  The person must be signed in to the provider in that browser. A page that asks
  for a sign-in is not a fault of the session.
- The last few steps are `text` lines with the step number and what the screen
  showed. Keep them short and keep only the last three.
- Two buttons: `Stop` in tone `danger` and `Run again`. No other button.
- Update the whole document after each step with `update` and the `panel_id`.
- Never put the user name, the access key or a text that the person typed as a
  secret on the card.

Read a click as `horizon-block` says:

1. Call `next_click` with `timeout_millis: 300000`, and leave out `panel_id`.
2. A press on `stop` runs `stop --failed --reason "stopped by the person"`,
   updates the card to `Ended`, and ends your work on this session.
3. A press on `again` runs a new `start` with the same app and device after the
   old session stopped, and sets the step count back to 1.
4. On `timed_out`, call `next_click` again while the session runs.

A session that you leave without a `stop` still bills its minutes, and the
provider ends it 90 seconds after the last command. Update the card when a
session ends by itself.

## Facts of a real session

These facts come from real runs on a Google Pixel 8 with Android 14 and on an
iPhone 15 with iOS 17.

- **A session that gets no command for 90 seconds ends.** One command takes 0.3
  to 2 seconds and `start` takes about 20 seconds. Keep the time between two
  commands below 90 seconds, and read `status` after a long pause.
- **`back` can leave the app.** On Android with no keyboard and no screen to go
  back to, the back key closes the app. Run `open` to bring the app back to the
  front, and look at the screen again.
- **A swipe across the keyboard types letters.** The keyboard uses glide typing,
  so a finger that moves over the keys writes a word. `swipe` closes the
  keyboard first and says in its answer whether a keyboard was there.
- **A label of a field is a separate text on Android Compose.** The label near a
  field is its own text row, and it is not the field. `type` with no target uses
  the first text field of the screen, and `--field N` uses another one, counted
  from 1 in reading order.
- **`wait` costs no more than its bound.** It polls the screen every half second
  and answers as soon as a row holds the text. Its default bound is 20 seconds
  and its largest bound is 60.
- **`status` and `start` answer a `watch` link.** The link is the page of the
  provider with a live view of the phone. Put it on the card as a `link`.
- **`stop` answers a `recording` link.** The link holds the video and the logs of
  the session.

## Cost and clean-up

> **CAUTION: EACH MINUTE OF A SESSION USES THE DEVICE MINUTES OF THE ACCOUNT.**
> Always run `stop`. A session that you leave open bills its minutes until the
> provider ends it, which is 90 seconds after the last command at the earliest.

- Run `stop` in the same run as every `start`, also when the test fails.
- Hold at most 2 sessions at a time, and hold them for the shortest time that
  proves the flow.
- Upload an app only when the person named that file. An upload is free, and a
  wrong app costs the minutes of the session that runs it.
- Prove that no session runs before you end your work. Run `status` before the
  last `stop`, and read the `state` of the session.
- A tool that ends with your agent panel ends no session by itself. The provider
  ends a session 90 seconds after its last command. Say this to the person when
  you cannot run `stop`.

## Evidence

- Keep the screenshots in the folder where you run the tool, with a name that
  holds the step: `step-1-start.png`, `step-4-typed.png`.
- Keep the `recording` link of `stop`, and put it in your final answer with the
  verdict and the seconds of the session.
- Put the state, the step count and the result of each step in the final answer.
  A screenshot or a link alone proves no flow.
- Keep a private app, a private account name and a token out of the card, of a
  file that you commit and of a public issue.

## What this skill does not do

- **It shows no live picture inside a card.** The live view of the phone is the
  page of the `watch` link, in a browser panel of the person.
- **It sends no gesture with two fingers.** It sends a tap, a swipe with one
  finger and a text. A pinch, a two-finger scroll and a long press are not part
  of this version.
- **It reads no system dialog outside the app.** `screen` reads the app of the
  session. A dialog of the system, such as a permission question of Android, is
  a text of the app only when the app itself draws it.
- **It runs no iOS app that is not signed.** The provider installs a signed
  `.ipa`; an unsigned build fails at `start`.
- **It starts no local tunnel.** An app that needs a backend on this computer
  needs the local tunnel of the provider, which this skill does not start.
- **It changes no app.** It does not build, sign, install on a local device,
  edit a release or upload a build for a store.

## References

| Reference | Contents |
|---|---|
| [android](references/android.md) | The rows of `screen` on Android, the selectors of `tap` and `type`, the fields of a Compose screen, and the keyboard |
| [ios](references/ios.md) | The rows of `screen` on iOS, the selectors of `tap` and `type`, and a signed build |
| [provider](references/provider.md) | The REST and WebDriver calls of the tool, the device list command and the shape of a second provider |
