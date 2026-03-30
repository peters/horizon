# Release Flow

Horizon releases are tag-driven.

- `vX.Y.Z-alpha.N` and `vX.Y.Z-beta.N` are prereleases.
- `vX.Y.Z` is a stable release.
- Publishing a GitHub Release with one of those tags triggers the release workflow, which uploads the platform binaries to the same GitHub Release.
- The same release workflow can also be started manually with an existing tag to recover a failed release after fixing workflow automation, without bumping the version.
- Stable releases also publish `SHA256SUMS.txt`, build Surge-managed GUI installers, publish Surge update packages to the dedicated `surge` GitHub Release tag in `peters/horizon-updates`, publish the classic `horizon-ui` snap to the Snap Store, update the `peters/homebrew-horizon` tap, and open or update the WinGet manifest PR for `Peters.Horizon`.

## Source Of Truth

The active release line lives in `Cargo.toml` under `[workspace.package].version`.

Examples:

- If `Cargo.toml` says `0.1.0`, the next release can be `v0.1.0-alpha.1`, `v0.1.0-beta.1`, or `v0.1.0`.
- After `v0.1.0` ships, bump `Cargo.toml` to the next line, such as `0.2.0`, in a normal PR before cutting more prereleases.

`scripts/check-version-sync.sh` validates that the workspace package version and the `horizon-core` workspace dependency version stay aligned.

## Pick The Next Tag

Use the helper script to suggest the next tag for the current release line:

```bash
./scripts/next-version.sh alpha
./scripts/next-version.sh beta
./scripts/next-version.sh stable
```

The script reads the base version from `Cargo.toml`, fetches tags from `origin` by default, and prints the next tag name.

Examples:

```bash
$ ./scripts/next-version.sh alpha
v0.1.0-alpha.3

$ ./scripts/next-version.sh stable
v0.1.0
```

If the stable tag for the current base version already exists, the script stops and tells you to bump `Cargo.toml` to the next release line first.

## Publish From GitHub Releases

1. Make sure the target commit is already on GitHub and has green CI.
2. Run `./scripts/next-version.sh <alpha|beta|stable>` locally.
3. Open **GitHub → Releases → Draft a new release**.
4. Create or select the suggested tag.
5. Choose the commit you want the tag to point at.
6. If the tag has an `-alpha.N` or `-beta.N` suffix, enable **Set as a pre-release**.
7. If the tag is plain `vX.Y.Z`, leave **Set as a pre-release** disabled.
8. Publish the release.

The release workflow validates:

- the tag format
- the GitHub prerelease checkbox matches the tag suffix
- the tag's base version matches `Cargo.toml`

If the workflow itself needs a fix after a release was already published, merge the workflow fix and run **GitHub Actions → Release → Run workflow** with the existing tag. The manual recovery path reuses the existing GitHub Release instead of requiring a bumped version tag. The manual form also lets you leave Snap publication disabled while recovering the rest of the stable release if the Snap Store side is not ready yet.

Then it:

- rewrites the workspace version to the exact tag version in CI
- builds the release binaries for Linux, macOS, and Windows
- for stable releases, stages Surge packages and GUI installers for the same platform matrix
- uploads the raw release assets, Surge installer assets, and `SHA256SUMS.txt` to the GitHub Release you just published
- for stable releases in `peters/horizon`, publishes Surge update metadata and package artifacts to the internal `surge` GitHub Release tag in `peters/horizon-updates` using the `stable` channel
- for stable releases in non-canonical staging repos, defaults Surge update storage to the current repository so hosted smoke can stay self-contained
- for stable releases, builds the classic `horizon-ui` snap from [`snap/snapcraft.yaml`](../snap/snapcraft.yaml) and releases it to the Snap Store `stable` channel
- in the canonical `peters/horizon` repo only, updates `peters/homebrew-horizon` so `brew install peters/horizon/horizon` tracks the latest stable release
- in the canonical `peters/horizon` repo only, updates the `Peters.Horizon` manifests in the configured `winget-pkgs` fork and opens or reuses the upstream PR against `microsoft/winget-pkgs`

## CLI Alternative

If you prefer the CLI over the GitHub UI:

```bash
TAG="$(./scripts/next-version.sh alpha)"
gh release create "$TAG" \
  --target main \
  --title "$TAG" \
  --notes "Release $TAG" \
  --prerelease
```

`--target` accepts any branch, tag, or commit SHA. For beta or stable releases from a specific commit, replace `main` with the desired ref.

For a stable release, omit `--prerelease`.

## Stable Packaging Requirements

Stable-release packaging assumes:

- the release assets keep their current names:
  - `horizon-linux-x64.tar.gz`
  - `horizon-osx-arm64.tar.gz`
  - `horizon-osx-x64.tar.gz`
  - `horizon-windows-x64.exe`
- stable releases also publish the Surge installer assets:
  - `horizon-installer-linux-x64.bin`
  - `horizon-installer-osx-arm64.bin`
  - `horizon-installer-osx-x64.bin`
  - `horizon-installer-win-x64.exe`
