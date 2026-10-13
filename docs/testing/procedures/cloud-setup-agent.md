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
- For a pull request that changes the setup agent, the proof or the desktop preview,
  also run [the setup gate](cloud-setup-gate.md). It runs the proof on real
  repositories.

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

2. Copy `<home>/smoke/ci-app` to `<home>/smoke/gui-app`. In the copy, add the
   executable file `app.sh` with `#!/usr/bin/env bash` and
   `exec xmessage -geometry 640x400 "gui-app fixture"`, and add the line "Start the
   app with ./app.sh" to `README.md`. Then run
   `git -C <home>/smoke/gui-app add -A` and
   `git -C <home>/smoke/gui-app commit -q -m "Add the app"`.

   Result: `git -C <home>/smoke/gui-app status --short` shows nothing, and
   `git -C <home>/smoke/gui-app log --oneline -1` shows "Add the app". The proof
   clones the committed tree, so an uncommitted change is not in it.

3. Copy `<home>/smoke/ci-app` to `<home>/smoke/plain-app`. In the copy, run
   `git -C <home>/smoke/plain-app rm -r -q .github`, add the line "Run the tests
   with ./test.sh" to `README.md`, then run `git -C <home>/smoke/plain-app add -A`
   and `git -C <home>/smoke/plain-app commit -q -m "Remove the workflows"`.

   Result: `git -C <home>/smoke/plain-app status --short` shows nothing, and
   `git -C <home>/smoke/plain-app ls-files .github` shows nothing.

4. Make the fixture repository `<home>/smoke/rust-app` with one commit: a Rust
   binary crate whose `Cargo.toml` names one git dependency by `rev`, its
   `Cargo.lock`, and `.github/workflows/ci.yml` with one job `test` that runs
   `cargo test --locked`. Add `rust-toolchain.toml` with `channel = "1.90.0"`. The
   git dependency must be a private GitHub repository: your Git login on this
   computer can clone it, and `git ls-remote` of it without credentials fails.

   Result: `git -C <home>/smoke/rust-app show HEAD:Cargo.lock` has one line that
   starts with `source = "git+`.

5. Start the candidate with the fixture configuration.

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
   Fast rerun and Desktop preview. The right column shows the agent's terminal
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

### 6.9 S09 — Start the setup from a cloud workspace

1. Make a cloud workspace the active workspace. Open **New cloud**, select a
   local repository that has no `.horizon/cloud.yml` and click **Set up with**
   an agent.

   Result: The hidden panel of the setup agent is in an ordinary local
   workspace and not in the cloud workspace. Its working folder is the selected
   repository. No compute is rented and no remote credential is used.

2. Open a local workspace, make the cloud workspace active again and do step 1
   again.

   Result: The setup uses the open local workspace. It never uses a remote
   workspace.

### 6.10 S10 — A live desktop preview

1. Open **New cloud** for `<home>/smoke/gui-app` and start the setup agent.

   Result: The steps are Read CI, Find tools, Write Dockerfile, Build image, Run CI
   in image, Fast rerun and Desktop preview.

2. Wait for the proof to pass.

   Result: Desktop preview shows "Horizon is starting the app". Then it shows the
   window size and "live in Horizon", and **Show live preview** appears.

3. Click **Show live preview**.

   Result: The dialog closes. A Device panel named "gui-app preview" shows the
   xmessage window with the text "gui-app fixture". The address of the panel is on
   `127.0.0.1`.

4. Open **New cloud** again.

   Result: The dialog shows the finished setup with **Use this setup**.

5. Click **Use this setup**.

   Result: The "gui-app preview" panel closes and the dialog reads the committed
   profile. `git -C <home>/smoke/gui-app log --oneline -1` shows "Add the Horizon
   cloud worker setup". Within one minute, `docker ps --all` lists no container and
   `docker network ls` lists no network whose name starts with
   `horizon-setup-gui-app-`.

### 6.11 S11 — A proof with commands

1. Open **New cloud** for `<home>/smoke/plain-app` and start the setup agent.

   Result: The agent finds no GitHub Actions and asks Horizon to prove the image
   with the commands from the README.

2. Wait for the proof to end.

   Result: Run CI in image and Fast rerun pass with their times, and the last
   lines of the proof log show "fixture tests passed".

