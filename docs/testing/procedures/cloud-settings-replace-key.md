---
procedure: cloud-settings-replace-key
feature: Cloud settings saved keys
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Cloud settings Replace key test procedure

## 1. Purpose

This procedure makes sure that **Replace** and **Keep saved key** work on each
saved key in the **Cloud settings…** dialog. It also makes sure that the card
layout stays compact when a replacement field is open. The dialog must not move
or change size after it opens.

## 2. Applicability

- Candidate: each candidate that changes the Cloud settings dialog.
- Platforms: Linux, in an isolated desktop with a live view.
- This procedure does not test: a save of a new key, a deploy, or a provider
  call. The procedure uses only fake keys and makes no network requests.

## 3. Safety

> **CAUTION:** USE ONLY FAKE KEYS IN THE PRIVATE HOME OF THE FIXTURE. If you
> copy a real key or a real `~/.horizon` file, a recording can show it.

> **CAUTION:** DO NOT CLICK SAVE SETTINGS. This procedure must not write the
> fake keys over the files that it prepared.

## 4. Equipment and preconditions

- The [local device smoke fixture](../../../scripts/device-smoke/README.md)
  with `--native-view`.
- A frozen candidate and its SHA-256.
- A private evidence directory, `<evidence>`, outside the fixture state.
- A Device panel that shows a live view of the fixture.
- No real credentials. The fake key files contain text such as
  `rpa_FAKEKEYFORSMOKE`.

## 5. Setup

1. Start the fixture with the frozen candidate and a new state directory.

   Result: The fixture writes `lab.json` and `target.json` in the state directory.

2. Make the directory `<state>/data/home/.horizon/cloud/credentials` with mode `0700`.

   Result: The directory is in the private home of the fixture.

3. Write the fake RunPod, Hetzner, Codex and Claude keys to files with mode `0600`.

   Result: Each file contains one fake key and no real credential.

4. Write `<state>/data/home/.horizon/cloud/settings.json` with mode `0600`.
   In this file, `<home>` is the real home path. The fixture shows the private
   home of the candidate at that path.

   ```json
   {
     "default_agents": ["codex", "claude"],
     "runpod_key_file": "<home>/.horizon/cloud/credentials/compute",
     "ssh_identity_file": "<home>/.horizon/cloud/ssh/id_ed25519",
     "docker_config": "<home>/.horizon/cloud/docker",
     "registry_pull_auth_id": null,
     "cpu_flavors": ["cpu3c"],
     "gpu_types": ["NVIDIA RTX A6000"],
     "openai_api_key_file": "<home>/.horizon/cloud/credentials/openai",
     "anthropic_api_key_file": "<home>/.horizon/cloud/credentials/anthropic",
     "hetzner": {
       "token_file": "<home>/.horizon/cloud/credentials/hetzner",
       "server_types": ["cx43"],
       "locations": ["hel1"]
     }
   }
   ```

   Result: The file is in the private home of the fixture.

5. Record the SHA-256 of `settings.json` and of the four key files.

   ```sh
   cd <state>/data/home/.horizon/cloud
   sha256sum settings.json credentials/* > <evidence>/baseline.sha256
   ```

   Result: `baseline.sha256` has five lines. Keep it outside the private home.

6. Start a recorder that captures only the fixture display.

   Result: The recorder writes frames from the isolated desktop.

## 6. Tasks

### 6.1 A01 — Open Cloud settings

1. In the toolbar, click **Menu**, then **Cloud**.

   Result: The Cloud submenu opens.

2. Click **Cloud settings…**.

   Result: The dialog opens. The RunPod, Hetzner Cloud and Coding agents cards
   show `Key saved`. Each saved key shows **Replace**.

3. Do not move the pointer for 2 seconds.

   Result: The recorder captures the first 2 seconds of the dialog.

4. Find the first recorded frame that shows the dialog.

   Result: The frame shows the title, **Cancel** and **Save settings**.

5. Measure the top edge of the dialog in each frame of the next 2 seconds.

   Result: The top edge is at the same position in each frame.

6. Compare the position of **Replace** on the RunPod card in each frame that shows it.

   Result: **Replace** is at the same position in each frame.

### 6.2 A02 — Replace and keep the RunPod key

1. Click **Replace** on the RunPod card.

   Result: An empty key field opens. **Keep saved key** is directly below the
   field. The caption is directly below **Keep saved key**.

2. Click **Keep saved key**.

   Result: The masked saved key and **Replace** show again.

### 6.3 A03 — Replace and keep the Hetzner key

1. Click **Replace** on the Hetzner Cloud card.

   Result: **Keep saved key** is directly below the field. There is no large
   empty area in the card.

2. Click **Keep saved key**.

   Result: The masked saved key shows again.

### 6.4 A04 — Replace and keep the agent keys

1. Click **Replace** below **Codex** in the Coding agents card.

   Result: **Keep saved key** is directly below the field. The **Claude**
   section is directly below the button.

2. Click **Keep saved key**.

   Result: The masked Codex key shows again.

3. Click **Replace** below **Claude**.

   Result: **Keep saved key** is directly below the field.

4. Click **Keep saved key**.

   Result: The masked Claude key shows again.

### 6.5 A05 — Close without a save

1. Click **Cancel**.

   Result: The dialog closes.

2. Compare the five files with the baseline.

   ```sh
   cd <state>/data/home/.horizon/cloud
   sha256sum -c <evidence>/baseline.sha256
   ```

   Result: Each of the five lines shows `OK`. Record the output in the results.

## 7. Pass criteria

- In A01, the dialog and its controls do not move after the first frame that
  shows the dialog.
- In each task, **Keep saved key** is directly below its field.
- No card shows a large empty area between a field and its button.
- **Keep saved key** removes the field and shows the masked saved key again.
- In A05, the comparison shows `OK` for all five files.
- The recording and the screenshots show only fake keys.

## 8. Cleanup

1. Stop the recorder.

   Result: The recording is complete and plays back.

2. Close the Device panel of this run.

   Result: The Device panel closes. The fixture continues.

3. Stop the fixture with Ctrl-C.

   Result: The fixture removes `target.json` and the private home.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
