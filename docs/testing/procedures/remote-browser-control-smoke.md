---
procedure: remote-browser-control-smoke
feature: one end-to-end run of the remote browser lane
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Remote browser control smoke test procedure

## 1. Purpose

This procedure proves that an agent on one computer controls a browser on another
machine through the public MCP contract. One ordered run covers the whole lane:
create, read, fill, click, evaluate, attach a file, screenshot, record, capture
network, the refusals, and close.

The run is a script, so it is repeatable and it writes its own evidence:
`report.json` and one transcript of every call.

## 2. Applicability

- Candidate: a build of `horizon` and `horizon-browser` from this branch or
  later, and the image of `packaging/browser-vm`.
- Platforms: Linux on the computer of the person.
- The other machine is a container on this computer, so the run rents nothing.
  The same run also covers a browser host on a second computer, when the person
  has one.
- This procedure does not test: a provider device target (`browser.remote.targets`),
  the sign-in to a real account, and the recording of the run as a video.

## 3. Safety

> **CAUTION:** A FAILED RUN KEEPS ITS PAGE ON THE OTHER MACHINE. Run the command again, which
> closes the page of the earlier run first, or close it with `browser_close`.

> **CAUTION:** REMOVE ONLY THE CONTAINER AND THE VOLUME OF THIS RUN. Another volume holds the
> sign-ins of another person.

> **CAUTION:** A STAGED FILE EXISTS ON THE OTHER MACHINE FOR THE DURATION OF THE CALL. The file
> of this run holds the text `remote smoke attachment` and no secret.

## 4. Equipment and preconditions

- A Linux computer with a working Docker daemon.
- The checkout at the commit under test, built:
  `cargo build --bin horizon --bin horizon-browser`.
- The image of the browser machine, built from that checkout:
  `docker build --file packaging/browser-vm/Dockerfile --target browser-vm-local --tag horizon-browser-vm-local:latest .`
- Python 3.11 or later. The script uses the standard library only.

## 5. Setup

1. Start the browser machine on this computer.

   ```bash
   ./target/debug/horizon --browser-vm-local up --build .
   ```

   Result: The command prints `the container horizon-browser-vm-local runs`,
   `the ssh target is horizon-browser-vm`, and the loopback address of its
   display. It also writes the host block `horizon-browser-vm` in
   `~/.ssh/config`.

2. Read the environment of the SSH session of the container.

   ```bash
   docker exec horizon-browser-vm-local cat /root/.ssh/environment
   ```

   Result: The output names `HORIZON_BROWSER_ROOT=/state/browser-root`, the
   display `:99`, and `HORIZON_BROWSER_ATTACHMENT_ROOTS=/state/browser-root/browser-staging`.
   The third line is the staging folder that a bridge copies an attachment into.

3. Write the page of this procedure on the machine.

   ```bash
   docker exec horizon-browser-vm-local sh -c \
     'mkdir -p /state/test && cat > /state/test/upload.html' <<'HTML'
   <html><body><h1>Remote smoke</h1>
   <input id="name" type="text">
   <input id="f" type="file">
   <p id="out">none</p>
   <script>
   document.getElementById('f').addEventListener('change', (event) => {
     const file = event.target.files[0];
     document.getElementById('out').textContent = file ? file.name : 'none';
   });
   </script></body></html>
   HTML
   ```

   Result: The command prints nothing, and
   `docker exec horizon-browser-vm-local ls -l /state/test/upload.html` names the
   file with the mode `0644`.

## 6. Task S1: One ordered run of the lane

1. Run the smoke against the machine.

   ```bash
   python3 -B scripts/remote-browser-control/smoke.py \
     --horizon target/debug/horizon \
     --remote horizon-browser-vm \
     --profile probe \
     --report-dir /tmp/remote-smoke
   ```

   Result: The command prints one line for each step, and the last line is
   `21 of 21 steps held; report in /tmp/remote-smoke/report.json`. The exit
   status is `0`.

