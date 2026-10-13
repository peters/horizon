---
procedure: cloud-browser-uploads
feature: Local files into the pages of cloud browsers
platforms: [linux]
cost: none
destructive: no
secrets: none
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
- The control service relays uploads only when the variable
  `HORIZON_CLOUD_BROWSER_UPLOADS` has the value `1`. U01 and U02 set it.
- This procedure does not test these functions:
  - The picker and the drops of local browser panels. The
    [browser file upload procedure](browser-file-upload.md) tests them.
  - Remote devices of a provider, for example BrowserStack. They get no
    uploads.

## 3. Safety

> **CAUTION:** USE ONLY SYNTHETIC FILES. The control service keeps the staged
> files until the browser closes, and the agents of a worker can read them.

## 4. Equipment and preconditions

- The candidate commit.
- Google Chrome or Chromium that is not a snap package. A snap Chromium cannot
  read the staged files.
- For U01: root or passwordless `sudo`, `setpriv` and `unshare`. Chrome must be
  on `/usr/local/bin:/usr/bin:/bin`.
- For U02: `bwrap`, and a directory `<chrome-bin>` that contains a
  `google-chrome` command.
- No other process listens on `127.0.0.1:47280`.

## 5. Setup

1. Build the worker binary of the candidate.

   ```sh
   cargo build -p horizon-cloud-worker
   ```

   Result: `target/debug/horizon-cloud-worker` exists. Record its SHA-256.

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

1. Start the control service in a private root that has a `/workspace`
   directory and no locale variables.

   ```sh
   state=$(mktemp -d)
   mkdir -p "$state/workspace" "$state/home"
   printf '{"agents":["claude"],"browsers":["chromium"]}' > "$state/workspace/capabilities.json"
   env -i PATH="<chrome-bin>:/usr/bin:/bin" HOME="$state/home" \
     HORIZON_CLOUD_BROWSER_UPLOADS=1 bwrap --tmpfs / \
     --dev-bind /usr /usr --dev-bind /etc /etc --dev-bind /home /home --dev-bind /var /var \
     --dev-bind "$state" "$state" --proc /proc --dev /dev --tmpfs /tmp \
     --symlink usr/lib /lib --symlink usr/lib64 /lib64 --symlink usr/bin /bin \
     --bind "$state/workspace" /workspace --die-with-parent --unshare-pid \
     "$PWD/target/debug/horizon-cloud-worker" serve &
   ```

   Result: The control service answers on `127.0.0.1:47280`.

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
   find "$state/home/.horizon/cloud-browser-history/.uploads" -mindepth 1
   ```

   Result: The output is empty, because the driver closed its browser.

## 7. Pass criteria

- U01 passes and is not skipped.
- U02 shows `RELAY PASS` and leaves no staged files.

## 8. Cleanup

1. Stop the control service of U02.

   ```sh
   kill %1
   ```

   Result: The control service and its browsers stop.

2. Remove the state directory of U02.

   ```sh
   rm -rf "$state"
   ```

   Result: The directory is gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request.
