# Surge Installer And Update Smoke Plan

Use this plan to validate the Surge-managed Horizon installer and update flow on macOS and Windows before relying on GitHub Releases, Homebrew, or WinGet publication.

## Goal

Prove four things on each target OS:

1. Horizon release artifacts can be packed into a Surge installer on that OS.
2. The generated installer can install Horizon into the normal per-user location.
3. The installed runtime manifest points at a working Surge backend.
4. A newer release can be promoted from `beta` to `stable`, then applied in place with `UpdateManager::download_and_apply`.

## Why Local Filesystem First

Use a local `filesystem` backend before any hosted smoke:

- it removes GitHub Release timing and auth from the first validation pass
- it lets you test `beta` and `stable` promotion on one machine
- it exercises the same package/index/update machinery that the hosted flow depends on

Horizon only shows its in-app update prompt for `github_releases` + `stable` managed installs. The local smoke below validates the installer and package-update plumbing directly, using the helper example at [crates/horizon-ui/examples/surge-update-smoke.rs](/home/peters/github/horizon-surge-stable/crates/horizon-ui/examples/surge-update-smoke.rs). After this passes, do one hosted GitHub Releases smoke for the UI prompt path.

## Shared Setup

Run these steps on the target OS you are validating.

1. Build Horizon once in release mode.
2. Build the Surge toolchain.
3. Create a temporary one-app Surge manifest that uses `provider: filesystem`.
4. Stage the same built Horizon binary twice as two smoke versions, for example `0.2.0-smoke.1` and `0.2.0-smoke.2`.
5. Pack both versions.
6. Push `0.2.0-smoke.1` to `beta`, then promote it to `stable`.
7. Install `0.2.0-smoke.1` with the generated installer.
8. Verify the installed tree and runtime manifest.
9. Push `0.2.0-smoke.2` to `beta` only and confirm the installed `stable` app still reports no update.
10. Promote `0.2.0-smoke.2` to `stable`, then apply it with the helper example.

Using the same Horizon binary for both smoke versions is acceptable here. The goal is to validate packaging, channel promotion, install layout, and update application, not binary differences.

## Temporary Manifest Template

Create a one-app manifest per target RID:

```yaml
schema: 1
storage:
  provider: filesystem
  bucket: <ABSOLUTE_STORE_PATH>

apps:
  - id: <APP_ID>
    name: Horizon
    main: <MAIN_EXE>
    installDirectory: horizon
    icon: assets/icons/icon-512.png
    channels: [beta, stable]
    shortcuts: [desktop, start_menu]
    installers: [online-gui]
    target:
      rid: <RID>
```

Values:

- Windows x64: `APP_ID=horizon-win-x64`, `MAIN_EXE=horizon.exe`, `RID=win-x64`
- macOS arm64: `APP_ID=horizon-osx-arm64`, `MAIN_EXE=horizon`, `RID=osx-arm64`
- macOS x64: `APP_ID=horizon-osx-x64`, `MAIN_EXE=horizon`, `RID=osx-x64`

## Windows Plan

### Prerequisites

- Windows 11 x64
- Rust stable `>= 1.88`
- Git Bash or another Bash environment for the repo scripts
- PowerShell 7 or Windows PowerShell for inspection steps

### Build And Pack

1. Build Horizon:

```bash
cargo build --release
```

2. Build Surge:

```bash
./scripts/build-surge-toolchain.sh --version v1.0.0-beta.1 --output-dir "$PWD/.tmp/surge-toolchain"
export PATH="$PWD/.tmp/surge-toolchain:$PATH"
```

3. Create a filesystem store, for example `C:/tmp/horizon-surge-store`, and write a temporary manifest for `win-x64`.

4. Stage both smoke versions with the same binary:

```bash
./scripts/stage-surge-artifacts.sh --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.1 --binary target/release/horizon.exe --main-exe horizon.exe
./scripts/stage-surge-artifacts.sh --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.2 --binary target/release/horizon.exe --main-exe horizon.exe
```

5. Pack both versions:

```bash
surge --manifest-path <SMOKE_MANIFEST> pack --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.1
surge --manifest-path <SMOKE_MANIFEST> pack --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.2
```

6. Publish version 1 to `beta`, then promote it to `stable`:

