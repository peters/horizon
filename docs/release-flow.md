# Release Flow

Horizon releases are tag-driven.

- `vX.Y.Z-alpha.N` and `vX.Y.Z-beta.N` are prereleases.
- `vX.Y.Z` is a stable release.
- Publishing a GitHub Release with one of those tags triggers the release workflow, which publishes to crates.io and uploads the platform binaries to the same GitHub Release.

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

Then it:

- rewrites the workspace version to the exact tag version in CI
- builds the release binaries for Linux, macOS, and Windows
- uploads those assets to the GitHub Release you just published

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
