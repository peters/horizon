---
procedure: mobile-app-session
feature: horizon-mobile skill: one app on one real phone of a device cloud
platforms: [linux]
cost: paid device
destructive: no
secrets: `BROWSERSTACK_USERNAME`, `BROWSERSTACK_ACCESS_KEY`
owner: peters
---

# horizon-mobile skill test procedure

## 1. Purpose

This procedure tests the bundled `horizon-mobile` skill and its tool
`scripts/mobile_session.py`. It proves that an agent starts a session on a real
Android phone and on a real iOS phone, reads the screen, taps, types and reads
the text back, shows a card with the watch link, stops the session from the card,
and leaves no session that runs.

## 2. Applicability

- Candidate: a current `horizon` build that installs the bundled skills.
- Platforms: Linux, with an agent panel that has a shell and the `block_panel`
  tool.
- The provider account of the tasks is a paid App Automate account.
- This procedure does not test a local backend, the local tunnel of the
  provider, a gesture with two fingers, or a build of an app. It does not test
  the installation of a skill in a new home.

## 3. Safety

> **CAUTION: EVERY SESSION USES THE DEVICE MINUTES OF THE ACCOUNT.** One minute
> of one session uses one device minute. Do the stop task of each session in the
> same run, and hold at most 2 sessions at a time.

> **CAUTION: EACH UPLOAD SENDS AN APP FILE TO THE PROVIDER.** Upload only the app
> file that the person named for this run, and never a build that another person
> owns.

> **CAUTION: THE CREDENTIAL OF THE ACCOUNT IS A NAMED SECRET.** Do not write the
> user name or the access key in a card, a shell command, a log or this report.
> If a value reaches a file, rotate the key.

## 4. Equipment and preconditions

- A frozen candidate `horizon` binary and its SHA-256.
- An isolated test desktop, viewed through a Horizon native VNC Device panel, as
  [the device smoke fixture](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  says.
- One agent panel in that workspace, with a shell and the `block_panel` tool.
- The two named secrets in **Settings → Secrets**, under the names
  `BROWSERSTACK_USERNAME` and `BROWSERSTACK_ACCESS_KEY`. The agent panel starts
  after the person saves them.
- One debug `.apk` of a test app for Android, and the public sample app of the
  provider for iOS. Both files are on the test computer, and the person named
  them.
- A browser panel of the workspace, signed in to the provider. The watch link
  opens the page of the session there.

## 5. Setup

1. Start the candidate in the isolated desktop through the fixture with the
   bundled skills enabled.

   Result: The workspace shows the agent panel, and the agent reports that the
   `horizon-mobile` skill is there.

2. Ask the agent: `Which real phones can the mobile skill use today?`

   Result: The agent answers with a device list, and it names a device and its
   `os_version` for each platform. The list holds no emulator.

3. Ask the agent to read the tool in its skill folder, and to report the two
   base addresses and the four commands of the loop.

   Result: The agent names `HORIZON_MOBILE_HUB`, `HORIZON_MOBILE_API`, `start`,
   `screen`, `stop` and `wait`, and it reports that the tool prints one JSON
   object for each call.

## 6. Tasks

### 6.1 MA-1 — Android: start, type, read back and stop

> **CAUTION: THIS TASK USES DEVICE MINUTES AND UPLOADS THE APK.** Do MA-5 in the
> same run.

1. Ask the agent: `Start the Android app on a Pixel with the mobile skill, type
   agent@example.test into the first text field, and read it back.`

   Result: The agent uploads the `.apk` once, starts one session, and shows a
   card with a heading of the app and the device, a `kv` of the session and the
   step count, the watch link and two buttons, `Stop` and `Run again`.

2. Read the card, and press the watch link.

   Result: A browser panel opens the page of the session, the person is signed
   in, and the live view of the phone shows the app. The card stays as it was and
   the press sends no click.

3. Wait for the agent to type, and read the step lines of the card.

   Result: The card holds the typed text in a step line, and the step count
   grows with each step. The agent reports a `screen` row with
   `agent@example.test`.

4. Ask the agent to stop the session with a verdict and to report the recording
   link.

   Result: The card shows `Ended` or `Passed` with no running session, and the
   agent gives the `recording` link. No `.mobile-session.json` stays in the
   folder of the run.

### 6.2 MA-2 — iOS: the sample app, a tap and a typed text

> **CAUTION: THIS TASK USES DEVICE MINUTES.** A signed `.ipa` is necessary, and
> the sample app of the provider is signed.

1. Ask the agent: `Run the sample app on an iPhone with the mobile skill, tap the
   field by its accessibility id, type hello with Enter, and read the text back.`

   Result: The agent starts one iOS session, taps the field, types, and reports
   the text of the field as `hello`.

2. Ask the agent to press `back` on iOS.

   Result: The agent reports the error of the tool that the driver of iOS has no
   back key, and it taps a control of the app instead.

3. Stop the session.

   Result: The answer holds the recording link of the iOS session, and the card
   shows the end of the session.

### 6.3 MA-3 — A press on `Stop` of the card

> **CAUTION: THIS TASK USES DEVICE MINUTES.** Press `Stop` of the card in the
> same run as the start of the session.

1. Ask the agent for a session on Android and for its card, and press `Stop` on
   the card.

   Result: The agent reports the click, runs `stop`, updates the card to the end
   of the session, and reports the recording link. The session ends.

2. Read the card after the click.

   Result: The card holds one `Ended` line and the person sees no running step.

### 6.4 MA-4 — A session that gets no command ends by itself

> **CAUTION: THIS TASK USES DEVICE MINUTES FOR THE 90 SECONDS OF THE WAIT.**

1. Ask the agent to start a session, to read the screen once, and then to run no
   command for 100 seconds.

2. Run `status` after the wait.

   Result: The tool answers with the state of the session, and the state is no
   longer `running`. The agent says that the provider ended the session.

3. Remove the state file of the ended session in the folder of the run, and tell
   the person that the session ended by itself.

### 6.5 MA-5 — Clean-up: no session runs

> **CAUTION: THIS TASK READS THE ACCOUNT OF THE PERSON.** It uploads no app and
> starts no session.

1. Ask the agent to read the plan of the account and the state of every session
   of the run, as
   [the provider reference](../../../assets/plugins/claude-code/skills/horizon-mobile/references/provider.md)
   says.

   Result: The agent reports the device minutes that remain, and no session of
   the run in the state `running`.

2. Read the folder of the run.

   Result: It holds the screenshots of the steps and no
   `.mobile-session.json`.

## 7. Pass criteria

- MA-1 starts one Android session in about 20 seconds, types into the first
  text field and reads the text back.
- The card of MA-1 holds the app, the device, the state, the step count, the
  watch link and the two buttons, and it changes after each step.
- The watch link opens the page of the session in a browser panel, and the page
  shows the live view of the phone.
- MA-2 starts one iOS session and reads the typed text back.
- MA-3 ends the session from the card, and the recording link is in the answer.
- MA-4 proves that a session with no command ends by itself.
- MA-5 proves that no session of the run still runs.
- No user name and no access key appear in a card, a shell command, a log or the
  report.

## 8. Cleanup

1. Stop every session of the run that still runs.

   Result: `status` answers a state that is not `running`.

2. Close the browser panel of the watch link.

   Result: The workspace holds no page of a session that ended.

3. Remove the screenshots and the state file of the run.

   Result: The folder of the run holds no file of the session.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). Keep the app file, the
credential and the private build out of the repository.
