---
applyTo: "crates/horizon-frame-source/src/**,crates/horizon-core/src/config/frame_sources.rs,crates/horizon-ui/src/device_widget/session.rs,crates/horizon-ui/src/device_widget/frame_input.rs,crates/horizon-ui/src/device_widget/frame_wheel.rs,crates/horizon-ui/src/app/settings/frame_sources.rs,scripts/device-smoke/frame_source_standin.py"
---

# Trust boundary of a frame source viewer

A native Device panel shows the pictures of a **frame source program**. Horizon
starts that program as its child and knows nothing about it: no codec, no
library, no name of a program. The contract of the bytes is in
[`docs/architecture/frame-source.md`](../../architecture/frame-source.md), and
the module comment of
[`crates/horizon-frame-source/src/lib.rs`](../../../crates/horizon-frame-source/src/lib.rs)
states the same in code.

Every picture and every control message comes from a program that a person named
in the settings. This page states what is untrusted, what Horizon guarantees and
what it does not guarantee.

## Untrusted: every byte of the program

- **The header is read under bounds.** The 32 bytes of a header hold the eight
  bytes `NV12FRM1`, the sequence number, the time of the capture, the width and
  the height. A header of another magic, a width or a height that is odd, below
  16 or above 8192, or a length that the picture cannot hold is one typed error:
  the viewer ends and the child stops. No plane is read before its header
  passed.
- **A sequence number that jumps is normal.** A missing number is a picture that
  the program dropped. Horizon counts it and shows it; it is not an error and it
  does not end the viewer.
- **The time of the capture is a number, not a clock.** Horizon shows the age of
  the newest picture from it and never sets a clock from it.
- **The control message is bounded.** A message is 4 bytes of length and then one
  JSON object of at most 65536 bytes, with a member `type`. A length above that
  bound, a body that is not JSON, and a member or a type that Horizon does not
  know are refused as a message: an unknown member is ignored, and a shape that
  cannot be read ends the viewer with a typed error.
- **Every text of the program is bounded and cleaned before a UI shows it.** The
  name of the source, the reason of `state`, the `message` of `error` and the last
  line of the standard error are cut at 200 characters and drawn with every
  control character removed. They are text of a program, not text of Horizon.
- **The program's standard error is the program's.** Only its last line is kept,
  and only to say why the program ended.
- **Input messages are answers of the program.** A program that answers `error`
  with `bad-value`, `outside`, `view-only` or `input-failed` never ends the
  viewer. Its text is shown in the details of the panel, cut and cleaned.

What the bounds do not guarantee:

- **A program shows what it shows.** Horizon draws the pixels of the program in a
  panel of the person. A program that draws a credential, a page of a person or a
  picture of the desktop of the person shows exactly that. The panel is a viewer,
  and the content of the picture is the content of the program.
- **The pixels are not a claim about the system.** Horizon uploads the pixels
  that arrived. It does not compare them with the desktop, the screen or any
  other source.

## Untrusted: the name and the target from an agent

- **An agent names a source of the settings, and never a command.** `create` with
  `frame_source` holds `name` and an optional `target`. Horizon looks the name up
  in the section `frame_sources` of the settings of this computer. A name that
  this Horizon does not hold is `unknown_frame_source`, and the answer lists the
  names and never the commands.
- **The target is one argument of the person's line.** `target` is 1 to 255
  characters of letters, digits, `.`, `_`, `:`, `/` and `-`, and it cannot start
  with `-`. It replaces `{target}`, which the person's line holds at most once as
  a whole argument. An agent cannot add an argument, a placeholder or a second
  occurrence of `{target}`.
- **No operation of an agent runs a program that a person did not name.** Horizon
  starts only the program of a line of the settings, and it starts it when a
  viewer of that source opens.

## Untrusted: the line of the settings

