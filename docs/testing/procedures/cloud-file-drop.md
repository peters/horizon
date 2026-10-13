---
procedure: cloud-file-drop
feature: Local files dropped on a cloud agent or shell panel
platforms: [linux]
cost: rents compute  # F05 only; F01 to F04 and F06 use only the local fixture
destructive: no
secrets: [provider API key, Claude API key]  # F05 only
owner: peters
---

# Cloud file drop test procedure

## 1. Purpose

This procedure makes sure that files dropped on a cloud panel go to the worker of
that panel. The panel gets the quoted worker paths only after all files arrived.
A failure or a cancel shows a reason and pastes no path. A local panel still gets
local paths.

## 2. Applicability

- Candidate: a build that uploads drops on cloud panels.
- Platforms: Linux with the local device smoke fixture and a stand-in worker.
- This procedure does not test these functions:
  - A worker at a provider. Task F05 gives the same check for a real cloud.
  - Agent isolation. The stand-in worker has one user. Unit tests make sure that
    the worker commands give the files to the group of the SSH user.
  - Drops on cloud browser panels. That is a different feature.

## 3. Safety

> **CAUTION:** USE ONLY THE SYNTHETIC FIXTURE AND SYNTHETIC FILES IN TASKS F01 TO
> F04 AND F06. The fixture has no provider key, so these tasks send no request to a
> provider.

> **CAUTION:** DO TASK F05 ONLY WITH APPROVAL FOR ONE PAID WORKER. F05 rents a worker
> and uses the provider API key and the Claude API key. Follow the cautions of the
> [cloud agent panel procedure](cloud-agent-panel-start.md).

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The [local device smoke fixture](../../../scripts/device-smoke/README.md),
  `scripts/device-smoke/serve.py`, with `--native-view`.
- A tools root, `<tools>`, outside the home directory. Unpack these Debian
  packages into it with `apt-get download` and `dpkg-deb -x`: `openssh-server`,
  `libnss-wrapper`, `x11vnc`, `libvncserver1` and `libvncclient1`.
- Python 3 with GTK 3 (`python3-gi`), `xdotool`, `bwrap` and ImageMagick.
- The files in [`cloud-file-drop/`](cloud-file-drop/):
  - `worker.py` starts the stand-in worker: an SSH server on `127.0.0.1` with a
    private `/workspace`.
  - `horizon-seeded`, `seed.py` and `runtime.yaml` give the candidate a ready
    cloud `drop-demo` on that worker and a local shell panel.
  - `drag.py` shows a small window that drags files, as a file manager does.
- A new directory for the candidate files, `<seed>`, outside the home directory.
- A private evidence directory, `<evidence>`, outside the fixture state.
- For F05 only: approval for one paid worker, and the equipment of the
  [cloud agent panel procedure](cloud-agent-panel-start.md), with its provider API
  key and Claude API key.

## 5. Setup

1. Copy the seed files and the frozen candidate into `<seed>`. Name the candidate
   `horizon`.

   ```sh
   cp docs/testing/procedures/cloud-file-drop/{horizon-seeded,seed.py,runtime.yaml} <seed>/
   cp <frozen-candidate> <seed>/horizon
   ```

   Result: `<seed>` contains `horizon`, `horizon-seeded`, `seed.py` and `runtime.yaml`.

2. Start the stand-in worker in a separate terminal.

   ```sh
   python3 docs/testing/procedures/cloud-file-drop/worker.py \
     --tools <tools> --state <new-worker-directory> --seed <seed>
   ```

   Result: The output shows a `port` and a `workspace`. `<seed>` contains `worker.json`.

3. Start the fixture with the wrapper and a new state directory.

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <seed>/horizon-seeded \
     --tools <tools> --native-view --state <new-directory>
   ```

   Result: The fixture output shows a `display` and a `vnc_address`.

4. Open a Device panel with the `vnc_address`.

   Result: The Device panel shows a live view of the candidate.

5. Find the Horizon process of the fixture. Compare the SHA-256 of
   `/proc/<pid>/exe` with the SHA-256 of the frozen candidate.

   Result: The two values are the same.

6. Make three synthetic files in a new directory, `<files>`, outside the home
   directory: `screen shot.png`, `O'Brien notes.txt` and `ærlig 测试.jpeg`.

   ```sh
   convert -size 320x200 gradient:blue-orange "<files>/screen shot.png"
   convert -size 200x120 plasma:fractal "<files>/ærlig 测试.jpeg"
   printf 'Synthetic notes.\n' > "<files>/O'Brien notes.txt"
   ```

   Result: `<files>` contains the three files.

7. Record the SHA-256 of each file in `<files>` in the evidence.

   Result: The evidence has three values.

8. In the candidate, click the chevron of the `drop-demo` card, then click
   **Manage** and **Full screen**.

   Result: The candidate shows only the cloud.