2. Read the report.

   ```bash
   python3 -c 'import json;print(json.load(open("/tmp/remote-smoke/report.json"))["held"])'
   ```

   Result: The command prints `True`.

3. Read the refusals of the run in the report.

   ```bash
   python3 -c 'import json;print([s for s in json.load(open("/tmp/remote-smoke/report.json"))["steps"] if s["step"].startswith("refuse")])'
   ```

   Result: The list holds three steps, each with `"held": true`:
   - `refuse a handoff of a page elsewhere`, which reports
     `unsupported_backend`, because no person here can steer a page on another
     machine;
   - `refuse a second page without the opt-in`, because one agent holds one page
     for each engine;
   - `refuse a reference that the page does not have`, which reports
     `stale_reference`.

4. Read the transcript of the calls.

   ```bash
   wc -l /tmp/remote-smoke/rpc-horizon-browser-vm.jsonl
   ```

   Result: The file holds one line for each call of the run, and each line names
   the tool, its arguments and whether it held.

## 7. Task S2: The page is not the page of this computer

1. Ask the machine for the files of the page folder of the run.

   ```bash
   docker exec horizon-browser-vm-local ls /state/browser-root/profile-host-captures | head -3
   ```

   Result: The command names one folder for each page of the run, and the
   screenshot and the recording of the run are inside that folder.

2. Ask this computer for the same page.

   ```bash
   ls ~/.horizon/cloud/browser-vm 2>/dev/null; ls /tmp/remote-smoke
   ```

   Result: The report folder holds the report, the transcript, the log and the
   uploaded file. It holds no screenshot, no recording and no network capture,
   because those files stay on the machine that runs the browser.

## 8. Task S3: A run that stops in the middle is repeatable

1. Stop the run in the middle, so its page stays.

   ```bash
   python3 -B scripts/remote-browser-control/smoke.py \
     --remote horizon-browser-vm --profile probe --report-dir /tmp/remote-smoke \
     --page https://example.org/ --timeout 1
   ```

   Result: The run fails and leaves its page, or it passes, and either answer is
   acceptable for this step.

2. Run the smoke again, as in task S1.

   Result: The first step is `pass start from an empty lane` and names the pages
   that it closed. The run then reaches `21 of 21 steps held`.

## 9. Pass criteria

- Every step of task S1 holds, and the report says `"held": true`.
- The three refusals are present and each one holds.
- The screenshot, the recording and the network capture stay on the other
  machine, and the report folder holds none of them.
- A second run closes the page of the first.

## 10. Cleanup

1. Remove the container of the run.

   ```bash
   ./target/debug/horizon --browser-vm-local down
   ```

   Result: The command prints `the container horizon-browser-vm-local stopped`,
   and `docker ps --all` names no container of the run.

2. Remove the state volume and the image of the run.

   > **CAUTION:** A REMOVED VOLUME DELETES EVERY PROFILE AND EVERY SIGN-IN IN IT. REMOVE ONLY
   > THE VOLUME OF THIS RUN.

   ```bash
   docker volume rm horizon-browser-vm-local-state
   docker image rm horizon-browser-vm-local:latest
   ```

   Result: `docker volume ls` and `docker image ls` name neither.

3. Remove the host block of the run from the ssh configuration of the person,
   when no local browser machine remains.

   > **CAUTION:** THE NEXT START OF THE COMMAND OF STEP 5.1 WRITES THE BLOCK AGAIN.

   ```bash
   sed -i '/^# >>> horizon browser vm/,/^# <<< horizon browser vm/d' ~/.ssh/config
   ```

   Result: `ssh -G horizon-browser-vm` names no block that Horizon wrote.

4. Remove the report folder.

   ```bash
   rm -rf /tmp/remote-smoke
   ```

   Result: The folder is absent.

## 11. Record of results

Record the run in a report under `docs/testing/reports/` from
[the report template](TEMPLATE.md). Keep these values:
- the commit under test and the SHA-256 of the binary, from `report.json`;
- the number of steps that held;
- the three refusal codes;
- the paths of the screenshot and the recording, as the other machine names
  them.
