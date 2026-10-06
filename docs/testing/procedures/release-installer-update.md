---
procedure: release-installer-update
feature: release installers and managed updates
platforms: [linux, windows, macos-arm64, macos-x64]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Release installer and update test procedure

## 1. Purpose

This procedure tests the executable identity in a new Surge installer.
It also tests local installation, channel selection, and an update with the Horizon runtime dependency.

## 2. Applicability

- Candidate: a change to release packaging or its Surge toolchain.
- Required lanes: Linux x64, Windows x64, macOS Apple Silicon, and macOS Intel.
- The procedure uses a local package store. It does not publish a release.
- A missing lane is HOLD. A build does not replace a run on the target operating system.

## 3. Safety

Use an isolated desktop and private application state for each lane.
On Linux, use the home-directory mask from the [device fixture](../../../scripts/device-smoke/README.md).
On Windows and macOS, use a dedicated account or disposable machine.

> **CAUTION:** USE ONLY THE INSTALL ROOT OF THIS RUN. The helper deletes this root and stops its applications.

The `--install-root` option sets the path that the helper examines.
It does not change the destination of the Surge installer.
Make sure this path matches the account's normal per-user install location.
Do not use a developer's existing installation.

## 4. Equipment and preconditions

- The exact candidate checkout with complete Git LFS assets.
- Rust and the build tools for the target operating system.
- A task-owned isolated desktop with a live view through a Horizon Device panel.
- A frozen candidate with its commit and SHA-256.
- A new private package store and install root.
- No provider credentials or real network endpoints in the fixture.

## 5. Setup

1. Record the tag from `SURGE_VERSION` in `.github/workflows/release.yml`.

   Result: The report identifies the packaging toolchain.

2. Record the `surge-core` tag from `crates/horizon-ui/Cargo.toml`.

   Result: The report identifies the application runtime dependency.

3. Build the candidate in the exact checkout.

   Result: The build succeeds with the recorded runtime dependency.

4. Freeze the candidate in a new task-owned directory.

   Result: The report records the candidate commit and SHA-256.

5. Build the update helper with the candidate's runtime dependency.

   ```sh
   cargo build -p horizon-ui --example surge-update-smoke
   ```

   Result: `target/debug/examples/surge-update-smoke` exists, with an `.exe` suffix on Windows.

6. Freeze the helper and set `smoke_update_helper` to its absolute path.

   Result: The report records the helper's SHA-256.

7. Start the isolated desktop for the selected lane.

   Result: The desktop has private application state and an explicit control target.

8. Start its live view through a task-owned Device panel.

   Result: Three timestamped inspections show displayed frames that advance during changes.

## 6. Tasks

### 6.1 INSTALL-01 — Local installation and update

1. Set the helper's `--rid` value for the selected lane.

   Result: The value is `linux-x64`, `win-x64`, `osx-arm64`, or `osx-x64`.

> **CAUTION:** USE ONLY THE PRIVATE STATE OF THIS RUN. The helper removes its earlier packages and installation.

2. Run the helper in the isolated desktop with the recorded toolchain and frozen candidate.

   ```sh
   ./scripts/run-surge-filesystem-smoke.sh \
     --rid "$smoke_rid" \
     --toolchain-version "$smoke_toolchain_tag" \
     --binary "$smoke_candidate" \
     --skip-build \
     --store-dir "$smoke_store" \
     --install-root "$smoke_install_root"
   ```

   Result: The helper installs `0.2.0-smoke.1` and starts the installed candidate.
   Result: A beta-only update stays hidden from the stable installation.
   Result: The helper promotes and applies `0.2.0-smoke.2`.
   Result: The installed candidate starts after the update.

3. Examine `app/.surge/runtime.yml` in the private install root.

   Result: `mainExe` is `horizon` on Linux and macOS, or `horizon.exe` on Windows.
   Result: The file identifies the local package store and `0.2.0-smoke.2`.

4. Examine the previous installation in the private install root.

   Result: The previous snapshot remains available at `app-0.2.0-smoke.1`.

### 6.2 INSTALL-02 — Executable identity regression

1. Copy the installed application into a second private fixture.

   Result: The fixture has its own runtime manifest and install root.

2. Remove `mainExe` from the copied runtime manifest.

   Result: The copied manifest represents the older installer output without a supervisor identity.

3. Run `surge-update-smoke` against the copied candidate without `--apply`.

   ```sh
   "$smoke_update_helper" --app-exe "$smoke_copy_root/app/$smoke_main_exe"
   ```

   Result: The check refuses the missing executable identity before it reads the release index.

4. Restore `mainExe` in the copied runtime manifest.

   Result: The value matches the candidate's relative executable path.

5. Repeat the check without `--apply`.

   ```sh
   "$smoke_update_helper" --app-exe "$smoke_copy_root/app/$smoke_main_exe"
   ```

   Result: The helper reports `no update available` for the latest stable version.

## 7. Pass criteria

- Every required lane completes INSTALL-01 and INSTALL-02.
- The generated runtime manifest contains the correct `mainExe`.
- The beta-only update stays hidden, and the stable update applies.
- The installed candidate starts before and after the update.
- Each application PID uses the frozen candidate's bytes.
- The report includes live-view evidence and the remaining platform limits.

## 8. Cleanup

1. Close the test application normally.

   Result: Only the test application's process exits.

2. Close the task-owned Device panel.

   Result: The live-view connection ends.

> **CAUTION:** DELETE ONLY THE STATE OF THIS RUN. Other state can contain a person's work.

3. Remove the private installation and package store.

   Result: The test paths are absent, and the control target expires.

## 9. Record of results

Use the [report template](../reports/TEMPLATE.md) when a run must be kept.
Otherwise, put the results in the pull request.
Record each lane as PASS, FAIL, or HOLD with its evidence.
Keep private paths, machine identifiers, and raw logs out of public reports.