```bash
surge --manifest-path <SMOKE_MANIFEST> push --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.1 --channel beta
surge --manifest-path <SMOKE_MANIFEST> promote --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.1 --channel stable
```

### Install Smoke

1. Run the generated installer headless:

```powershell
.surge\installers\horizon-win-x64\win-x64\Setup-win-x64-horizon-win-x64-stable-online-gui.exe --headless
```

2. Verify the install tree:

- install root: `%LOCALAPPDATA%\horizon`
- active app dir: `%LOCALAPPDATA%\horizon\app`
- runtime manifest: `%LOCALAPPDATA%\horizon\app\.surge\runtime.yml`
- desktop shortcut: `%USERPROFILE%\Desktop\Horizon.lnk`
- start-menu shortcut: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Horizon.lnk`

3. Launch the installed `horizon.exe` once and confirm it stays running long enough to create a window.

### Update Smoke

1. Push version 2 to `beta` only:

```bash
surge --manifest-path <SMOKE_MANIFEST> push --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.2 --channel beta
```

2. Confirm no `stable` update is visible yet:

```bash
cargo run -p horizon-ui --example surge-update-smoke -- --app-exe "$LOCALAPPDATA/horizon/app/horizon.exe"
```

Expected: no update available.

3. Promote version 2 to `stable`:

```bash
surge --manifest-path <SMOKE_MANIFEST> promote --app-id horizon-win-x64 --rid win-x64 --version 0.2.0-smoke.2 --channel stable
```

4. Apply the update:

```bash
cargo run -p horizon-ui --example surge-update-smoke -- --apply --app-exe "$LOCALAPPDATA/horizon/app/horizon.exe"
```

5. Re-check the install tree:

- runtime manifest version becomes `0.2.0-smoke.2`
- previous snapshot exists as `%LOCALAPPDATA%\horizon\app-0.2.0-smoke.1`
- `%LOCALAPPDATA%\horizon\app\.surge-cache` exists
- relaunch from the installed binary still works

### Windows Hosted Follow-Up

After the local filesystem smoke is green, run the WinGet publication smoke with [scripts/run-winget-azure-smoke.sh](/home/peters/github/horizon-surge-stable/scripts/run-winget-azure-smoke.sh). That validates manifest rendering, WinGet install/upgrade/uninstall, and launch in a disposable Windows 11 VM. It does not replace the local Surge update smoke above.

### Hosted GitHub Releases Smoke

Use a separate public staging repo for this step, not `peters/horizon`.

1. Mirror the Horizon release workflow in the staging repo and point its `.surge/surge.yml` `bucket` to that staging repo.
2. Cut a stable release in the staging repo, install it on Windows from the generated Surge installer, and confirm `%LOCALAPPDATA%\\horizon\\app\\.surge\\runtime.yml` records the staging repo in `bucket:`.
3. Cut a second stable release in the same staging repo.
4. Launch the already-installed app and wait for the Horizon update prompt.
5. Click `Download Installer` and confirm the browser opens the staging repo URL, not production.

Success criteria:

- prompt appears for the second staged stable release
- opened URL matches `https://github.com/<staging-owner>/<staging-repo>/releases/download/v<version>/...`
- downloaded installer launches and upgrades the existing install

## macOS Plan

Run this once per architecture you ship, on native hardware or a matching VM:

- Apple Silicon for `osx-arm64`
- Intel for `osx-x64`

### Prerequisites

- macOS 14+ on the target architecture
- Xcode Command Line Tools
- Rust stable `>= 1.88`

### Build And Pack

1. Build Horizon:

```bash
cargo build --release
```

2. Build Surge:

```bash
./scripts/build-surge-toolchain.sh --version v1.0.0-beta.1 --output-dir "$PWD/.tmp/surge-toolchain"
export PATH="$PWD/.tmp/surge-toolchain:$PATH"
```

3. Create a filesystem store, for example `/tmp/horizon-surge-store`, and write a temporary manifest for the target RID.

4. Stage both smoke versions with the same binary:

```bash
./scripts/stage-surge-artifacts.sh --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.1 --binary target/release/horizon --main-exe horizon
./scripts/stage-surge-artifacts.sh --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.2 --binary target/release/horizon --main-exe horizon
```

5. Pack, push, and promote version 1:

