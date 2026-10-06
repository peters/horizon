---
procedure: device-type-multi-chunk
feature: horizon-device text input on X11, several type actions
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Device multi-chunk text input test procedure

## 1. Purpose

This procedure makes sure that a sequence of `type` actions from
`horizon-device` gives each character to the application one time. It tests
text that is too long for one action. You must send this text as several
actions. The procedure also makes sure that the other actions continue to
operate after text input uses the spare keycodes.

## 2. Applicability

- Candidate: each candidate that changes the X11 input of `horizon-device`.
- Platforms: Linux, in an isolated desktop with a live view.
- This procedure does not test these items:
  - Wayland, macOS and Windows.
  - The focus change after a click. Send a click, then wait for the focus
    before you type.
  - Keyboard layouts other than the default US layout of Xvfb.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC TEXT AND FAKE KEYS. If you type a real key,
> the recording, the screenshots and the saved key file can show it.

> **CAUTION:** SEND INPUT ONLY TO THE DISPLAY OF THIS RUN. Another fixture or
> the developer desktop can receive keys that you send to a wrong display.

> **CAUTION:** DO NOT USE THE REAL HOME DIRECTORY. The fixture writes the fake
> key to its private home. A real `~/.horizon` can contain real keys.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`.
- A frozen candidate for `horizon` and `horizon-device`, and their SHA-256.
- A private state directory, `<state>`, and a private evidence directory,
  `<evidence>`. Both have mode `0700`. On a host with a full `/tmp`, put both
  below `/var/tmp`.
- A second Xvfb display for the live tests in T01. No other run uses it.
- A Device panel that shows a live view of the fixture.
- No real credentials. Each key in this procedure is synthetic.

## 5. Setup

1. Build `horizon` and `horizon-device` from the candidate commit.

   Result: The build completes without errors.

2. Freeze the two executables in a new directory, `<bin>`.

   ```sh
   cp target/debug/horizon target/debug/horizon-device <bin>/
   sha256sum <bin>/horizon <bin>/horizon-device > <bin>/SHA256SUMS
   ```

   Result: `SHA256SUMS` has two lines. Record them in the results.

3. Start the fixture with the frozen `horizon` and a new state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <bin>/horizon \
     --native-view --state <state>
   ```

   Result: The fixture writes `lab.json` and `target.json` in `<state>`.

4. Find the Horizon child process of the fixture. Compare its executable hash
   with `SHA256SUMS`.

   ```sh
   sha256sum /proc/<horizon-pid>/exe
   ```

   Result: The hash is the same as the hash of `<bin>/horizon`.

5. Make a Device panel with the `vnc_address` from `lab.json`.

   Result: The Device panel shows the isolated desktop.

6. Inspect the Device panel three times, two seconds apart.

   Result: `connection` is `connected`. `image_displayed` is `true`. The value
   of `frame_sequence` increases.

7. Start a new Xvfb display for T01 in the background. Record its process ID.

   ```sh
   Xvfb -displayfd 3 -screen 0 1280x800x24 -nolisten tcp -noreset \
     3> <evidence>/display &
   echo $! > <evidence>/xvfb.pid
   ```

   Result: Xvfb runs. The option `-noreset` keeps the keymap when the last
   client disconnects.

8. Wait until `<evidence>/display` contains the display number. Then write a
   target file for this display.

   ```sh
   until [ -s <evidence>/display ]; do sleep 0.1; done
   printf '{"id":"live","endpoint":{"kind":"local_x11","display":":%s"}}' \
     "$(cat <evidence>/display)" > <evidence>/live-target.json
   ```

   Result: `live-target.json` names a display that no other run uses.

## 6. Tasks

Give each result the task ID. A report uses the ID to give a result.

### 6.1 T01 — Live input tests with a late receiver

1. Run the live tests on the display of step 5.7.

   ```sh
   HORIZON_DEVICE_TEST_TARGET=<evidence>/live-target.json \
   HORIZON_DEVICE_TYPE_ITERATIONS=20 \
     cargo test -p horizon-device --features cli --lib --test x11_live \
     --test cli -- --ignored --test-threads=1
   ```

   Result: All tests pass. The multi-chunk test types 20 synthetic keys of 106
   characters. Its receiver reads each key 600 ms late with the current keymap.
   The test also types text with Caps Lock on. With a held Shift key, it gets
   `unsupported` and no key.

2. Run the same command again on the same display.

   Result: All tests pass. The second run uses the temporary keycodes of the
   first run again.

3. If a test fails, record the test name and the first different character.

   Result: The record shows the index of each lost or wrong character.

### 6.2 T02 — Rejection before input

1. Send one `type` action with more distinct non-layout characters than the
   free slots. Use Unicode private use characters, for example `U+E000` and
   later characters.

   Result: The action fails with `invalid_request` and the message
   `text exceeds available X11 Unicode key mappings`.

2. Examine the focused window of the fixture.

   Result: The window shows no new character.

### 6.3 T03 — Type a key into a Horizon text field

