# Release Flow

Horizon releases are tag-driven.

- `vX.Y.Z-alpha.N` and `vX.Y.Z-beta.N` are prereleases.
- `vX.Y.Z` is a stable release.
- The Git tag is the source identity. Saving a draft GitHub Release for one of those tags, or dispatching the Release workflow with an existing tag, builds the deliverables.
- The workflow uploads and verifies the required asset set while the GitHub Release is still a draft, then publishes it. A failed build therefore stays pending instead of advertising an empty public release.
- Interrupted uploads resume from the recorded tag commit and existing asset digests. Matching files are skipped; changed files for the same commit are replaced. The workflow never retags.
- Before publication, the workflow removes the four retired `horizon-installer-*` assets from a draft release.
  If removal fails, the release stays a draft. The workflow keeps other assets and assets on published releases.
- The same release workflow can also be started manually with an existing tag to recover a failed or incomplete release after fixing workflow automation, without bumping the version.
- Releases publish four executable assets and `SHA256SUMS.txt`.
- Stable releases update the `peters/homebrew-horizon` tap and open or update the WinGet PR for `Peters.Horizon`.
- Snap Store publication is currently paused.

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
8. **Save draft**. Do not click **Publish release**.

Saving the draft creates the Git tag if needed and starts the release workflow. If a release is published before the assets exist, the workflow converts it back to a draft, builds, uploads, and publishes only after the required set is present.

The release workflow validates:

- the tag format
- the GitHub prerelease checkbox matches the tag suffix when a GitHub Release already exists
- the tag's base version matches `Cargo.toml`
- the Git tag commit recorded on the draft

If the workflow itself needs a fix after a release was started, merge the workflow fix and run **GitHub Actions → Release → Run workflow** with the existing tag. The manual recovery path accepts a draft, an incomplete published release, or a tag with no GitHub Release yet, and it does not require a bumped version tag. The manual form's `publish_snap` input is currently disabled and has no effect while Snap Store publication is paused.

Then it:

- rewrites the workspace version to the exact tag version in CI
- builds the release binaries for Linux, macOS, and Windows
- uploads the release assets and `SHA256SUMS.txt` to the draft GitHub Release; it skips files with matching hashes
- publishes the GitHub Release only after the required asset set is present on that draft
- Snap Store publication is currently paused; the `publish-snap` job and its `publish_snap` manual input remain disabled
- in the canonical `peters/horizon` repo only, updates `peters/homebrew-horizon` so `brew install peters/horizon/horizon` tracks the latest stable release
- in the canonical `peters/horizon` repo only, updates the `Peters.Horizon` manifests in the configured `winget-pkgs` fork and opens or reuses the upstream PR against `microsoft/winget-pkgs`

## Update The Quick-Start Image

Quick start in **New cloud** uses the public base worker image
`ghcr.io/peters/horizon-worker-base`. Horizon pins this image by digest in
`crates/horizon-core/src/cloud_runtime/repository/launch/quick_start.rs`. Thus each
Horizon build starts the image that was tested with it. The `cpu` tag moves, and
Horizon does not use it.

The image changes only when the pin changes. Update the pin when the worker helpers,
the worker contract or the agent CLIs in the base image must change for a release:

1. Open the newest successful run of the **Worker images** workflow on `main`.
2. In the summary of the **Publish CPU base worker** job, find the image reference
   that has `@sha256:`.
3. In `quick_start.rs`, set the value of the `image!` macro to that reference.
4. In the log of the **Build and check** step of the same job, find the lines between
   `Worker check report for the quick-start capabilities:` and
   `End of the worker check report.`

   Result: Each line is one contract marker, for example
   `horizon-source-contract=1`. Do not copy the two boundary lines.
5. CAUTION: SET `CONTRACT` ONLY FROM THE REPORT OF THE NEW IMAGE. Horizon uses
   `CONTRACT` instead of a local check of the pinned image. A wrong value lets
   Horizon accept an image that the worker refuses when it starts.
6. In `quick_start.rs`, set `CONTRACT` to these marker lines.
7. Run `cargo test -p horizon-core quick_start`.

   Result: The tests pass. They make sure of these items:

   - The value is a digest of the public base image.
   - The base image recipe builds the capabilities of the built-in profile.
   - Each line of `CONTRACT` is a marker of the current worker check.
