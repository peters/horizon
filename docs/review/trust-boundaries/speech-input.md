---
applyTo: "crates/horizon-core/src/speech/**,crates/horizon-core/src/config/speech.rs,crates/horizon-ui/src/app/speech.rs,crates/horizon-ui/src/app/speech/**,crates/horizon-ui/src/app/settings/speech.rs,scripts/device-smoke/serve.py,scripts/device-smoke/speech_standin_recorder.py,scripts/device-smoke/speech_standin_service.py"
---

# Trust boundary of speech input

Speech input is a thin layer: a hotkey, one take, and the insert of the text of
that take. Horizon runs a recorder command of the person while the hotkey is
down, and it sends the recording to a transcription service or to a
transcription command. Horizon holds no speech engine, no audio library and no
model. The module comment of
[`crates/horizon-core/src/speech.rs`](../../../crates/horizon-core/src/speech.rs)
states the design of that layer.

The text of a take leaves one program and enters a panel of the person. This page
states what is untrusted on both sides of that crossing, what leaves the computer,
what Horizon guarantees, and what it does not guarantee.

## What leaves the computer

- **The recording leaves the computer when the transcriber is a service.** The
  whole recording of one take goes, as the body of one `multipart/form-data`
  request, to the endpoint that the person typed in the settings. The default
  endpoint is a service of a vendor, so a person who keeps that endpoint sends
  the recording of every take to that vendor. Say this plainly in the documents
  and in the Settings tab: a take is not private to this computer.
- **The key goes to that endpoint and nowhere else.** The value of one named
  secret travels as the `Authorization: Bearer` header of that one request. It is
  read from the secrets file at the moment of the request, on the worker thread.
  It never reaches a log line, an error text, an environment variable, an
  argument of a program or a file of the take.
- **A transcriber command stays on this computer.** Its standard input is empty,
  its standard output is the text, and it sends nothing by itself. What that
  program does is the program's own business.
- **Nothing else leaves.** Horizon sends no take to a service that the person did
  not name, no telemetry, and no second request. A take that fails is not
  retried.

## Untrusted: the text of the service or of the command

- **The text is text of an answer, not a command.** It comes from a service body
  or from the standard output of a program. Horizon cleans it before the insert:
  every line break and every tab becomes one space, every other control
  character (C0, DEL, C1, ESC) is removed, the result is trimmed and cut at 8
  KiB.
- **The clean removes every escape and every control byte.** A terminal reads an
  ESC byte, a carriage return or a `C0` code as a command. The clean removes
  them, so the inserted text is printable text and spaces.
- **The insert is a paste, not typed keys.** The text enters the panel through
  the paste path of the terminal widget, so a program that asked for bracketed
  paste gets one paste. The paste path removes an ESC byte and a `C3` byte
  itself, and it wraps the text in one bracketed-paste pair.
- **A bracketed-paste end marker inside the text is text.** A service that
  answers `\x1b[201~` holds an ESC byte, so the clean removes it and the paste
  path removes it again. The text cannot end the paste early and cannot send a
  second line to the program.
- **No line break at the end.** Horizon adds none, so the text waits at the
  prompt of the program and the person decides when to send it. The design says
  so in the procedure.
- **The text is inserted in the panel that had the focus when the hotkey went
  down.** A panel that lost the focus during the take still receives the text of
  that take, and a panel that closed during the take receives nothing.

What the clean does not guarantee:

- **A program reads the text as input.** The text reaches the command line of a
  shell, a prompt of an agent or a search field of a full-screen program, and the
  program decides what it means. Horizon inserts no Enter key, so a program
  waits for the person. A person who hotkeys a take while a program waits for a
  `y` answers that question with the text of the take.
- **The service decides its own text.** Horizon limits the length and removes
  control bytes. It does not judge the words, and it does not tell a service from
  a person.
- **A terminal that a person pastes into twice is a terminal of that person.**
  Horizon inserts the text of a take and nothing else.

## Untrusted: the two command lines of the settings

- **A command line is split, never given to a shell.** The line is split on
  white space, with single quotes and double quotes that group one argument, and
  a backslash outside single quotes that keeps the next character. Every
  argument is one `OsString` of one `Command`, so a `;`, a `|`, a `$(...)`, a
  newline or a `>` is a character of one argument and never a second command.
- **The placeholder is one argument.** `{file}` inside an argument is replaced
  by the path of the take, and nothing else is replaced. The path is
  `<Horizon home>/speech/take-<random>.wav`; its name holds no space, no
  metacharacter and no user text.
- **No shell runs, and no argument is interpolated again.** A line that is empty,
  or a line whose program is empty, is a typed refusal and no take starts.
- **The child gets the environment of Horizon.** The recorder and the command of
  the person read `PATH`, `HOME` and the variables of the session, and they get
  no named secret. A value of a named secret is never in the environment of a
  program that Horizon starts for speech input.
- **The recorder is the exact child.** Horizon starts one program, remembers its
  process id, and sends `SIGINT` to that id. When the child still runs after 0.3
  seconds Horizon sends `SIGTERM` to that id, and when two seconds pass it sends
  `SIGKILL` to that id. No signal goes to a process group and no name is matched,
  so no other program of this computer can be touched.
