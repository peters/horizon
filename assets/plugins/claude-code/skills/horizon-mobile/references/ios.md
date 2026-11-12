# iOS on a real phone

This page describes what the tool shows and how it selects an element on an iOS
device. Read it with [the skill](../SKILL.md).

## The rows of `screen`

The driver of iOS answers `GET /source` with an XCTest hierarchy. The tool turns
each element of that tree into one row:

| Field | Source of iOS | Meaning |
|---|---|---|
| `kind` | the tag without `XCUIElementType` | `Button`, `StaticText`, `TextField`, `Cell` |
| `text` | the attribute `label`, or `value`, or `name` | the text that the person reads, or the value of the field |
| `id` | `name` | the accessibility identifier of the element |
| `tap` | the kind of the element | `true` for a button, a cell, a link, a field, a switch, a tab and a search field |

The tool leaves out an element with `visible="false"`, an element with no text
and no id, and a repeat of an earlier row with the same kind, text and id. It cuts
one text to 80 characters and it shows at most 60 rows. `screen --all` shows
every row.

The root row of a hierarchy is often the `Application` element, and its `label`
is the name of the app. Read it as the name of the app and not as a text of the
screen.

## The selectors of `tap` and `type`

| Flag | Selector on iOS | Notes |
|---|---|---|
| `--text TEXT` | `-ios predicate string` with `label == "TEXT" OR name == "TEXT" OR value == "TEXT"` | The comparison is exact, so give the whole text of the row. |
| `--id ID` | `accessibility id` | The accessibility identifier that the app sets. |
| `--xpath XPATH` | `xpath` | An `xpath` of the hierarchy. It is slow and it breaks when the screen changes. |
| `--at X,Y` | no selector | One touch at a point of the screen, in device pixels. |

The comparison of `--text` on iOS is an equality and not a part of the text, so a
text of a row of `screen` is the exact text that `--text` needs. The `--text`
flag of Android holds a part of the text instead; read
[the Android page](android.md) for that difference.

With no `--into-*` flag, `type` uses the text field of the screen:

| Flag | Selector | Notes |
|---|---|---|
| none, or `--field 1` | `-ios class chain` with `XCUIElementTypeTextField`, `XCUIElementTypeSecureTextField`, `XCUIElementTypeSearchField` and `XCUIElementTypeTextView` at `[1]` | The first text field in reading order. |
| `--field N` | the same chain at `[N]` | The Nth text field in reading order. |

`--enter` adds a line break to the text, and the keyboard sends its own action for
that field, such as **Go** or **Done**.

## The back key and the app under test

- The driver of iOS has no back key. `back` answers one error that says so, and
  `open` brings the app to the front again.
- Press the back control of the app with `tap` instead, such as a button named
  **Back** or the first row of a navigation bar.
- `open` runs `POST /session/<id>/execute/sync` with the script
  `mobile: activateApp` and the app id `{"appId": "<bundle id>"}`. The bundle id
  is the `appium:bundleId` of the session capabilities, which the tool read at
  `start` and kept in the state file.

## An app file

- The provider installs a **signed** `.ipa`. A build for a simulator is not an
  `.ipa` of a device, and the driver refuses it at `start`.
- A signed build of the repository of the app is the file. The build needs the
  signing of the person, which this skill does not do.
- An app that needs a backend on this computer needs the local tunnel of the
  provider, which this skill does not start.

## The keyboard

- `hide-keyboard` closes the keyboard and answers `hidden`, or `not shown` when no
  keyboard was on the screen. It runs
  `POST /session/<id>/appium/device/hide_keyboard`.
- The tool closes the keyboard before every `swipe`, as it does on Android.
