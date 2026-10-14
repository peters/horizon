---
procedure: cloud-uncommitted-build-context
feature: Uncommitted files of an image build context (repository::uncommitted)
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Uncommitted build context test procedure

## 1. Purpose

This procedure makes sure that the uncommitted files of an image recipe go into
the build context only as the
[trust boundary](../../../.github/instructions/rebuild-uncommitted.instructions.md)
permits. It tests the library alone. The rebuild that uses it has its own task
(L14) in the [lifecycle procedure](cloud-panels/l-lifecycle.md) when that task
exists.

## 2. Applicability

- Candidate: a checkout of the branch under test.
- Platforms: Linux, macOS and Windows. The link and hard link tasks apply to
  Linux and macOS only.
- This procedure does not test: the rebuild confirmation, the image build, the
  push or a worker.

## 3. Safety

The tests make synthetic Git repositories in temporary folders only. They do
not use credentials, the network or a container registry.

## 4. Equipment and preconditions

- The Rust toolchain of the repository and Git.
- `TMPDIR` names a folder that is not in a Git repository.

## 5. Setup

1. In the checkout, build the tests of `horizon-core`.

   ```sh
   cargo test -p horizon-core --features cloud-workspaces --lib --no-run
   ```

   Result: The build completes without an error.

## 6. Tasks

### 6.1 U01: Which files go into the context

1. Run the tests of the module.

   ```sh
   cargo test -p horizon-core --features cloud-workspaces --lib -- cloud_runtime::repository::uncommitted
   ```

   Result: All tests pass. On Linux there are 14 tests or more.

2. Examine the names of the tests that passed.

   Result: Tests pass for each of these rules:
   - Modified, new and deleted files go in. Ignored files, also a file that was
     removed from the index and then ignored, do not go in.
   - Only files in the context folder and the Dockerfile count.
   - The hash changes when the content changes.
   - A file that changes after the confirmation is refused.
   - Common secret file names, unclear names and path parts that are not one
     plain name are refused.
   - Another configured work tree (`core.worktree`) is refused.

### 6.2 U02: Links (Linux and macOS)

1. Run the tests of the module that contain `unix`.

   ```sh
   cargo test -p horizon-core --features cloud-workspaces --lib -- cloud_runtime::repository::uncommitted::tests::unix
   ```

   Result: Three tests pass. A link goes in as a link, a folder replaced by a
   link is refused, and a hard link is refused.

## 7. Pass criteria

- U01 passes on each platform.
- U02 passes on Linux and macOS.

## 8. Cleanup

No cleanup is necessary. The tests delete their temporary folders.

## 9. Record of results

Put the results in the pull request. A report is not necessary.