9. Push Ctrl+Shift+N.

   Result: A **Shell** panel opens in the cloud. It shows the prompt
   `worker:/workspace$`.

10. Push Esc.

    Result: The board shows the cloud with its **Shell** panel and the
    **local shell** panel.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result. Use `xdotool` only on
the display of the fixture. Start `drag.py` with `DISPLAY` set to that display.

### 6.1 F01: Drop several files on a cloud panel

1. Start the drag window with the three files.

   ```sh
   cd <files> && DISPLAY=<display> python3 <path>/drag.py 240 830 \
     "screen shot.png" "O'Brien notes.txt" "ærlig 测试.jpeg"
   ```

   Result: A white window with the three names shows in the fixture.

2. Drag from the window to the body of the cloud **Shell** panel.

   Result: The upload window opens with the progress of the files, then closes.

3. Examine the prompt of the cloud **Shell** panel.

   Result: The prompt shows three quoted paths that start with
   `/workspace/.horizon-drops/`. Each path ends with the name of a file.

4. Push Ctrl+A, type `sha256sum `, then push Enter.

   Result: The panel shows three SHA-256 values. They are the same as the values
   in the evidence.

5. Examine the batch folder in the `workspace` of the stand-in worker.

   Result: The folder has mode `0750` and each file has mode `0640`.

### 6.2 F02: Drop a folder

1. Start the drag window with `screen shot.png` and a folder.

   Result: The window shows the two names.

2. Drag from the window to the body of the cloud **Shell** panel.

   Result: The upload window shows **Upload failed** and says that the folder is a
   folder. The panel shows no new path.

3. Click **Close**.

   Result: The upload window closes.

### 6.3 F03: Cancel an upload

1. Make a sparse file of 6 GB in a new directory outside the home directory.

   ```sh
   truncate -s 6G "<large>/large disk image.img"
   ```

   Result: The file exists.

2. Drag the file to the body of the cloud **Shell** panel.

   Result: The upload window shows the progress, the speed and the time left.

3. Click **Cancel Upload**.

   Result: The upload window shows **Upload cancelled** and **No path was
   inserted**.

4. Examine the session folder in `.horizon-drops` of the stand-in worker.

   Result: The batch folder and the `.part` folder of the cancelled drop are gone.
   Earlier batch folders stay.

5. Click **Done**.

   Result: The upload window closes.

### 6.4 F04: Refuse a changed host key, then try again

1. Copy the file `known-hosts-stand-in` of the cloud in the private home of the
   fixture to `<evidence>`.

   Result: `<evidence>` contains the copy.

2. Replace the key in that file with the key of a new, synthetic key pair.

   Result: The file names `horizon-cloud-stand-in` with a different key.

3. Drag `screen shot.png` to the body of the cloud **Shell** panel.

   Result: The upload window shows **Upload failed**. The text starts with
   `The worker did not show the host key` and ends with
   `Host key verification failed.`

4. Examine the session folder in `.horizon-drops` of the stand-in worker.

   Result: The folder has no new batch folder.

5. Put back the copy from `<evidence>`.

   Result: The file has the original key.

6. Click **Try Again**.

   Result: The upload window closes. The panel shows one new quoted path.

### 6.5 F05: Drop files on a real cloud agent

> **CAUTION:** START ONLY ONE CLOUD FOR THIS TASK AND STOP IT AFTER THE TASK. The
> provider charges money until the worker stops.

1. Start a cloud with the
   [cloud agent panel procedure](cloud-agent-panel-start.md), steps 1 to 11 of
   section 5.

   Result: The card shows **Ready**.

2. Open a Claude Code panel in the cloud.

   Result: The agent starts.

3. Drag the three files of `<files>` to the body of the Claude Code panel.

   Result: The upload window closes. The input of the agent shows three quoted
   worker paths.

4. Ask the agent to give the SHA-256 of each file.

   Result: The values are the same as the values in the evidence.

### 6.6 F06: Drop a file on a local panel

1. Drag `screen shot.png` to the body of the **local shell** panel.

   Result: The panel shows the quoted local path of the file. No upload window
   opens.

## 7. Pass criteria

- F01: The panel shows the worker paths only after the upload, and the SHA-256
  values are the same as the local values.
- F02, F03 and F04: The panel shows no new path, and the upload window shows the
  reason.
- F03: The worker keeps no partial folder.
- F04: The worker gets no file while the host key is different.
- F05: The agent reads the same content as the local files.
- F06: The local panel gets the local path.

## 8. Cleanup

1. Close the Device panel.

   Result: The Device panel closes. The fixture continues.

2. Stop the fixture with SIGTERM on the exact PID of `serve.py`.

   Result: The fixture stops its processes and removes its private data.

3. Stop the stand-in worker with Ctrl-C.

   Result: The SSH server stops.

4. Remove `<seed>`, `<files>`, the large file and the worker directory.

   Result: No fixture file stays.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