3. Click **Stop**.

   Result: Within one minute, `docker ps --all`, `docker volume ls` and
   `docker network ls` list no name that starts with `horizon-setup-plain-app-`.

### 6.12 S12 — The providers in the heading

1. Open **New cloud** for `<home>/smoke/ci-app` after S04.

   Result: The heading shows one chip for each provider that the dialog compares,
   each with its mark and then its name. When the title is empty, the hint for the
   title shows under the title field.

### 6.13 S13 — Replace and withdraw a live preview

> **CAUTION:** THE NEXT STEP REMOVES A COMMIT. Make sure that the path is the
> fixture copy under `<home>/smoke` and not a real repository.

1. Run `git -C <home>/smoke/gui-app reset -q --hard HEAD~1`.

   Result: `git -C <home>/smoke/gui-app log --oneline -1` shows "Add the app".

2. Open **New cloud** for `<home>/smoke/gui-app`, start the setup agent and wait
   until Desktop preview shows "live in Horizon".

   Result: A "gui-app preview" panel shows the xmessage window, and
   `docker ps --format '{{.Names}}'` lists one name that ends in `-preview-1`.

3. Click the agent's terminal in the dialog, type "Set preview_request to 2 in the
   status file." and press Enter.

   Result: Desktop preview shows "Horizon is starting the app" and the "gui-app
   preview" panel closes. Then a new "gui-app preview" panel opens on
   `127.0.0.1` with the xmessage window. `docker ps --format '{{.Names}}'` lists a
   name that ends in `-preview-2` and none that ends in `-preview-1`.

4. Type "Set preview to null in the status file and mark Desktop preview skipped."
   and press Enter.

   Result: The "gui-app preview" panel closes and Desktop preview shows skipped.
   Within one minute, `docker ps --format '{{.Names}}'` lists no name that ends in
   `-preview-2`.

5. Click **Stop**.

   Result: Within one minute, `docker ps --all`, `docker volume ls` and
   `docker network ls` list no name that starts with `horizon-setup-gui-app-`.

### 6.14 S14 — The app of a live preview stops

1. Open **New cloud** for `<home>/smoke/gui-app`, start the setup agent and wait
   until Desktop preview shows "live in Horizon".

   Result: `docker ps --format '{{.Names}}'` lists one name that ends in
   `-preview-1`.

2. Run `docker kill <name>` with that name.

   Result: Within 10 s, Desktop preview shows failed with "the app stopped while it
   was live", the "gui-app preview" panel closes and **Use this setup** does not
   show. The agent's terminal shows "Horizon desktop preview failed".

3. Click **Stop**.

   Result: Within one minute, `docker ps --all`, `docker volume ls` and
   `docker network ls` list no name that starts with `horizon-setup-gui-app-`.

### 6.15 S15 — An organization blocks Horizon's GitHub app

Steps 1 to 8 use a test double for the build. Step 9 uses a real organization and
is optional.

Test double: put an executable `docker` script first on the `PATH` of the candidate
only, not of your shell. For `docker buildx build`, the script writes these lines, as
buildx does when ghcr.io refuses an image, and exits with 1. For all other commands,
it runs `/usr/bin/docker`:

```text
#2 [internal] load metadata for ghcr.io/acme/base:1
ERROR: failed to build: failed to solve: ghcr.io/acme/base:1: failed to resolve source metadata for ghcr.io/acme/base:1: unexpected status from HEAD request to https://ghcr.io/v2/acme/base/manifests/1: 403 Forbidden
denied: Although you appear to have the correct authorization credentials, the `acme` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited.
```

1. Make a fixture repository `<home>/smoke/org-app` like `ci-app` whose README names
   the base image `ghcr.io/acme/base:1`. Open **New cloud** for it and click **Set
   up with <agent>**.

   Result: The agent writes a Dockerfile `FROM ghcr.io/acme/base:1` and asks for a
   proof.

2. Wait for the proof.

   Result: **Build image** fails. A card **acme blocks Horizon's GitHub app** shows,
   with **Let the agent do it** and **Open GitHub myself**. The agent gets a message
   that the Dockerfile is not the cause and that it must change no file.

