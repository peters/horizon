---
procedure: speech-input
feature: Speech input with a recorder program and a transcription service or command
platforms: [linux, macos]
cost: none
destructive: no
secrets: [SPEECH_API_KEY of the test only]
owner: peters
---

# Speech input test procedure

## 1. Purpose

This procedure proves that one take of speech input records with a recorder
program of this computer, that a transcription service or a transcription
command turns the take into text, that the text reaches the panel that had the
focus with no line break, that the state mark tells the state of the take, that
the take file goes after every outcome, and that the key of the service reaches
one request header and no file of the run.

## 2. Applicability

- Candidate: each candidate that changes speech input, the Settings tab
  **Speech**, the speech section of the settings or the speech boundary.
- Platforms: Linux with the local device smoke fixture. The macOS lane uses an
  isolated desktop session, a recorder program that the person installs, and the
  same stand-ins.
- This procedure does not test: a real microphone, a real service, a model of a
  speech engine, or the recognition quality of a vendor.

## 3. Safety

> **CAUTION:** USE A MARKER VALUE AND A NEW STATE DIRECTORY. Never type a real
> key of a provider into this procedure. The value of the fixture is in clear
> text in its private state, and that state must be a new directory that this
> run owns.

> **CAUTION:** THE TASK SI-8 SENDS A RECORDING TO A REAL SERVICE AND COSTS MONEY
> OF THE OWNER. Run it only with the explicit permission of the owner, with a key
> of the smallest scope, and with a recording that holds no private content.

> **CAUTION:** DO NOT START A RECORDER OF THIS COMPUTER OUTSIDE THE FIXTURE. A
> recorder opens the microphone, and the stand-ins of this procedure record one
> prepared file and open no audio device.

> **CAUTION:** CLOSE ONLY THE DEVICE PANEL AND THE FIXTURE OF THIS RUN. Do not
> stop another Horizon instance, another fixture, or the Horizon of the person.

## 4. Equipment and preconditions

- A Linux computer with Xvfb, Openbox, x11vnc, `bwrap`, `dbus-daemon` and
  Python 3, or a macOS test machine with an isolated desktop session.
- A frozen candidate binary, `<frozen>/horizon`, and its SHA-256 record.
- A private evidence directory, `<evidence>`, outside the checkout.
- A prepared recording, `<evidence>/take.wav`. Any WAV file of at least one
  second works, because the stand-in recorder copies it. A silent file of 16 kHz
  and one channel is enough:

  ```sh
  python3 - <<'PY'
  import struct, wave
  with wave.open('<evidence>/take.wav', 'wb') as out:
      out.setnchannels(1)
      out.setsampwidth(2)
      out.setframerate(16000)
      out.writeframes(b'\x00\x00' * 16000)
  PY
  ```

- A marker value, `<marker>`, such as `stand-in-key-7f3a`.
- The transcript of one take, `<text>`, such as `hei paa deg`.

## 5. Setup

1. Start the stand-in transcription service, and keep the file that holds what it
   received:

   ```sh
   python3 -B scripts/device-smoke/speech_standin_service.py \
     --record <evidence>/received.json --text '<text>'
   ```

   Result: It prints one address, such as `127.0.0.1:40913`. This is
   `<stand-in>` in the steps that follow.

