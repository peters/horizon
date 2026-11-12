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

## 2.1 The real recorder of a Linux session

The tasks of this procedure use stand-ins and open no audio device. One extra
task, SI-16, uses the real recorder `pw-record` and the sound server of the
computer. It needs the option `--host-audio`, because the sandbox binds a private
runtime folder over `/run/user/<uid>`, and the socket `pipewire-0` of the person
is not in that folder. `--host-audio` binds that one socket at the same name.

That task does not use a microphone. It records from a null sink that it makes
and destroys, so it changes no default device. See
[the fixture README](../../../scripts/device-smoke/README.md#speech-input-with-the-real-recorder-of-the-session)
for the commands of the sink and for the measured limit of that route.

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

1. Start the stand-in transcription service, and keep the file that holds what
   it received last:

   ```sh
   python3 -B scripts/device-smoke/speech_standin_service.py \
     --record <evidence>/received.json --text '<text>'
   ```

   Result: It prints one address, such as `127.0.0.1:40913`. This is
   `<stand-in>` in the steps that follow. It answers every request until step 2
   of Cleanup stops it, so every take of this run reaches it. Its record holds the
   last request that it answered. A service of one request needs the option
   `--requests N`, and the tasks of this procedure do not use it.

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

   Result: A pill says `The transcription service at <stand-in> answered 401:
   Incorrect API key provided`. It holds no value of the key.

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

   Result: The recorder line is empty, because macOS has no recorder that is a
   part of the system. The tab names two programs and the exact line of each:
   `ffmpeg -f avfoundation -i :default -ac 1 -ar 16000 -y {file}` and
   `rec -c 1 -r 16000 {file}` of SoX.

3. Install `ffmpeg`, type `ffmpeg -f avfoundation -i :default -ac 1 -ar 16000
   -y {file}` in the recorder line, and press **Save**.

4. Press and hold `F9`, and speak one short sentence.

   Result: The pill counts the seconds, then the panel gains the text of the
   sentence with no line break. This step needs a microphone on the test machine.

5. If SoX is installed, repeat steps 3 and 4 with
   `rec -c 1 -r 16000 {file}`.

   Result: The take records with SoX and the panel gains the text. A machine
   without SoX reports that `rec` is not installed, and the take records
   nothing.

### 6.10 SI-10 — The language hint of the service

1. Open **Settings** › **Speech** and select the transcriber `Service`.

   Result: The `Language` box is empty. Its hint reads `no, en`, and the line
   under the fields reads `An empty language lets the service detect the
   language of each take.`

2. Type `no` in the `Language` box and run one take. Then clear the box and run
   one more take.

   Result: Both takes answer their text. The request of the second take has no
   `language` field, so the service detects the language of that take.

### 6.11 SI-11 — The hotkey of a detached window

1. Detach the workspace of the test panel with the workspace menu.

   Result: The workspace opens in a window of its own, and the panel is in it.

2. Click the panel of the detached window, then press and hold the speech
   hotkey and speak one short sentence.

   Result: The state mark of the take is in the detached window, on the panel
   that has the focus there. The panel gains the text of the sentence with no
   line break, and the panel of the main window gains no text.

### 6.12 SI-12 — A recorder that ignores the interrupt signal is stopped

> **CAUTION:** RUN THIS TASK WITH THE STAND-IN RECORDER OF THE SETUP. Do not start
> a recorder of this computer, because a recorder opens the microphone.

1. Open **Settings** › **Speech**, and set the recorder line to a wrapper that
   ignores the interrupt signal before it starts the stand-in recorder of the
   setup:

   ```sh
   sh -c 'trap "" INT; exec "$0" "$@"' /usr/bin/python3 -B \
     scripts/device-smoke/speech_standin_recorder.py {file} --source <evidence>/take.wav
   ```

2. Press and hold `F9` for two seconds, then release it.

   Result: The pill says `Listening 0:01`, then `Writing…`, then it goes. The
   panel gains the text `<text>` in less than one second after the release: the
   stop sent `SIGTERM` after 0.3 seconds, so the recorder ended without the two
   second kill of the bound.

3. Read the record of the stand-in service as in SI-1 step 3.

   Result: The record describes the request of this take, so the take reached the
   service with the bytes of a closed recording.

### 6.13 SI-13 — The tests of a take pass as a background job

A person starts Horizon from a script with `&`, from `nohup`, or from a
launcher, and such a program inherits `SIGINT` and `SIGQUIT` set to "ignore". The
tests below prove that the take of this procedure passes in that situation too.

1. Build the test binary of `horizon-core`, and read its path:

   ```sh
   cargo test -p horizon-core --lib --no-run --message-format=json \
     | python3 -c 'import json,sys
   for line in sys.stdin:
       event = json.loads(line)
       if event.get("reason") == "compiler-artifact" and event.get("executable"):
           print(event["executable"])'
   ```

   Result: The last line is a path, such as
   `<target>/debug/deps/horizon_core-<hash>`.

2. Run the module of a take as a background job of a non-interactive shell:

   ```sh
   sh -c '"$0" speech::take > "$1" 2>&1 & wait' <binary> <evidence>/take-tests.log
   ```

   Result: The last line of the log says `test result: ok.` and names a count of
   passed tests with `0 failed`, and the command exits with the code 0. A failed
   line names the test and the events that the worker gave.

3. Read the log for a failure:

   ```sh
   grep -E "test result|^    speech::" <evidence>/take-tests.log
   ```

   Result: Every test of the module passed, including the two stand-ins that
   ignore `SIGINT` and the one that ignores `SIGINT` and `SIGTERM`.

4. List the recorders that the tests left:

   ```sh
   ps -eo pid,args | grep "[.]tmp.*/rec " || echo "no recorder stays"
   ```

   Result: `no recorder stays`. A test that failed before it stopped its recorder
   ends that recorder by its own process id, and a take whose process a test killed
   leaves no recorder either.

### 6.14 SI-14 — A recording that its recorder did not close is still sent

> **CAUTION:** RUN THIS TASK WITH THE STAND-IN RECORDER OF THE SETUP. Do not start
> a recorder of this computer, because a recorder opens the microphone.

1. Write a transcription command that answers a text only when the two length
   fields of the take name the bytes of the file:

   ```sh
   cat > <evidence>/closed.sh <<'SH'
   #!/bin/sh
   set -e
   size=$(wc -c < "$1")
   data=$(od -An -tu4 -j40 -N4 "$1" | tr -d ' ')
   if [ "$data" = "$((size - 44))" ]; then printf 'closed header'; fi
   SH
   chmod 700 <evidence>/closed.sh
   ```

2. Open **Settings** › **Speech**, and set the recorder line to the stand-in of the
   setup with `--open-header`, which leaves the two length fields at zero as a
   recorder that had to be killed does:

   ```sh
   /usr/bin/python3 -B scripts/device-smoke/speech_standin_recorder.py {file} \
     --source <evidence>/take.wav --open-header
   ```

3. Select the transcriber `Command`, type `<evidence>/closed.sh {file}`, and press
   **Save**.

4. Press and hold `F9` for two seconds, then release it.

   Result: The panel gains `closed header`. The length fields of the take named the
   bytes of the file before the command read it, so the repair ran before the
   request.

5. Read the folder of the takes as in SI-2 step 2.

   Result: The folder is empty. The file of the take is deleted after the take,
   repaired or not.

### 6.15 SI-15 — A service that cannot be reached says so at once

1. Read one free port of this computer, and keep the number:

   ```sh
   python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
   ```

   Result: It prints one number, such as `40913`. This is `<free>`. No listener
   holds it, so a connection to it is refused at once.

2. Open **Settings** › **Speech**, select `Service`, and set the endpoint to
   `http://127.0.0.1:<free>/v1/audio/transcriptions`. Press **Save**.

   Result: The status line shows `Saved`.

3. Press **Record two seconds**.

   Result: The line under the button says `Could not connect to
   127.0.0.1:<free>: the connection was refused or the host cannot be reached`.
   It names the host and no path, and it says nothing of 30 seconds.

4. Press and hold `F9` for two seconds, and release it.

   Result: The pill says the same sentence. The sentence of a take and the
   sentence of **Try** are one sentence.

5. Set the endpoint back to `http://<stand-in>/v1/audio/transcriptions`, and
   press **Save**.

   Result: The status line shows `Saved`. The next take reaches the stand-in
   service of the setup again.

### 6.16 SI-16 — A take with the real recorder of the session

This task uses the recorder of the Linux session and the sound server of the
computer. It answers the state that a live run showed, when the sandbox held no
sound socket and the take said `pw-record ended early: error:
  pw_context_connect() failed`.

> **CAUTION:** MAKE AND DESTROY ONE NULL SINK OF THE SESSION. Use the name of
> this run, and keep the id of the node for the destroy step. Do not change the
> default device of the person.

1. Make the null sink of the session, and read its id:

   ```sh
   pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=smoke-null media.class=Audio/Sink object.linger=true audio.position=[MONO] }'
   pw-cli info smoke-null
   ```

   Result: The first command prints no line. The second command prints the lines
   of the node, and the first of them is `id: <id>`. The name is `smoke-null`, so
   `<name>` is `smoke-null` and `<id>` is that number in the steps below.

2. Stop the fixture of the setup. Start it again with the real recorder, the
   stand-in service of the setup and `--host-audio`:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <frozen>/horizon --native-view \
     --state <evidence>/fixture-real --host-audio \
     --speech-recorder "pw-record --target <name> --properties stream.capture.sink=true --rate 16000 --channels 1 --format s16 {file}" \
     --speech-service "http://<stand-in>/v1/audio/transcriptions" \
     --named-secret SPEECH_API_KEY=<marker>
   ```

   Result: The manifest prints `vnc_address`, and the run starts. A run without
   the socket `pipewire-0` stops with one line that names `--host-audio` and no
   traceback.

3. Play one clip into the null sink, and press and hold `F9` at the same time:

   ```sh
   pw-play --target <name> <evidence>/take.wav
   ```

   Result: The pill counts the seconds of the take. After the release it says
   `Writing…`, and then it goes. The state mark holds no line of
   `pw_context_connect()`.

4. Read the last line of the panel, and read the record of the stand-in service
   as in SI-1 step 3.

   Result: The panel holds the text of that take, and the service received one
   request with the fields of SI-1. The words can be wrong: a clip through a null
   sink of a busy computer is damaged. This task proves the path of the recorder
   and of the request, and it does not prove the words.

5. Destroy the null sink:

   ```sh
   pw-cli destroy <id>
   ```

   Result: The node goes, and the default device of the person did not change.

## 7. Pass criteria

- One take inserts the text of its transcription with no line break.
- Escape, a hotkey with no terminal, a recorder that is not installed and a
  refused key each leave one sentence and one state.
- A service that cannot be reached is one sentence that names the host and no
  seconds, and the tab **Try** shows that same sentence.
- No file of a take stays after a success, an error or a cancel.
- A recorder that ignores the interrupt signal ends on the termination signal, and
  its take reaches the service.
- A recording that its recorder did not close reaches the transcriber with the
  length of its file in its header, and a recording of another shape reaches it
  unchanged.
- Every test of the module of a take passes as a background job of a
  non-interactive shell, and no recorder of a test stays.
- The marker value is in the secrets file and in no log file, no settings file
  and no file of a take.

## 8. Cleanup

1. Close the Device panel of this run.

   Result: The Device panel goes, and the fixture keeps running.

2. Stop the fixture and every stand-in service of this run.

   Result: No process of this run stays.

3. Delete `<evidence>`.

   Result: The recordings, the records and the private home of this run are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