> **CAUTION:** TYPE ONLY THE SYNTHETIC KEY FROM THIS PROCEDURE. A real key goes
> into the saved key file and into the screenshots of this run.

1. Make a synthetic key of 106 characters from `[A-Za-z0-9_-]`.

   ```sh
   python3 -c "import secrets, string; a = string.ascii_letters + string.digits + '_-'; print(''.join(secrets.choice(a) for _ in range(106)), end='')" > <evidence>/key-01.txt
   ```

   Result: `key-01.txt` contains 106 characters and no line feed.

2. Click **Cloud** in the menu bar of the candidate.

   Result: The Cloud menu opens.

3. Click **Cloud settings…**.

   Result: The dialog opens. The RunPod card shows an empty key field, or a
   saved key with **Replace**.

4. If the RunPod card shows **Replace**, click **Replace**.

   Result: An empty key field opens.

5. Click the RunPod key field.

   Result: The field has the text cursor.

6. Send the key as three `type` actions of 40, 40 and 26 characters. Do not
   wait between the actions.

   Result: Each action gives the receipt `dispatched`.

7. Click **Save settings**.

   Result: The dialog closes. `settings.json` in the private home changes.

8. Find the saved key file. Use the `runpod_key_file` value in
   `<state>/data/home/.horizon/cloud/settings.json`. Replace the real home path
   with `<state>/data/home`.

   Result: You have the path of the saved key file in the private home.

9. Compare the saved key file with `key-01.txt`.

   ```sh
   cmp <saved key file> <evidence>/key-01.txt
   ```

   Result: `cmp` shows no difference. Record the result as `match` or `loss`.

### 6.4 T04 — Repeated runs in the Horizon text field

> **CAUTION:** TYPE ONLY SYNTHETIC KEYS. Make a new synthetic key for each run.

1. Do steps 1 to 9 of T03 20 times. Make a new key file for each run.

   Result: Each run has a result line, `match` or `loss`.

2. If a run gives `loss`, compare the two files character by character.

   Result: The record shows the index of each lost character and its action.

3. If a click before the typing did not open the dialog, mark the run as
   invalid. Do not count it as `match` or `loss`.

   Result: Each invalid run has a reason in the record.

### 6.5 T05 — Text that needs temporary keycodes

1. Click the **Device input test** terminal panel of the fixture.

   Result: The terminal has the keyboard focus.

2. Type this command with one `type` action. `N` is the UTF-8 byte count of the
   text of step 4.

   ```sh
   head -c N > ~/unicode.txt
   ```

   Result: The terminal shows the command.

3. Send a `key` action with `enter`.

   Result: The command waits for input.

4. Send four `type` actions, one after the other. Use 15 distinct Latin-1
   letters, 15 Greek letters, 15 Cyrillic letters, and the first part again.

   Result: Each action gives the receipt `dispatched`. Together, the four
   actions need more temporary keycodes than the free slots have. Thus, the
   tool changes temporary keycodes between the actions.

5. Compare `<state>/data/home/unicode.txt` with the text of step 4.

   Result: The two texts are the same.

### 6.6 T06 — Other actions after text input

1. List the unused keycodes of the fixture display. Do not count keycode 8.

   ```sh
   xmodmap -display :<fixture display> -pke | awk 'NF <= 3'
   ```

   Result: After T05, one keycode, `<K>`, is unused.

2. Map `<K>` from this independent client.

   ```sh
   xmodmap -display :<fixture display> -e "keycode <K> = F13 F13"
   ```

   Result: The list of step 1 shows only keycode 8. The keymap has no unused keycode.

3. Run `doctor` for the fixture target.

   ```sh
   <bin>/horizon-device --target <state>/target.json doctor
   ```

   Result: The result has `"ok":true` and the capability `keyboard`. The list
   of step 1 shows one more unused keycode: `doctor` cleared the oldest
   temporary mapping.

4. Send a `key` action with `escape` and the modifier `meta`.

   Result: The action gives the receipt `dispatched`.

5. Clear the mapping of step 2.

   ```sh
   xmodmap -display :<fixture display> -e "keycode <K> ="
   ```

   Result: Keycode `<K>` is unused again.

## 7. Pass criteria

- In T01, all live tests pass in both runs.
- In T02, the action fails before input, and no character arrives.
- In T03 and T04, each valid run shows `match`. There are 20 valid runs or more.
- In T05, the saved text is the same as the typed text.
- In T06, `doctor` and the `key` action succeed.
- The Device panel shows a live view during T03 to T06.
- The screenshots and the records show only synthetic text.

## 8. Cleanup

1. Close the Device panel of this run.

   Result: The Device panel closes. The fixture continues.

2. Stop the fixture with Ctrl-C.

   Result: The fixture removes `target.json` and the private home.

3. Stop the Xvfb display of step 5.7.

   ```sh
   kill "$(cat <evidence>/xvfb.pid)"
   ```

   Result: Only the processes of this run stop.

4. Delete the synthetic key files in `<evidence>`.

   Result: No key file of this run remains.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
