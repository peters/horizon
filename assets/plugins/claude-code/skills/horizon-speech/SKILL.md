---
name: horizon-speech
description: Set up and use speech input in Horizon: one hotkey records with a recorder program of this computer, and a transcription service or command inserts the text into the focused terminal. Use when speech input records nothing, when the settings of the recorder or the transcriber must change, when a key must be stored as a named secret, or when a take must be tried.
---

# Horizon speech input

Horizon has no speech engine, no audio library and no model. It runs one
recorder program that you name, sends the recording to a transcription service
over HTTPS or to a transcription command, and inserts the text into the panel
that had the focus. The trust boundary is
`docs/review/trust-boundaries/speech-input.md` in the Horizon repository.

## What one take does

1. The hotkey goes down. Horizon starts the recorder as its exact child, and the
   recording goes to `<Horizon home>/speech/take-<random>.wav`.
2. The hotkey comes up. Horizon sends `SIGINT` to that exact child. When the
   child still runs after 0.3 seconds, Horizon sends `SIGTERM` to that id, and it
   kills the id when two seconds pass. `SIGTERM` is the signal that ends a
   recorder that inherited "ignore `SIGINT`" from a background shell, `nohup` or a
   launcher, and every recorder line below closes its file on it. `Escape` while
   the key is down drops the take.
3. A take shorter than 0.3 seconds, a file with no byte, a file above 25 MB and a
   take that reaches 120 seconds at the 120th second are handled as the code
   says: a short or empty take is dropped, a large file is refused, and a long
   take is transcribed.
4. The transcription runs with a bound of 30 seconds. The text is cleaned: every
   line break and tab becomes one space, every other control character goes, and
   the text is cut at 8 KiB.
5. The text goes into the panel of the take through the terminal paste path, with
   no Enter key. The person sends the line.
6. The take file is deleted after every outcome.

Press the hotkey with no terminal under the focus and Horizon says "Click a
terminal first." and records nothing. The hotkey also works in a terminal panel
and an agent panel, and not in an editor, browser or card panel.

## The recorder of this computer

The recorder line is one command line with the placeholder `{file}`. Horizon runs
the program itself and never a shell, so a quoted argument stays one argument.

| Platform | Recorder line |
|---|---|
| Linux with PipeWire (the default) | `pw-record --rate 16000 --channels 1 --format s16 {file}` |
| macOS with ffmpeg | `ffmpeg -f avfoundation -i :default -ac 1 -ar 16000 -y {file}` |
| macOS with sox | `rec -c 1 -r 16000 {file}` |

`pw-record`, `arecord` (`arecord -f S16_LE -c 1 -r 16000 {file}`), `ffmpeg` and
`rec` write the length of the recording into the header of the file when they
end, on `SIGINT` and on `SIGTERM`. A take whose recorder had to be killed holds a
header with the length 0 or `0xFFFFFFFF`: Horizon writes the two length fields of
that canonical header from the size of the file before it sends the take, so the
recording still reaches the service.

macOS has no recorder command of the system, so the line is empty there until a
person installs one and writes it in **Settings** › **Speech**. A program that no
folder of `PATH` holds is one sentence in the mark, and it names the program.

## The transcriber

Choose one of two:

- **Service** — `endpoint` is `https`, or `http` only on a loopback address.
  `model` defaults to `gpt-transcribe`, `language` is an optional hint such as
  `no`, and `key_secret` is the name of a named secret that holds the key. The
  default name is `SPEECH_API_KEY`: the names that start with `OPENAI` belong to
  the tools of Horizon, so a named secret never takes one.
- **Command** — one command line with `{file}`. Its standard output is the text.
  It gets the environment of Horizon and no named secret.

### Store the key as a named secret

The key is a named secret, so its value lives in `secrets.sqlite` of the Horizon
home under `named/SPEECH_API_KEY`, and it never appears in the settings, a log
line or an error text. Store it once:

1. Open **Settings** › **Secrets**.
2. Type the name `SPEECH_API_KEY` and the value, and press the button that stores
   it.
3. The **Speech** tab then says `SPEECH_API_KEY: stored on this computer`.

The value reaches one `Authorization` header of one request. A take goes to the
host of the endpoint, so read that host before you keep it, and use a key that
you can rotate.

## Try one take

1. Open **Settings** › **Speech**.
2. Set the hotkey, the recorder line and the transcriber.
3. Press **Record two seconds**. The tab shows the text of that take, or the
   sentence of its error.

The state mark of a take is a small pill at the bottom of the panel: the seconds
while it records, then one line of a transcription, then six seconds of a
sentence of a failure. The sentences are one of: the recorder is not installed;
the recorder ended early, with its last line of standard error; no secret of that
name is on this computer; the service did not answer in 30 seconds; the service
answered a status, with its message cut at 200 characters.

## What leaves the computer

- The recording of every take goes to the host of the endpoint, as one
  `multipart/form-data` request. A service keeps what it keeps; Horizon cannot
  delete it.
- The key goes in that request and nowhere else.
- A transcriber command stays on this computer.
- Nothing goes anywhere else: no telemetry, and no retry of a failed take.