- the stable release uploads the four raw assets plus the four installer assets before the tap update runs
- the canonical `peters/horizon` repo publishes Surge storage to the dedicated GitHub Release tag `surge` in `peters/horizon-updates`
- `snap/snapcraft.yaml` continues to package Horizon as a classic snap named `horizon-ui`
- `SURGE_STORAGE_TOKEN` is configured in the `peters/horizon` repository secrets with write access to `peters/horizon-updates`
- `SNAPCRAFT_STORE_CREDENTIALS` is configured in the `peters/horizon` repository secrets with credentials exported for the `horizon-ui` snap
- `HOMEBREW_TAP_TOKEN` is configured in the `peters/horizon` repository secrets with write access to `peters/homebrew-horizon`
- `WINGET_PKGS_TOKEN` is configured in the `peters/horizon` repository secrets with write access to the `peters/winget-pkgs` fork
- `peters/winget-pkgs` exists as a fork of `microsoft/winget-pkgs`

WinGet publication still depends on the normal `microsoft/winget-pkgs` review process after the PR opens, so catalog availability can lag behind the GitHub Release.

If a stable release is missing one of those assets, the Surge storage token, the Snap Store credentials, the tap token secret, the WinGet token secret, or the WinGet fork, the release workflow fails instead of publishing a partial Snap, Homebrew, Surge, or WinGet update.

## Cross-Platform Installer And Update Smoke

Before trusting a changed Surge packaging/update flow, run the Windows + macOS local-filesystem smoke plan in [docs/testing/2026-03-24-surge-installer-update-smoke.md](docs/testing/2026-03-24-surge-installer-update-smoke.md). That plan validates:

- installer creation on the target OS
- headless installer execution into the normal user install root
- runtime manifest contents and shortcut creation
- beta-to-stable promotion behavior using a local filesystem backend
- package-based update application via `UpdateManager::download_and_apply`

The quickest supported local entrypoint is `./scripts/run-surge-filesystem-smoke.sh`. It now bakes in the path rules the local smoke exposed:

- `stable` must be the first channel in the temporary manifest so the installer targets the stable line
- `0.2.0-smoke.1` must be installed before `0.2.0-smoke.2` is packed, because later packs replace the stable installer artifact
- Horizon now ships `offline-gui` installers by default because the packaged app is small enough that a self-contained installer is the simpler release path
- `surge pack` and `surge push` need explicit artifact/package directories when the temporary manifest lives outside the repo root
- Horizon smoke builds default to `cargo build` debug binaries for speed; use `--profile release` only when you specifically need a release payload
- `./scripts/build-surge-toolchain.sh` reuses `.surge/toolchain-bin` when the requested Surge source ref and commit match the cached toolchain
- after `v1.0.0-beta.6`, the default released smoke path uses the tagged Surge source with no override flags
- when you override Surge for smoke, `./scripts/run-surge-filesystem-smoke.sh` patches `surge-core` through a local `file://` Git source at the exact checkout/commit instead of a raw crate path; that preserves Surge workspace dependency resolution on Windows
- after the headless installer returns, stop the installer-launched `--surge-first-run` Horizon process before the scripted launch/update checks continue; that keeps repeated Windows runs deterministic
- use `./scripts/run-surge-filesystem-smoke.sh --surge-path ../surge` to validate a local unmerged Surge checkout without recloning or retagging it

For a disposable Windows host from Linux or macOS, use `./scripts/run-surge-azure-smoke.sh`. It provisions a Windows 11 VM, installs Build Tools when needed, forces one autologon to create the desktop session, then launches the smoke through an interactive scheduled task. If you rerun it with the same `--resource-group` and `--vm-name`, it starts and reuses that VM instead of provisioning another one. The helper now streams the guest-side Bash smoke output live instead of buffering it until the whole Windows command exits, so use `smoke.stream.log` as the primary progress signal while the task is running. To validate an open Surge PR before merge, pass `--surge-repo-url https://github.com/fintermobilityas/surge.git --surge-commit-sha <sha>`.

Use the hosted WinGet smoke below only after the local filesystem path is green.

Hosted GitHub Releases smoke is intended to run from a separate public staging repo. In that setup:

- the staging repo still builds raw assets, Surge installers, and Surge release-index packages
- Homebrew and WinGet publication jobs are skipped automatically because they only run in `peters/horizon`
- the in-app prompt uses the managed install's GitHub repo metadata, so it opens installer downloads from the staging repo instead of production

## Interactive WinGet Smoke

For full install, upgrade, launch, and uninstall validation on a disposable Windows 11 VM, use `scripts/run-winget-azure-smoke.sh`.

The runner:

- creates a Windows 11 VM with `az`
- stages the local WinGet manifest renderer and smoke script onto the VM
- opens an RDP session so the smoke runs from a PowerShell window in the logged-in desktop session
- polls smoke status and collects the final logs
- deletes the Azure resource group by default when it exits

Example:

```bash
./scripts/run-winget-azure-smoke.sh \
  --install-version 0.1.1 \
  --install-sha 23fda14bc79aaca79e3a5fbd52c3501c11b4971d69b7a28f2f69bba94bd566e1 \
  --install-release-date 2026-03-21 \
  --upgrade-version 0.2.0 \
  --upgrade-sha b7c1632f077067106883302b6936e720998ab53a2f5331511306bff8280fe5d5 \
  --upgrade-release-date 2026-03-23
```

Host prerequisites:

- `az` authenticated for the target subscription
- `xfreerdp` available on `PATH`
- `xvfb-run` available when running headless without an existing `DISPLAY`