3. Click **Let the agent do it**.

   Result: A browser panel **GitHub access** opens in the agent's workspace at
   Horizon's app page on GitHub. The card says that the agent makes the fix and
   shows **Show browser**. The agent gets the task. When GitHub shows a sign-in
   page, the agent hands the browser to you and does not type a password or code.

4. Click **Close**, close the **GitHub access** panel on the canvas, open **New
   cloud** again and click **Show browser**.

   Result: A new **GitHub access** panel opens and shows, and the agent gets the
   task again for the new panel.

5. Click **Stop**.

   Result: The **GitHub access** panel closes with the run.

6. Start the setup again and wait for the card. Click **Open GitHub myself**.

   Result: The **New cloud** dialog closes and the **GitHub access** panel shows. The
   agent gets no task.

7. Open **New cloud** again and click **Prove again**.

   Result: Before the click, the card shows **Show browser** and **Prove again**.
   After it, the agent gets the message that the fix was made on GitHub, and it asks
   for a new proof. **Build image** fails again, because the test double refuses
   every build, and the card comes back.

8. Click **Stop**, and remove the test double from the `PATH` of the candidate.

   Result: The **GitHub access** panel closes with the run.

> **CAUTION:** THE NEXT STEP CHANGES THE OAUTH APP POLICY OF A REAL ORGANIZATION.
> Use only an organization where you may grant Horizon. The grant stays after the
> test.

9. Optional, with a real organization that limits OAuth apps, has not granted
   Horizon, and owns a private image `ghcr.io/<organization>/<image>` (you must be an
   owner of it): set up a fixture whose README names that image, click **Let the
   agent do it** on the card, sign in when the agent asks, and let the agent grant
   Horizon to the organization.

   Result: The agent asks for a new proof, and **Build image** passes.

### 6.16 S16 — A preview that needs hardware virtualization

The result of step 2 depends on whether the host has `/dev/kvm`.

1. In `<home>/smoke/gui-app`, change the README so that the app needs hardware
   virtualization. Start the setup agent.

   Result: The agent adds `"kvm": true` to the preview in the status file.

2. Wait for Desktop preview.

   Result, with `/dev/kvm`: `docker inspect` of the container whose name starts
   with `horizon-setup-gui-app-` and ends with `-preview-1` shows the device
   `/dev/kvm`. With rootless Docker, the container user is `0:0`, with no group
   added; otherwise it is `10001:10001` with the group of `/dev/kvm` added.

   Result, without `/dev/kvm`: Desktop preview shows as skipped with "the app
   needs /dev/kvm, which this computer does not have". No preview container
   starts. When the other steps pass, **Use this setup** is available. When the
   device exists but a container cannot open it (rootless Docker without access
   for your account), the step shows as skipped with that reason instead.

### 6.17 S17 — The toolchain pins

1. Open **New cloud** for `<home>/smoke/rust-app` and start the setup agent.

   Result: The agent writes `.horizon/Dockerfile` with `ARG RUST_VERSION=1.90.0`
   and `.horizon/pins.json` with an entry for `rust-toolchain.toml`. It does not
   change `.github`. Horizon writes `.horizon/verify-pins.py`. The first lines of
   Build image show "Horizon pins match: RUST_VERSION=1.90.0". The summary of the
   agent names the CI step `python3 -I .horizon/verify-pins.py`.

2. Change `channel` in `rust-toolchain.toml` to `1.91.0` and commit that change.
   Tell the agent to prove the setup again without a change to the pins.

   Result: Build image fails in seconds. The proof log shows
   "pins 1.90.0, rust-toolchain.toml says 1.91.0".

3. Put back `channel = "1.90.0"` and commit it. In `.horizon/pins.json`, set `file`
   of the entry to the absolute path of a file outside the repository, and tell the
   agent to prove again.

   Result: Build image fails, and the proof log says that the file "is outside the
   repository". The log does not show the content of that file.

### 6.18 S18 — Private Rust git dependencies

> **CAUTION:** USE A FIXTURE WHOSE GIT DEPENDENCY YOU MAY SHARE. Horizon clones each
> repository that the box names with your Git login, and the proof's CI can read it.

