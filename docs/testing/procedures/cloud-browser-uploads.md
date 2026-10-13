---
procedure: cloud-browser-uploads
feature: Local files into the pages of cloud browsers
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [Hetzner API token file, SSH identity of the cloud settings, ghcr.io publishing login]
owner: peters
---

# Cloud browser uploads test procedure

## 1. Purpose

This procedure makes sure that a page in a cloud browser gets the local files
that a person gives to it. It also makes sure of these conditions:

- The control service relays the page file chooser only to a host that sends
  uploads. It cancels the chooser when no such host polls.
- The page gets each file with the same name, size and SHA-256 as the local
  file, also for a name that is not ASCII.
- The control service attaches only complete files that it staged itself.
- The control service removes the staged files of a browser when the browser
  closes.
- In Horizon, the upload button of a page in a cloud browser opens the Horizon
  file picker, and a file drop on the page sends the files. The panel shows
  the progress. A folder or a failed transfer gives an error and sends nothing.

## 2. Applicability

- Candidate: a build that includes the upload relay of
  [issue #1506](https://github.com/peters/horizon/issues/1506). The worker
  binary `horizon-cloud-worker` comes from the same commit.
- Platforms: Linux.
- The protocol is in `crates/horizon-browser-protocol/src/cloud_view/uploads.rs`.
  The section "Browser uploads" of `examples/cloud-worker/README.md` tells how
  the control service works.
- The driver `cloud-browser-uploads/relay.py` speaks the protocol as a host
  does. It needs no Horizon window.
- U03 to U06 use a real cloud on Hetzner. The image of the cloud is the
  quick-start base image with the `horizon-cloud-worker` of the candidate.
- This procedure does not test these functions:
  - The picker and the drops of local browser panels. The
    [browser file upload procedure](browser-file-upload.md) tests them.
  - Remote devices of a provider, for example BrowserStack. They get no
    uploads.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC FILES. The control service keeps the staged
> files until the browser closes, and the agents of a worker can read them.

> **CAUTION:** U03 TO U06 RENT A HETZNER CX23 SERVER AND PUSH A TEST IMAGE TO
> GHCR.IO. Delete the cloud and the image versions in the cleanup, also after a
> failure.

> **CAUTION:** THE FIXTURE GETS A COPY OF THE SSH IDENTITY AND THE HETZNER
> TOKEN. Keep the seed directory private (mode `0700`) and delete the copies in
> the cleanup.

## 4. Equipment and preconditions

- The candidate commit.
- Google Chrome or Chromium that is not a snap package. A snap Chromium cannot
  read the staged files.
- For U01: root or passwordless `sudo`, `setpriv` and `unshare`. Chrome must be
  on `/usr/local/bin:/usr/bin:/bin`.
- For U02: `bwrap`, and a directory `<chrome-bin>` that contains a
  `google-chrome` command.
- No other process listens on `127.0.0.1:47280`.
- For U03 to U06:
  - Docker, and Horizon cloud settings, `<settings>`, with a Hetzner section
    and a ghcr.io publishing login. The Hetzner token is in the file that the
    settings name.
  - The [local device smoke fixture](../../../scripts/device-smoke/README.md)
    with `--native-view`, its tools root `<tools>` with `x11vnc`, and a Device
    panel in your Horizon.
  - Python 3 with GTK 3, `xdotool` and ImageMagick.
  - `docs/testing/procedures/cloud-file-drop/drag.py` of the cloud file drop
    procedure (pull request #1530), which drags files as a file manager does.
  - New private directories outside the home directory: `<smoke>` for the
    repository and the seed, and `<state>` for the fixture.

## 5. Setup

1. Build the worker binary of the candidate.

   ```sh
   cargo build -p horizon-cloud-worker
   ```

   Result: `target/debug/horizon-cloud-worker` exists. Record its SHA-256.

2. For U03 to U06, build the worker binary for the image in Debian 12, so it
   runs on the glibc of the base image.

   ```sh
   docker run --rm -v "$PWD:/src:ro" -v <smoke>/target:/t -e CARGO_TARGET_DIR=/t \
     rust:1.99.0-slim-bookworm sh -c 'apt-get update -qq && apt-get install -y -qq \
     build-essential pkg-config libssl-dev libxkbcommon-dev >/dev/null && cd /src && \
     cargo build --locked --release -p horizon-cloud-worker && \
     strip -o /t/horizon-cloud-worker.stripped /t/release/horizon-cloud-worker'
   ```

   Result: `<smoke>/target/horizon-cloud-worker.stripped` exists. Record its
   SHA-256.

3. Make a repository `<smoke>/repo` with one commit. Put the stripped binary in
   `.horizon/horizon-cloud-worker`, and add `.horizon/Dockerfile` and
   `.horizon/cloud.yml`.

   ```dockerfile
   FROM <the quick-start image of crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs>
   ADD https://httpbin.org/uuid /usr/local/share/horizon-smoke/build-nonce
   COPY horizon-cloud-worker /usr/local/bin/horizon-cloud-worker
   ```

   Give `cloud.yml` one profile `uploads`: the values of the built-in
   quick-start profile, `image: ghcr.io/peters/horizon-development`, and
   `build: {context: .horizon, dockerfile: .horizon/Dockerfile, platform: linux/amd64}`.

   Result: `git -C <smoke>/repo log` shows one commit.

   > **CAUTION:** THE NEXT STEP RENTS A SERVER AND PUSHES AN IMAGE.

4. Deploy the cloud on a Hetzner cx23 with a new cloud ID `<id>`, for example
   `u1506-` and six hexadecimal digits.

   ```sh
   printf '%s\n' '{"provider":"Hetzner","id":"cx23","kind":"cpu","vcpu":2,"memory_gb":4,"gpu_memory_gb":null,"location":"fsn1"}' > <smoke>/choice.json
   cargo build -p horizon-core --example cloud_deploy
   target/debug/examples/cloud_deploy deploy <settings> <smoke>/repo uploads \
     <smoke>/seed/cloud/<id> <id> --worker-choice <smoke>/choice.json
   ```

   Result: The output ends with the stage times of a ready cloud. If Hetzner
   reports `error during placement`, delete the cloud (cleanup step 3) and use
   another location.

5. Make sure that the worker runs the candidate binary. Use the address in
   `<smoke>/seed/cloud/<id>/deployment.json`, the host key file in the same
   directory and the SSH identity of `<settings>`.

   ```sh
   ssh <options of the cloud> 'sha256sum /usr/local/bin/horizon-cloud-worker'
   ```

   Result: The value is the SHA-256 of step 2.

6. Serve `cloud-browser-uploads/page.html` on the worker loopback.

    ```sh
    ssh <options of the cloud> 'mkdir -p /tmp/upload-demo && cat > /tmp/upload-demo/index.html && cd /tmp/upload-demo && (setsid nohup python3 -m http.server 47291 --bind 127.0.0.1 >/dev/null 2>&1 </dev/null &)' \
      < docs/testing/procedures/cloud-browser-uploads/page.html
    ```

    Result: The worker serves the page on `127.0.0.1:47291`.

   > **CAUTION:** THE NEXT STEP COPIES A SECRET. Keep `<smoke>/seed` private
   > and delete the copies in the cleanup.

7. Copy the seed into `<smoke>/seed`: `seed.py`, `horizon-seeded`, the frozen
    candidate as `horizon`, `cloud-id` with `<id>`, the SSH identity as
    `identity` and the Hetzner token file as `hetzner-token` (mode `0600`).

    Result: `<smoke>/seed` contains the six items and the `cloud` directory.

8. Start the fixture with the wrapper, without `TMPDIR`.

    ```sh
    env -u TMPDIR python3 scripts/device-smoke/serve.py --horizon <smoke>/seed/horizon-seeded \
      --tools <tools> --native-view --state <state>
    ```

    Result: The fixture output shows a `display` and a `vnc_address`. In the
    Device panel, the cloud `upload-demo` shows **Ready**. The SHA-256 of
    `/proc/<pid>/exe` of the fixture Horizon is the value of the frozen
    candidate.

9. Ctrl+double-click inside the cloud, then click **Browser** in the list.
    Enter `http://127.0.0.1:47291/` in the address field of the new panel.

    Result: The panel shows **Upload demo page (on the cloud worker)**. The
    panel shows no warning about manual upload selection.

10. Make synthetic files in `<state>/data/home/upload-files`, for example
    `O'Brien notes.txt`, `screen shot.png`, `Rapport ñ photo.jpeg` and a random
    40 MB file. Make two files to drop in another directory `<drop>`. Record
    the SHA-256 of each file.

    Result: The evidence has the values.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 U01: Relay as the agent user

1. In `examples/cloud-worker`, run the upload test of the browser lane.

   ```sh
   HORIZON_TEST_CLOUD_WORKER="$PWD/../../target/debug/horizon-cloud-worker" \
     HORIZON_TEST_BROWSER=/bin/true python3 -B -m unittest \
     test_browser_lane.BrowserLaneTests.test_a_page_receives_the_files_that_a_host_uploads_through_the_cloud_view
   ```

   Result: The test passes and does not show `skipped`. The control service
   and the browser run as UID 10001, as on a worker. The CI step **Cloud worker
   scripts** runs the same test.

### 6.2 U02: Relay without a locale

1. In the repository root, start the control service in a private root that
   has a `/workspace` directory and no locale variables. The host directories
   are read-only for the service. Only `$state` is writable.

   ```sh
   state=$(mktemp -d /var/tmp/cloud-uploads.XXXXXX)
   mkdir -p "$state/workspace" "$state/home"
   printf '{"agents":["claude"],"browsers":["chromium"]}' > "$state/workspace/capabilities.json"
   env -i PATH="<chrome-bin>:/usr/bin:/bin" HOME="$state/home" bwrap --tmpfs / \
     --ro-bind /usr /usr --ro-bind /etc /etc --ro-bind /home /home --ro-bind /var /var \
     --bind "$state" "$state" --proc /proc --dev /dev --tmpfs /tmp \
     --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin \
     --bind "$state/workspace" /workspace --die-with-parent --unshare-pid \
     "$PWD/target/debug/horizon-cloud-worker" serve &
   service=$!
   ```

   Result: The control service answers on `127.0.0.1:47280`. `$service` is
   the PID of its `bwrap`.

2. Run the driver.

   ```sh
   python3 docs/testing/procedures/cloud-browser-uploads/relay.py
   ```

   Result: The driver shows each line below, then `RELAY PASS`:

   - `older host: the worker cancelled the chooser at once`
   - `page received: names, sizes and SHA-256 agree`. One name contains `ñ`
     and an apostrophe.
   - `drop received: name, size and SHA-256 agree`
   - `host left: the worker cancelled the chooser after 10 s`. A value from
     10 s to 12 s is correct.

3. Examine the staged files.

   ```sh
   find "$state/home/.horizon/cloud-browser-history/.uploads" -mindepth 1 2>/dev/null
   ```

   Result: The output is empty, because the driver closed its browser.

### 6.3 U03 — Relay on a real worker

1. Run the driver over SSH.

   ```sh
   python3 docs/testing/procedures/cloud-browser-uploads/relay.py --ssh "ssh <options of the cloud>"
   ```

   Result: The driver shows the lines of U02, then `RELAY PASS`.

### 6.4 U04 — Pick files in a cloud browser

1. In the cloud browser panel, click **Choose files…**.

   Result: The Horizon picker **Choose files to upload** opens. It shows
   `Upload to http://127.0.0.1:47291`.

2. Open `upload-files`, select the four files and click **Upload**.

   Result: The button shows **Uploading…**. Above the page, the panel shows
   `Uploading files to the cloud browser…` with a percentage that increases.

3. Wait until the picker closes.

   Result: The page shows `picked: 4 file(s) received`. Each name, size and
   SHA-256 prefix agrees with the evidence.

### 6.5 U05 — Drop files on a cloud browser

1. In `<drop>`, start `drag.py` on the display of the fixture with the two files.

   Result: A white window with the two names shows in the fixture.

2. Drag from the window to **Drop files here** on the page.

   Result: The page shows `dropped: 2 file(s) received`. Each name, size and
   SHA-256 prefix agrees with the evidence.

### 6.6 U06 — Cancel and refuse

1. Click **Choose files…**, then click **Cancel** in the picker.

   Result: The page shows `Selection cancelled`.

2. Drag a file and a folder to the page.

   Result: The panel shows `File drop failed: … is a folder. Drop the files in
   the folder, or an archive of it.` The page gets no files.

3. On the worker, examine the staged files.

   ```sh
   ssh <options of the cloud> 'cd /workspace/home/.horizon/cloud-browser-history/.uploads && find . -type f -exec stat -c "%a %U %n" {} + && find . -type f -exec sha256sum {} +'
   ```

   Result: Each file has mode `600` and the owner of the agents. Each SHA-256
   agrees with the evidence.

## 7. Pass criteria

- U01 passes and is not skipped.
- U02 and U03 show `RELAY PASS`, and U02 leaves no staged files.
- U04 to U06 show the results above on the frozen candidate.

## 8. Cleanup

1. Stop the control service of U02.

   ```sh
   kill "$service"
   ```

   Result: The control service and its browsers stop.

2. Remove the state directory of U02.

   > **CAUTION:** THIS STEP DELETES A DIRECTORY. Make sure that `$state` is the
   > directory that `mktemp -d /var/tmp/cloud-uploads.XXXXXX` made in U02
   > before you delete it.

   ```sh
   rm -rf "$state"
   ```

   Result: The directory is gone.

3. Stop the fixture with SIGINT on the exact PID of its `serve.py`, then
   delete the cloud.

   > **CAUTION:** THIS STEP DELETES THE CLOUD, ITS VOLUME AND ITS SSH KEY.

   ```sh
   target/debug/examples/cloud_deploy delete <settings> <smoke>/seed/cloud/<id>
   ```

   Result: The Hetzner API lists no server, volume or SSH key of `<id>`.

4. Delete the image versions with the tag `horizon-<id>` of each deployment.

   > **CAUTION:** DELETE ONLY THE VERSIONS THAT THIS RUN PUSHED.

   ```sh
   gh api --paginate "/users/peters/packages/container/horizon-development/versions?per_page=100" \
     --jq '.[] | select(any(.metadata.container.tags[]; . == "horizon-<id>")) | .id'
   gh api -X DELETE /user/packages/container/horizon-development/versions/<version-id>
   ```

   Result: The list shows no version with the tag.

5. Delete `<smoke>/seed/identity`, `<smoke>/seed/hetzner-token`, the local
   image tags of the run and `<state>`. The fixture home in `<state>/data` has
   copies of the identity and the token.

   > **CAUTION:** THIS STEP DELETES DIRECTORIES. Make sure that `<state>` is the
   > fixture directory of this run before you delete it.

   ```sh
   rm -f <smoke>/seed/identity <smoke>/seed/hetzner-token
   rm -rf <state>
   ```

   Result: No copy of the identity or the token stays.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request.