8. On a computer with Docker, run
   `cargo test -p horizon-core quick_start -- --ignored --nocapture`.

   Result: The test pulls the pinned image and runs its worker check. It prints the
   report and finds the same markers as `CONTRACT`.
9. On a computer with Docker, do lane C of the
   [worker GitHub chain procedure](testing/procedures/worker-github-chain.md)
   with the scripts of the commit that the image was published from (the
   `headSha` of the Worker images run of step 1).

   Result: In task C1, the GitHub service runs and the agent cannot see the
   isolation marker.
10. CAUTION: The next step rents compute from RunPod.
11. Do the [quick start test procedure](testing/procedures/cloud-quick-start.md)
   with the new pin. On the worker, examine `/run/horizon-worker/services.json`.

   Result: It lists `github` when the image has the GitHub service.
12. Merge the change in a normal PR before you save the draft release.

## CLI Alternative

If you prefer the CLI over the GitHub UI:

```bash
TAG="$(./scripts/next-version.sh alpha)"
gh release create "$TAG" \
  --target main \
  --title "$TAG" \
  --notes "Release $TAG" \
  --prerelease \
  --draft
```

`--target` accepts any branch, tag, or commit SHA. For beta or stable releases from a specific commit, replace `main` with the desired ref.

For a stable release, omit `--prerelease` and keep `--draft`. The workflow publishes the GitHub Release after the required assets are uploaded and verified.

To recover an interrupted tag without creating the GitHub Release by hand, run **GitHub Actions → Release → Run workflow** and pass that tag. The workflow creates the draft if needed.

## Stable Packaging Requirements

Stable-release packaging assumes:

- the release assets keep their current names:
  - `horizon-linux-x64.tar.gz`
  - `horizon-osx-arm64.tar.gz`
  - `horizon-osx-x64.tar.gz`
  - `horizon-windows-x64.exe`
- The workflow uploads the four executable assets before the tap update starts.
- `snap/snapcraft.yaml` remains available for the classic `horizon-ui` snap, but the release job is currently paused
- The Homebrew and WinGet jobs use the private `Horizon Release Automation` GitHub App with Contents write permission.
- Each job limits its token to `peters/homebrew-horizon` or `peters/winget-pkgs`, respectively.
- `SNAPCRAFT_STORE_CREDENTIALS` is not currently required while Snap Store publication is paused
- the protected `release-automation` environment contains `RELEASE_AUTOMATION_APP_ID` (variable) and `RELEASE_AUTOMATION_PRIVATE_KEY` (secret); permit the trusted `main` branch and release tags only
- `WINGET_PUBLIC_PR_TOKEN` in that environment is a separate expiring classic PAT with only `public_repo`, used only to query/create upstream WinGet PRs; it has no private-repository, workflow, or package scope
- `peters/winget-pkgs` exists as a fork of `microsoft/winget-pkgs`

After the credential migration, recover an older release by dispatching **Release from `main`** with the existing tag. Do not rerun a pre-migration workflow run: reruns retain the old workflow definition and its retired secret references.

Run **Verify Release Credentials** after rotating either credential. It checks protected-environment access, one-repository App token scope and the public PR token owner/scope/expiry without writing release content. The initial migration also verified disposable branch writes, a draft storage asset upload/download, a controlled public PR, cleanup and token revocation.

Each release job mints a short-lived installation token restricted to its one destination repository. The token action revokes installation tokens when the job ends. The App does not have workflow-write permission.

The upstream WinGet repository belongs to Microsoft. A token scoped only to the fork cannot create its upstream PR, so the dedicated public-only PAT remains an explicit exception. Rotate it before its recorded expiry. Do not reuse it for repository checkout, package access, or private repositories.

WinGet publication still depends on the normal `microsoft/winget-pkgs` review process after the PR opens, so catalog availability can lag behind the GitHub Release.

If a stable release is missing one of those assets, the release App credentials, the WinGet public PR token, or the WinGet fork, the release workflow fails instead of publishing a partial Homebrew or WinGet update.

## Release validation

Use the [release assets test procedure](testing/procedures/release-without-surge.md) before a release.
Horizon has no in-app updater. Users update through their package manager or replace the release executable.

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
