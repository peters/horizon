---
procedure: cloud-setup-agent
feature: Set up from CI in New cloud (setup agent and Horizon's proof)
platforms: [linux]
cost: none
destructive: yes
secrets: [the local login of the agent (Codex or Claude)]
owner: peters
---

# Setup agent test procedure

## 1. Purpose

This procedure proves that **Set up from CI** in **New cloud** runs a setup agent
in the dialog, that Horizon proves the agent's worker image itself (image build,
worker check, the CI jobs with localci and a fast rerun), and that **Use this
setup** commits `.horizon` and reads it as the repository profile.

## 2. Applicability

- Candidate: a Horizon build of the pull request head.
- Platforms: Linux with Docker. The isolated fixture of
  [area S of the cloud panels procedure](cloud-panels/s-test-fixture.md) is
  recommended.
- This procedure does not test: a deploy, a provider, or a paid agent API key.
  No step rents compute.

## 3. Safety

> **CAUTION:** DO NOT CLICK **Start cloud**. This button rents compute from the
> provider.

> **CAUTION:** PUT ONLY TEST CONTENT IN THE FIXTURE REPOSITORY. The agent reads all
> of it, and the proof copies it into a container.

## 4. Equipment and preconditions

- Docker on the host, and about 10 GB of free disk for the image.
- A local login of Codex or Claude for the user who runs Horizon. The agent uses
  this login. It needs no API key.
- Network access to GitHub, for the public worker base image and localci.

## 5. Setup

1. Make the fixture repository `<home>/smoke/ci-app` with one commit that has these
   files:

   - `test.sh`, executable: `#!/usr/bin/env bash` and `echo "fixture tests passed"`.
   - `.github/workflows/ci.yml`: one job `test` on `ubuntu-latest` with the steps
     `actions/checkout` and `run: ./test.sh`.

   Result: `git -C <home>/smoke/ci-app log --oneline` shows one commit.

2. Start the candidate with the fixture configuration.

   Result: The Horizon window opens with no cloud.

## 6. Tasks

### 6.1 S01 — Offer the setup choices

1. Click **Cloud**, then **New cloud…**, and type `<home>/smoke/ci-app` as the
   repository.

   Result: The dialog shows **No cloud setup yet** with two cards: **Quick start**
   with **Start on the base image**, and **Set up from CI** with one chip for each
   agent of the New Workspace menu (for example **Codex** and **Claude**).

2. Click the **Codex** chip.

   Result: The button says **Set up with Codex**. Each agent shows one chip, in
   its default mode, with no YOLO or Auto variant.

### 6.2 S02 — Run the setup agent in the dialog

1. Click **Set up with Codex**.

   Result: The dialog changes to **Set up ci-app** with the text "An agent prepares
   a worker image from your CI. Nothing is rented yet." The left column shows the
   steps Read CI, Find tools, Write Dockerfile, Build image, Run CI in image,
   Desktop preview and Fast rerun. The right column shows the agent's terminal
   with "live · click to type to the agent". No panel appears on the canvas.

2. If the agent asks a question, click its terminal and answer it.

   Result: The question also shows above the steps as **The agent asks**. It goes
   away when you answer.

3. Wait until the agent writes `.horizon/Dockerfile` and `.horizon/cloud.yml`.

   Result: Read CI, Find tools and Write Dockerfile are done, each with a short
   detail and its elapsed time.

### 6.3 S03 — Horizon proves the image

1. Wait for the agent to ask for a proof.

   Result: Build image shows "Horizon is running it" and the last lines of the
   proof log show under the steps. Then Run CI in image and Fast rerun run.

2. Wait for the proof to end.

   Result: Build image, Run CI in image and Fast rerun show "proved by Horizon"
   with their times. The fast rerun is much shorter than the CI run. The header
   says **Ready** with the agent's summary, and the footer lists the `.horizon`
   files.

### 6.4 S04 — Use the setup

1. Click **Use this setup**.

   Result: The dialog shows the repository profile from `.horizon/cloud.yml`, with
   no red error. The agent's panel is gone.

2. Run `git -C <home>/smoke/ci-app log --oneline -1`.

   Result: The last commit is "Add the Horizon cloud worker setup" and contains
   only files in `.horizon`.

### 6.5 S05 — Stop a setup

1. Open **New cloud** for a copy of the fixture without `.horizon`, start the
   setup agent, then click **Stop** before it is done.

   Result: The agent stops, its hidden panel closes and the dialog shows the setup
   choices again. No file outside `.horizon` changed.

### 6.6 S06 — Close the dialog and come back

1. Start the setup agent on a copy of the fixture without `.horizon`, then click
   **Close** while it works.

   Result: The dialog closes. The agent keeps working.

2. Wait one minute, then open **New cloud** for the same repository.

   Result: The dialog shows the same run with its steps and elapsed times. A proof
   that the agent asked for while the dialog was closed ran or runs.

### 6.7 S07 — A failed proof

1. Start the setup agent on a copy of the fixture whose `test.sh` ends with
   `exit 1`.

   Result: Run CI in image shows a red failure with the exit code, and the last
   lines of the proof log show the failing test.

2. Examine the footer.

   Result: **Use this setup** is not available while the proof failed, even when
   the agent wrote a summary. The agent gets the failure and the path of the whole
   log.

### 6.8 S08 — Update a committed setup

1. Open **New cloud** for `<home>/smoke/ci-app` after S04, and click **Update setup
   from CI**.

   Result: The dialog shows **Update the setup of ci-app**. The agent starts from
   the committed `.horizon` files and keeps the profile choices.

2. When the run is ready, click **Use this setup**.

   Result: When the agent changed nothing, no new commit is made and the dialog
   reads the committed profile, with no error.

## 7. Pass criteria

- S01 to S08 give the results above.
- The proof ran on this computer. Before you click **Use this setup** in S04,
  `docker images` lists `horizon-setup-ci-app-<hash>-<run>:candidate`. `<hash>` is
  six hexadecimal digits that Horizon derives from the repository's path. `<run>` is
  eight hexadecimal digits for the setup run.
- Within one minute after **Use this setup** or **Stop**, `docker images` and
  `docker volume ls` no longer list the image and the volume of that run.
- No cloud was started.

## 8. Cleanup

> **CAUTION:** THE NEXT STEP REMOVES DOCKER IMAGES AND VOLUMES. Remove only the
> names that start with `horizon-setup-ci-app-`.

1. Find the proof image and volume with
   `docker images --format '{{.Repository}}' | grep '^horizon-setup-ci-app-'` and
   `docker volume ls --format '{{.Name}}' | grep '^horizon-setup-ci-app-'`, then remove
   each with `docker rmi <name>:candidate` and `docker volume rm <name>`.

   Result: The two commands no longer list them.

> **CAUTION:** THE NEXT STEP DELETES A FOLDER. Make sure that the path is the
> fixture copy `<home>/smoke/ci-app` and not a real repository.

2. Delete `<home>/smoke/ci-app`.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