1. In the run of S17, put back the `file` of the pin entry that step 3 of S17
   changed, so that `.horizon/pins.json` names `rust-toolchain.toml` again. Then
   find the box **Fetch private Rust git dependencies on this
   computer for the proof**.

   Result: The box shows because the committed `Cargo.lock` names a GitHub git
   source. The line under the box names that repository as `OWNER/NAME`. For
   `<home>/smoke/ci-app` the box does not show.

2. Check the box, then tell the agent to prove the setup again.

   Result: Before Run CI in image, Horizon clones the named repository. The CI job
   builds with the git dependency and does not ask for a credential. `docker
   inspect` of the CI container shows one read-only mount below
   `/opt/host-cargo-git` for each named repository, and no other mount of a Git or
   Cargo folder of this computer. After the proof ends,
   `~/.horizon/cloud/setup-fetch` has no `crates-*` folder.

3. Add a second git dependency to `Cargo.toml`, commit `Cargo.toml` and
   `Cargo.lock`, and tell the agent to prove the setup again.

   Result: Horizon clones only the repository that the box named when the run
   started. The new dependency is not cloned.

4. Clear the box, then tell the agent to prove the setup again.

   Result: Nothing is cloned, and the proof log says why for each repository. Run CI
   in image starts on a new volume: the proof log shows no step that a record of an
   earlier proof skipped.

### 6.19 S19 — An agent unchecks a companion

> **CAUTION:** USE A TEST CLOUD. An uncheck queues the removal of the source cloud's
> access to the companion worker. The access is gone only when `cloud_companions`
> shows `status: unselected`.

Precondition: a source cloud whose repository declares a companion that its owner
checked on the cloud card, and an agent panel in the same workspace.

1. From the agent, call `cloud_companion_uncheck` with the source cloud's ID and
   the companion's alias.

   Result: The answer has `"changed": true` and `"queued": true`. The box of the
   companion on the source cloud's card is clear. No cloud was started, stopped or
   deleted.

2. Call `cloud_companions` every 10 seconds until the companion's row shows
   `status: unselected`.

   Result: The row shows `status: unselected` within two minutes.

3. Call `cloud_companion_uncheck` again with the same arguments.

   Result: The answer has `"changed": false`. No tool checks a companion: only the
   owner can check the box again, on the card.

## 7. Pass criteria

- S01 to S19 give the results above.
- The proof ran on this computer. Before you click **Use this setup** in S04,
  `docker images` lists `horizon-setup-ci-app-<hash>-<run>:candidate`. `<hash>` is
  six hexadecimal digits that Horizon derives from the repository's path. `<run>` is
  eight hexadecimal digits for the setup run.
- Within one minute after **Use this setup** or **Stop**, `docker images` and
  `docker volume ls` no longer list the image and the volume of that run.
- No cloud was started.

## 8. Cleanup

> **CAUTION:** THE NEXT STEP REMOVES DOCKER CONTAINERS, IMAGES, VOLUMES AND NETWORKS. Remove
> only the names that start with `horizon-setup-ci-app-`, `horizon-setup-gui-app-`,
> `horizon-setup-plain-app-`, `horizon-setup-org-app-` or `horizon-setup-rust-app-`.

1. Find the proof containers, images, volumes and preview networks with
   `docker ps --all --format '{{.Names}}' | grep -E '^horizon-setup-(ci|gui|plain|org|rust)-app-'`,
   `docker images --format '{{.Repository}}' | grep -E '^horizon-setup-(ci|gui|plain|org|rust)-app-'` and
   `docker volume ls --format '{{.Name}}' | grep -E '^horizon-setup-(ci|gui|plain|org|rust)-app-'` and
   `docker network ls --format '{{.Name}}' | grep -E '^horizon-setup-(ci|gui|plain|org|rust)-app-'`,
   then remove each with `docker rm --force <name>`, `docker rmi <name>:candidate`,
   `docker volume rm <name>` and `docker network rm <name>`.

   Result: The four commands no longer list them.

> **CAUTION:** THE NEXT STEP DELETES FOLDERS. Make sure that each path is a
> fixture copy under `<home>/smoke` and not a real repository.

2. Delete `<home>/smoke/ci-app`, `<home>/smoke/gui-app`, `<home>/smoke/plain-app`,
   `<home>/smoke/org-app` and `<home>/smoke/rust-app`, and the `docker` test double
   of S15.

## 9. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