- **The line is the person's, and it is split, never given to a shell.** The
  split rules are the ones of
  [`crates/horizon-core/src/speech/command.rs`](../../../crates/horizon-core/src/speech/command.rs):
  white space divides two arguments, quotes group one argument, and no shell
  reads the line. A `;`, a `|`, a `$(...)` or a newline is a character of one
  argument.
- **The program is the exact program.** Horizon starts one program with the
  arguments of the line, with no shell between them.
- **The exact child is stopped.** Horizon holds the process id of the program.
  `close` sends `bye`, waits up to two seconds and then sends `SIGTERM` to that
  id alone. A program that still runs after two more seconds gets `SIGKILL` to
  that id alone. No signal goes to a process group and no name is matched, so no
  other program of this computer can be touched.
- **The program does not outlive the window.** Horizon keeps the write end of the
  standard input of the program and the read end of its picture socket. Both are
  closed by the kernel when the process that holds them ends, whatever ends it,
  and the program ends with the socket. A test starts the stand-in, ends the
  parent that holds the handles and checks that the stand-in exits.

What the guarantee does not cover:

- **The program runs with the rights of the person.** A frame source program
  reads the files, the devices and the network that the person reads. Horizon
  starts it, bounds its bytes and stops it; it does not sandbox it.
- **A program that starts its own child is not ended by the signal.** A shell
  wrapper that starts another program and waits for it passes no term to it. Give
  the line the program itself.
- **A program for macOS is the person's to install.** Horizon starts the line of
  the settings on Linux and on macOS, and it holds no program of its own.

## What Horizon guarantees about one viewer

- **One worker thread for each viewer.** The reader of the socket, the reader of
  the control messages, the writer of `view` and the convert of one picture run
  on threads of the viewer. The UI thread takes the newest picture and uploads
  it; every function that waits starts with `ui_thread::blocking`.
- **A slot of one picture.** The reader keeps only the newest picture, so a slow
  frame never queues pictures and never shows an older picture after a newer one.
- **A picture is converted once and only when a frame shows it.** The convert of
  NV12 to the pixels of the widget runs on the worker.
- **The size of a picture is read from its own header.** After a size change one
  or two pictures of the old size can still come, and each of them is read with
  its own header.
- **The program asks for the size that the panel draws at.** The widget sends
  `view` with the size in physical pixels after that size has been stable for 150
  ms and only when it changed, and `visible` false while the viewer is hidden, off
  canvas or behind a fullscreen panel.
- **Input reaches the program only after the person turned Interact on.** No
  operation of an agent can turn it on, and a source whose `hello.input` is false
  or absent is view only.
- **A pointer event of the picture is the pointer event that the program gets.**
  A press is one only inside the drawn picture and only under the layer of this
  panel, so the press on a control of Horizon and a press on a popup above the
  picture reach no program. A release follows only a press that was sent, a
  release beside the picture is at the last point that the program saw, and no
  point is invented or moved to an edge.
- **A key and a text come from the picture that has the keyboard.** A press
  inside the picture takes the keyboard, and a press outside it, a turn of
  Interact off, a covered or hidden viewer and a window that lost the focus
  release every key that the program holds. A release without a press that was
  sent is no message, so a chord that Horizon handled as its own shortcut never
  reaches the program.
- **The wheel of the contract is steps and not pixels.** Horizon adds the travel
  of the wheel up, sends the whole notches of the picture, keeps the rest for the
  next frame, and holds at most ten steps in one event.
- **A new picture asks for one repaint.** There is no repaint loop, and the upload
  to the GPU happens at most once for each frame of the UI and at most 60 times
  each second.

## What it does not guarantee

- **A busy source is refused, not shared.** One program serves one viewer. A
  second viewer of the same source answers `busy`, and Horizon shows it.
- **A program that answers slowly is a slow picture.** Horizon drops no picture
  that arrived and it holds one. The rate of the pictures is the rate of the
  program.
- **Horizon does not judge a picture.** A picture of the wrong size, a still
  picture and a black picture are pictures. Only the header is checked.