```bash
surge --manifest-path <SMOKE_MANIFEST> pack --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.1
surge --manifest-path <SMOKE_MANIFEST> pack --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.2
surge --manifest-path <SMOKE_MANIFEST> push --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.1 --channel beta
surge --manifest-path <SMOKE_MANIFEST> promote --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.1 --channel stable
```

### Install Smoke

1. Run the generated installer headless:

```bash
chmod +x .surge/installers/<APP_ID>/<RID>/Setup-<RID>-<APP_ID>-stable-online-gui.bin
.surge/installers/<APP_ID>/<RID>/Setup-<RID>-<APP_ID>-stable-online-gui.bin --headless
```

2. Verify the install tree:

- install root: `~/Library/Application Support/horizon`
- active app dir: `~/Library/Application Support/horizon/app`
- runtime manifest: `~/Library/Application Support/horizon/app/.surge/runtime.yml`
- applications shortcut: `~/Applications/Horizon.app`
- desktop shortcut: `~/Desktop/Horizon.app`

3. Launch the installed Horizon binary once from `~/Library/Application Support/horizon/app/horizon`.

4. Capture:

```bash
mkdir -p /tmp/horizon-surge-smoke
screencapture -x /tmp/horizon-surge-smoke/install-launch.png
```

### Update Smoke

1. Push version 2 to `beta` only:

```bash
surge --manifest-path <SMOKE_MANIFEST> push --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.2 --channel beta
```

2. Confirm no `stable` update is visible yet:

```bash
cargo run -p horizon-ui --example surge-update-smoke -- --app-exe "$HOME/Library/Application Support/horizon/app/horizon"
```

Expected: no update available.

3. Promote version 2 to `stable`:

```bash
surge --manifest-path <SMOKE_MANIFEST> promote --app-id <APP_ID> --rid <RID> --version 0.2.0-smoke.2 --channel stable
```

4. Apply the update:

```bash
cargo run -p horizon-ui --example surge-update-smoke -- --apply --app-exe "$HOME/Library/Application Support/horizon/app/horizon"
```

5. Re-check the install tree:

- runtime manifest version becomes `0.2.0-smoke.2`
- previous snapshot exists as `~/Library/Application Support/horizon/app-0.2.0-smoke.1`
- relaunch from the installed binary still works

6. Capture:

```bash
screencapture -x /tmp/horizon-surge-smoke/post-update-launch.png
```

### Hosted GitHub Releases Smoke

Use a separate public staging repo for this step, not `peters/horizon`.

1. Mirror the Horizon release workflow in the staging repo and point its `.surge/surge.yml` `bucket` to that staging repo.
2. Cut a stable release in the staging repo, install it on macOS from the generated Surge installer, and confirm `~/Library/Application Support/horizon/app/.surge/runtime.yml` records the staging repo in `bucket:`.
3. Cut a second stable release in the same staging repo.
4. Launch the already-installed app and wait for the Horizon update prompt.
5. Click `Download Installer` and confirm the browser opens the staging repo URL, not production.

Success criteria:

- prompt appears for the second staged stable release
- opened URL matches `https://github.com/<staging-owner>/<staging-repo>/releases/download/v<version>/...`
- downloaded installer launches and upgrades the existing install

## Pass Criteria

The smoke is green only if all of these hold on the tested OS:

- both smoke versions pack successfully
- the generated installer runs successfully
- the installed tree contains `.surge/runtime.yml`
- the installed runtime manifest names the expected `filesystem` backend and channel
- the `stable` install ignores a newer `beta`-only release
- the helper example detects the newer release after promotion to `stable`
- `download_and_apply` completes without error
- the installed app still launches after update

## Failure Triage

Check these in order:

1. Installer creation failed:
   - verify the Surge toolchain directory contains `surge`, `surge-supervisor`, `surge-installer`, `surge-installer-ui`, and the native runtime library
2. Installer runs but no app appears under the install root:
   - inspect the installer's stderr output and the generated `installer.yml`
3. Install succeeds but update helper cannot find updates:
   - inspect `.surge/runtime.yml` for wrong provider/channel/bucket fields
   - inspect the filesystem store for `releases.zst` and package artifacts under the expected app scope
4. Update helper sees the update but apply fails:
   - inspect `.surge-staging`, `.surge-cache`, and the previous `app-<version>` snapshot
