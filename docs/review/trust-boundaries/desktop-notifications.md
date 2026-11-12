---
applyTo: "crates/horizon-core/src/notify.rs,crates/horizon-core/src/notify/**,crates/horizon-ui/src/app/notify.rs"
---

# Trust boundary of a desktop notification

Horizon sends one desktop notification when an agent panel starts to need the
person or finishes while no Horizon window has the focus of the desktop. The
body of that notification names the panel, and the name of a panel comes from the
terminal title of an agent or from a name that the person typed. An agent sets
its own terminal title, so that name is untrusted text that leaves Horizon
through a program of the system.

This page states what is untrusted, what Horizon guarantees and what it does not
guarantee. The module comment of
[`crates/horizon-core/src/notify.rs`](../../../crates/horizon-core/src/notify.rs)
states the same rules for the text.

## Untrusted: the name of a panel and the name of a workspace

- **An agent writes its own terminal title.** The text can hold a line break, a
  control character, a run of markup such as `<b>` or `&amp;`, any length and any
  leading `-`. It is the text of another program, not the text of Horizon.
- **The text reaches one program of this computer as an argument.** It never
  reaches a shell. Horizon starts the program with an argument list, so a
  metacharacter is a character of an argument and never a command.
- **The option list ends before the text.** A program of the system reads a word
  that starts with `-` as an option. Every command line of a notification puts
  `--` before the title and the body, so a name such as `--urgency=critical`
  arrives as text.
- **The text is cleaned and cut.** Every control character and every line break
  is removed, a run of whitespace becomes one space, and a name is cut at 60
  characters. A notification is therefore one line of printable text.
- **The markup of the notification server is escaped on Linux.** A notification
  server reads the body as markup. Horizon escapes `&`, `<` and `>` before it
  starts the program, so an agent cannot draw a link or an image in the
  notification and cannot end the markup of the body.
- **The script text of macOS holds no name.** The AppleScript that shows the
  notification is a constant. The title and the body arrive as arguments of the
  program and are read from the argument list of the script, so a name can never
  become AppleScript code.

## Horizon guarantees: one line, and no content

- **No prompt and no tool content.** A notification names the panel and the
  workspace and says that the agent needs the person or is done. The text of the
  turn, the arguments of a tool, a path and a transcript are not part of it.
- **One notification is one decision about one change.** A change that arrives
  while a Horizon window has the focus sends nothing, and a panel that the person
  marked unread by hand sends nothing. At most 20 notifications leave in 10
  minutes; after that one line says that more agents wait and the program stays
  silent until a Horizon window gets the focus again.
- **A failure is a log line, and the window keeps its state.** A program that is
  absent, that fails or that does not end within 5 seconds changes no state of
  Horizon. The program is ended and reaped, and one line at the level `INFO` says
  so. The sidebar still shows the state of every panel.
- **The program is a program of the system.** Horizon starts `notify-send` on
  Linux and `osascript` on macOS, with a cleared environment, and it never sends a
  notification to a service.

## Does not guarantee

- **The notification is visible, not read.** Whether the desktop shows the
  notification, hides it, groups it or reads it aloud is the decision of the
  notification server of the person. Horizon cannot tell whether a person saw it.
- **The text of the server is the text of the desktop.** A server that reads the
  escaped body draws the name with its characters, and a server that ignores the
  escaping draws the raw characters. Neither case sends a command.
- **Another program of the person can show a notification.** Any program of this
  computer can start `notify-send`. This boundary keeps the content of the
  notification of Horizon out of the notification, not the notification out of the
  desktop.
- **A name can be wrong or long.** An agent can set a title that says anything, so
  a notification can name a panel in a way that misleads the person. It cannot
  carry more than 60 characters, a line break or a control character.