2. Start the fixture with the stand-in recorder, the stand-in service, and the
   named secret of the key:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon --native-view \
     --state <evidence>/fixture \
     --speech-recorder "/usr/bin/python3 -B scripts/device-smoke/speech_standin_recorder.py {file} --source <evidence>/take.wav" \
     --speech-service "http://<stand-in>/v1/audio/transcriptions" \
     --named-secret SPEECH_API_KEY=<marker>
   ```

   Result: The manifest prints `vnc_address`. The settings store of the fixture
   holds the section `speech` with the recorder line and the endpoint, and the
   secrets file of its private home holds the row `named/SPEECH_API_KEY`.

3. Open the Device panel of the calling workspace on `vnc_address`, and check
   that `frame_sequence` advances.

   Result: The isolated desktop is visible and live.

4. Click in the terminal panel `Live render heartbeat`.

   Result: The panel has the focus, and its titlebar is bright.

## 6. Tasks

### 6.1 SI-1 — One take inserts its text with no line break

1. Press and hold `F9` for two seconds, then release it.

   Result: A pill at the bottom of the panel says `Listening 0:01` and counts the
   seconds. After the release it says `Writing…`, and then it goes.

2. Read the last line of the panel.

   Result: The panel holds the text `<text>` at the prompt of the shell, and the
   shell has not run it: the line waits for Enter.

3. Read the record of the stand-in service:

   ```sh
   python3 -c 'import json,sys; print(json.dumps(json.load(open("<evidence>/received.json")), indent=2))'
   ```

   Result: `fields` is `["model", "response_format", "file"]`, `authorization` is
   `redacted`, and `path` is `/v1/audio/transcriptions`.

4. Search the private home of the fixture for the marker value:

   ```sh
   grep -rl '<marker>' <evidence>/fixture/data/home
   ```

   Result: Only `secrets.sqlite` holds it. No log file, no settings file and no
   file of a take holds it.

5. Press Enter in the panel.

   Result: The shell runs the text `<text>`.

### 6.2 SI-2 — Escape drops the take

1. Press and hold `F9`, and press `Escape` while the pill counts.

   Result: The pill goes at once. The panel gains no text.

2. List the folder of the takes:

   ```sh
   ls -A <evidence>/fixture/data/home/.horizon/speech
   ```

   Result: The folder is empty. The stand-in service received no second request:
   its record still describes the request of SI-1.

### 6.3 SI-3 — A hotkey with no terminal under the focus records nothing

1. Press `F9` with the pointer over the empty canvas, and click the canvas first
   so that no panel has the focus.

   Result: A pill says `Click a terminal first.` for six seconds. The folder of
   the takes stays empty, and the stand-in service receives no request.

### 6.4 SI-4 — A recorder that is not installed is one sentence

1. Open **Settings** › **Speech**, and change the recorder line to
   `hz-no-such-recorder {file}`. Press **Save**, and close the Settings window.

   Result: The status line shows `Saved`.

2. Press and hold `F9` for one second.

   Result: A pill says `hz-no-such-recorder is not installed on this computer`,
   and it names the program.

3. Restore the recorder line of the setup, and press **Save**.

   Result: The status line shows `Saved`.

### 6.5 SI-5 — The state of the key of the service

1. Open **Settings** › **Speech**, select `Service`, and read the line under the
   field **Key secret**.

   Result: The line says `SPEECH_API_KEY: stored on this computer.` It holds no
   value of the key.

2. Press **Record two seconds**.

   Result: The line under the button says `The take held:` and the text `<text>`.

### 6.6 SI-6 — A refused key is one sentence with its status

1. Stop the stand-in service of the setup, and start it again with a refusal:

   ```sh
   python3 -B scripts/device-smoke/speech_standin_service.py --status 401 \
     --error 'Incorrect API key provided' --record <evidence>/refused.json
   ```

   Result: It prints a new `<stand-in>` address. Name that address in the
   endpoint of **Settings** › **Speech**, and press **Save**.

2. Press and hold `F9` for two seconds, and release it.

   Result: A pill says `the transcription service answered 401: Incorrect API
   key provided`. It holds no value of the key.

3. Read the record:

   ```sh
   python3 -c 'import json; print(json.load(open("<evidence>/refused.json"))["authorization"])'
   ```

   Result: `redacted`.

### 6.7 SI-7 — A transcriber command prints the text

1. Start a stand-in command that prints a text, in `<evidence>/transcriber.sh`:

   ```sh
   printf '#!/bin/sh\nprintf "%%s" "text from a command"\n' > <evidence>/transcriber.sh
   chmod 700 <evidence>/transcriber.sh
   ```

2. Open **Settings** › **Speech**, select `Command`, type
   `<evidence>/transcriber.sh {file}`, and press **Save**.

   Result: The status line shows `Saved`.

3. Press and hold `F9` for two seconds, and release it.

   Result: The panel gains `text from a command`, and the tab **Speech** receives
   no request of a service.

### 6.8 SI-8 — A real service and a real key

> **CAUTION:** THIS TASK SENDS THE RECORDING OF EACH TAKE TO THE SERVICE OF THE
> ENDPOINT AND COSTS MONEY OF THE OWNER. Use the permission of the owner, a key
> of the smallest scope, and a silent prepared recording. Delete the key at the
> end of the task.

1. Store the real key in a named secret of that name in **Settings** ›
   **Secrets**, and set the real endpoint and model in **Settings** › **Speech**.

2. Press and hold `F9`, speak one short sentence, and release it.

   Result: The pill counts the seconds, then says `Writing…`, then the panel gains
   the text of the sentence with no line break.

3. Delete the named secret of the key.

   Result: The row goes, and a later take says that no secret of that name is on
   this computer.

### 6.9 SI-9 — The macOS lane

1. On the macOS test machine, start the fixture with the stand-in recorder and
   the stand-in service, as in the setup.

   Result: The board opens, and the panel has the focus.

2. Open **Settings** › **Speech**.

   Result: The recorder line is empty, because macOS has no recorder of its own,
   and the tab names the two recipes of the skill `horizon-speech`.

3. Install one recipe, such as `ffmpeg -f avfoundation -i :default -ac 1 -ar
   16000 -y {file}`, type it in the recorder line, and press **Save**.

4. Press and hold `F9`, and speak one short sentence.

   Result: The pill counts the seconds, then the panel gains the text of the
   sentence with no line break. This step needs a microphone on the test machine.

## 7. Pass criteria

- One take inserts the text of its transcription with no line break.
- Escape, a hotkey with no terminal, a recorder that is not installed and a
  refused key each leave one sentence and one state.
- No file of a take stays after a success, an error or a cancel.
- The marker value is in the secrets file and in no log file, no settings file
  and no file of a take.

## 8. Cleanup

1. Close the Device panel of this run.

   Result: The Device panel goes, and the fixture keeps running.

2. Stop the fixture and the stand-in service of this run.

   Result: No process of this run stays.

3. Delete `<evidence>`.

   Result: The recordings, the records and the private home of this run are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
