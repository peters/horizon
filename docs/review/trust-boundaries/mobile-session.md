---
applyTo: "assets/plugins/claude-code/skills/horizon-mobile/**,assets/plugins/codex/skills/horizon-mobile/**"
---

# Trust boundary of a mobile session

An agent runs `mobile_session.py` of the `horizon-mobile` skill in its own shell.
The tool drives one app on one real phone of a device cloud, reads the screen of
that phone, and puts a few rows of it on a card of the person. The skill page is
`assets/plugins/claude-code/skills/horizon-mobile/SKILL.md` and the calls are in
`assets/plugins/claude-code/skills/horizon-mobile/references/provider.md`.

## Untrusted: the screen of the phone

The provider runs the phone and the app under test, so the text of a screen is
untrusted input.

- **A screen is data and never a command.** `screen` and `wait` read
  `GET /session/<id>/source` as XML with `xml.etree.ElementTree`, which parses no
  external entity and no document type. They read attributes only: no text of a
  screen becomes a path, a command, a selector by itself or a value of the
  settings.
- **A row is bounded.** One row keeps its kind, at most 80 characters of its text
  and at most 80 characters of its id. One answer holds at most 60 rows unless
  the agent asks for `--all`, and it names the rows that it leaves out.
- **The text of a row reaches the agent and a card, and no other place.** The tool
  prints it in one JSON object on its standard output. A card that the agent draws
  from it is bounded by the trust boundary of
  [`block-panel.md`](block-panel.md): plain text, at most 512 characters in one
  text, no markup and no command.
- **No text of a screen is executed.** `tap --text` and `type --into-text` build
  one selector of the driver from the text with `json.dumps`, so a text with a
  quote, a brace or a separator is one value of a selector and never a second
  statement of the selector language.

## Untrusted: the app file and the arguments of the tool

- **The app file is chosen by the person.** `upload` reads the file that the agent
  names and sends it to the provider as one `multipart/form-data` part. The tool
  reads no other file, writes no temporary copy of the app, and it uploads only
  the path that it was given.
- **The state file is the session of one folder.** `./.mobile-session.json` holds
  the session id, the platform, the device name, the OS version and the app id,
  and the tool makes it with mode `0600` before it writes. A `start` with a state
  file in the folder is refused, so one folder runs one session. `stop` deletes
  the file. The file holds no credential.
- **An argument is one argument.** The tool parses its arguments with
  `argparse` and runs no shell, so a text is never a command. It starts no other
  program.

## The credential of the account

The user name and the access key are two named secrets, so the boundary of
[`named-secrets.md`](named-secrets.md) covers their way to the environment of the
agent panel.

- **The tool reads the two values from the environment** as
  `BROWSERSTACK_USERNAME` and `BROWSERSTACK_ACCESS_KEY`, and it sends them as HTTP
  Basic authentication to the two base addresses only. Each value passes the name
  rule of `SecretName::parse`, and a test of
  `crates/horizon-core/src/named_secrets/tests/rules.rs` proves it.
- **Both values reach no output.** `scrub` replaces each value with `<hidden>` in
  every text that the tool prints, including the body of an HTTP error and the
  reason of a failed request. An error names the path of the call and never the
  host, and it never holds the value.
- **The tool writes neither value to a file.** The state file, a screenshot and
  the standard output are the only places that the tool writes, and no value
  reaches a screenshot by the tool itself.
- **A card holds neither value.** The skill says so, and the boundary of
  `block-panel.md` draws only the text that the agent composes.

## Untrusted: the answers of the provider

- **An answer is read as JSON and never trusted.** A status code other than 2xx is
  one typed error with the code and at most 500 characters of the body. A body
  that is not JSON is one error of the parser, which the agent reads as
  `{"error": ...}`.
- **The two links of an answer are text.** `watch` and `recording` come from the
  page of the session in the answer. The tool prints them, and a card shows
  `watch` as a `link`. The person opens such a page in a browser panel, where the
  boundary of [`remote-browser-tunnel.md`](remote-browser-tunnel.md) and the
  browser panels of Horizon apply.
- **No answer starts a program and no answer writes a file,** except the image of
  `screenshot`, which the tool writes to the path that the agent named.

## What the guarantees do not cover

- **The provider and the app see the credential and the app file.** Both leave
  this computer for the provider, and the boundary of the provider applies to
  them.
- **A real phone shows the app and its data.** A screenshot and the recording of a
  session hold whatever the app draws, which can include the data of a test
  account. Keep that evidence as private as the account.
- **The card proves no session.** The card holds what the agent read, not what the
  provider ran. `status` and `stop` are the two calls that read the provider
  itself.
- **A session that the agent does not stop bills its device minutes** until the
  provider ends it, which is 90 seconds after the last command at the earliest.