- **The stop does not depend on one signal.** A program that a script, `nohup` or
  a launcher started in the background inherits `SIGINT` set to "ignore", keeps
  that across `exec`, and a non-interactive shell cannot trap it. `SIGTERM` is not
  ignored in that situation, so the second signal in the step reaches a recorder
  that the first one never reached. Every recorder line of the documents
  (`pw-record`, `arecord`, `ffmpeg`, `rec`) closes the header of its file on
  `SIGTERM`, so the take keeps a length and the stop needs no kill.

What the guarantees do not cover:

- **A command of the person runs with the rights of the person.** A recorder or a
  transcriber that the person writes reads the files that the person reads,
  opens the microphone, and sends what it sends. Horizon starts it and bounds it;
  it does not sandbox it.
- **A recorder that starts its own child is not ended by the signal.** A shell
  wrapper that starts another program and waits for it passes `SIGKILL` to
  neither. Give the recorder line the program itself, as the recipes of the
  documents do.
- **A program that writes to the file after the signal writes what it writes.**
  Horizon deletes the file of the take after every outcome, so a late writer
  writes to a path that is gone.

## Untrusted: the endpoint

- **The endpoint must be `https`, or `http` on this computer.** An endpoint is
  one absolute URL. `http` is accepted only when the host is a loopback address
  (`127.0.0.0/8`, `::1`, or `localhost`), so a test and a stand-in service need
  no certificate and a real service needs no plain-text link.
- **The endpoint decides where the recording goes.** Any `https` endpoint that a
  person types receives the recording and the key. That is the purpose of the
  field, and it is also the risk: read the host of an endpoint before you keep
  it.
- **A redirect is refused.** The request does not follow a redirect, so an
  endpoint cannot move a take to another host. The answer of a redirect is one
  error line with its status.
- **The answer is bounded.** Horizon reads at most 1 MiB of the answer, so a
  service cannot fill the memory of this process. The whole call is bounded by
  30 seconds.
- **The answer is JSON with `text`.** An answer of another shape is one error
  line. The status is one line, and the message of the service is cut at 200
  characters and never holds the key.

## What Horizon guarantees about one take

- **The folder is private.** The folder `<Horizon home>/speech/` is made with
  mode `0700` and a take file with mode `0600`. Every file that is left in the
  folder at the start of a take is deleted, so a crash leaves no recording behind
  for longer than the next take.
- **One take runs at a time.** A second press of the hotkey while a take runs
  does nothing. A take stops by itself after 120 seconds, and the text is not
  transcribed then: the take is dropped.
- **A short take is dropped.** A take shorter than 0.3 seconds, or a file with no
  byte, sends no request to a service and runs no command.
- **A large file is refused.** A take above 25 MiB is one error line and no
  request.
- **A recording that its recorder did not close is still sent.** A recorder that
  the stop had to kill writes no length, so it leaves the two length fields of a
  canonical 44-byte `RIFF`/`WAVE` header of PCM audio at zero or at `0xFFFFFFFF`. A
  service reads that file as a recording of no data. Horizon writes those two
  fields from the size of the file before either transcriber reads it, and only
  for that shape: a header of another shape, such as one with a `fact` chunk or a
  stream of raw bytes, is sent as it is. Horizon repairs a length and adds no
  audio, so a take of silence stays a take of silence.
- **Every bound is one number in the code.** The stop waits 2 seconds, a take runs
  at most 120 seconds, a transcription runs at most 30 seconds, the answer is
  1 MiB and the text is 8 KiB. A test injects a shorter bound through the clock,
  so the code path is the same one that a person reaches.
- **The file of a take is deleted after success, after an error and after a
  cancel.** The delete runs before the state of the take changes, so a state that
  says that the take ended is a state where the file is gone.
- **Escape cancels the take that is running.** The recording is stopped and the
  file is deleted; no request runs.
- **The insert is untrusted text of a cleaned shape.** See the clean above.
- **The state mark is Horizon's own text.** "Listening 0:03", "Writing…" and one
  sentence of an error are drawn by Horizon. The text of a service reaches the
  panel and never the mark, the log or the settings.
- **The state mark repaints once per second while a take records.** No frame
  repeats while a take waits, so an idle window costs nothing.
- **The worker holds the take.** The recorder child, the service request, the
  command, the file reads and the file deletes run on one worker thread of the
  window. The UI thread sends the hotkey events and reads the state, and every
  function that waits starts with `ui_thread::blocking`.

## What the guarantees do not cover

- **The service keeps what it keeps.** A take that a service received is in the
  service. Its policy, its retention and its country are the policy of the
  vendor of the endpoint. Horizon cannot delete it, and it cannot tell whether
  the service logged the key. Read the terms of the service before you keep it,
  and store a key that you can rotate.
- **The key is as private as the secrets file.** A value of a named secret is in
  clear text in `<Horizon home>/secrets.sqlite`, mode `0600` in a `0700` folder.
  A process of the same user reads it, and so does a backup of the home. The
  boundary of that file is [`named-secrets.md`](named-secrets.md).
- **A program can read the network of this computer.** Horizon does not check the
  certificate of a proxy of the person, and it does not use a certificate
  authority of its own.
- **A take is not encrypted by Horizon.** The link is the encryption of rustls,
  and the endpoint and its certificate authority decide the rest.
- **A name that starts with `OPENAI` is refused as a named secret.** Horizon
  refuses the names that its own tools set, so the default name of the key is
  `SPEECH_API_KEY`.
- **The microphone is the recorder's.** Horizon opens no audio device. A recorder
  that records silence, another device or a loudspeaker answers as it answers.
- **A person who listens through a loudspeaker records the loudspeaker.** The
  recorder of the person decides the device and the echo.
