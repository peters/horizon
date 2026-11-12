# Android on a real phone

This page describes what the tool shows and how it selects an element on an
Android device. Read it with [the skill](../SKILL.md).

## The rows of `screen`

The driver of Android answers `GET /source` with a UIAutomator hierarchy. The
tool turns each element of that tree into one row:

| Field | Source of Android | Meaning |
|---|---|---|
| `kind` | the class name after the last `.` | `EditText`, `Button`, `TextView`, `ImageView` |
| `text` | the attribute `text`, or `content-desc` | the text that the person reads, or the description of the element |
| `id` | `resource-id` | the id of the element, such as `com.example.app:id/email` |
| `tap` | `clickable` | `true` when the person can press the element |

The tool leaves out an element with `displayed="false"`, an element with no text
and no id, and a repeat of an earlier row with the same kind, text and id. It
cuts one text to 80 characters and it shows at most 60 rows. `screen --all`
shows every row.

A Compose screen often has three rows for one field: a `Text` row with the label,
an `EditText` row with the current value, and a row for the error text. The
label is a text of its own and it is not part of the field.

## The selectors of `tap` and `type`

| Flag | Selector on Android | Notes |
|---|---|---|
| `--text TEXT` | `-android uiautomator` with `new UiSelector().textContains("TEXT")` | The first element whose text holds `TEXT`. It reads `text`, not `content-desc`. |
| `--id ID` | `id` | The `resource-id` of the element, with its package, such as `com.example.app:id/email`. |
| `--xpath XPATH` | `xpath` | An `xpath` of the hierarchy. It is slow and it breaks when the screen changes. |
| `--at X,Y` | no selector | One touch at a point of the screen, in device pixels. |

`type` uses the same flags with the prefix `--into-`: `--into-text`,
`--into-id` and `--into-xpath`. It presses the element first, then it enters the
text.

With no `--into-*` flag, `type` uses the text field of the screen:

| Flag | Selector | Notes |
|---|---|---|
| none, or `--field 1` | `new UiSelector().className("android.widget.EditText").instance(0)` | The first text field in reading order. |
| `--field N` | the same selector with `instance(N-1)` | The Nth text field in reading order. |

A screen with a search field, an e-mail field and a password field holds three
`EditText` elements, so `--field 2` is the e-mail field of that screen.

`--enter` adds a line break to the text, and the keyboard sends its own action for
that field, such as **Next** or **Done**.

## The keyboard

- The keyboard covers the lower part of the screen. A `swipe` across it uses
  glide typing and writes a word into the field. The tool closes the keyboard
  before every `swipe` for this reason.
- `hide-keyboard` closes the keyboard and answers `hidden`, or `not shown` when
  no keyboard was on the screen. It runs the call
  `POST /session/<id>/appium/device/hide_keyboard` and it treats the two refusals
  of that call, 404 and 500, as no keyboard.
- A `tap` on an element that the keyboard covers reaches the keyboard instead.
  Close the keyboard, or swipe the screen up, before the next tap.

## The back key and the app under test

- `back` sends `POST /session/<id>/back`, which is the system back key.
- With no keyboard and no earlier screen, the back key closes the app and the
  person sees the home screen. Run `open` to bring the app to the front again.
- `open` runs `POST /session/<id>/execute/sync` with the script
  `mobile: activateApp` and the app id `{"appId": "<package>"}`. The package is
  the `appium:appPackage` of the session capabilities, which the tool read at
  `start` and kept in the state file.

## An app file

- `upload` sends an `.apk` or an `.aab`. The debug build of the repository of the
  app is the usual file, and the repository of the app builds it.
- The provider installs the file on the device. A build that a debug signature
  signed is enough for Android; no release key is necessary.
- An app that needs a backend on this computer needs the local tunnel of the
  provider, which this skill does not start.
