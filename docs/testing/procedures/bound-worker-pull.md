---
procedure: bound-worker-pull
feature: exact private image pull grants for CPU workers
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: synthetic fixtures only
owner: peters
---

# Bound worker pull test procedure

## 1. Purpose

This procedure checks exact registry bindings for CPU workers.
It checks that host user data receives the read-only material that verified the immutable image.

## 2. Applicability

- Candidate: The bound worker pull prerequisite of #1597.
- Platforms: Linux and macOS for provisioning fixtures; Windows for applicable credential checks.
- These tasks use synthetic inputs. They allocate no provider resource.
- This procedure does not test a deployed worker or automatic image selection.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain.
- A POSIX shell for the immutable image probe fixture on Linux and macOS.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

## 6. Tasks

### 6.1 BP-01 — Check exact bindings and provisioning guards

1. Run the CPU provider tests.

   ```sh
   cargo test -p horizon-core --lib cloud_runtime::deployment::hetzner -- --test-threads=1
   ```

   Result: An exact registry binding takes precedence over the legacy provider login.
   Missing verification, another immutable image or an elapsed known expiry stops before provider contact.
   Existing unbound and reconnect paths pass their regressions.

### 6.2 BP-02 — Check verified read-only material

1. Run the host login tests.

   ```sh
   cargo test -p horizon-core --lib cloud_runtime::registry::tests::host_login -- --test-threads=1
   ```

   Result: The probe verifies an immutable digest with a synthetic read-only login.
   A later credential-file change does not replace that login in host user data.
   The operation also retains its original declared expiry when local settings change.
   The user data contains no publishing credential.
   Expired or unconfirmed grants fail.
   Explicit default and zero-padded ports retain the image-facing Docker login key.
   Unsupported host spelling fails before provisioning.
   The temporary Docker directory has mode 0700 on Unix.

## 7. Pass criteria

- All applicable tasks pass.
- A bound allocation requires current verification of its exact immutable image.
- The verified read-only material stays separate from publishing material.
- No provider resource is created.

## 8. Cleanup

1. Let the tests remove their temporary directories.

   Result: The synthetic credentials and operation records are removed.

## 9. Record of results

Put the task results, candidate commit and local matrix evidence in the pull request.
Keep private evidence outside the repository.
