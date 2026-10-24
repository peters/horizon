---
procedure: local-image-preferences
feature: worker image destinations
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Local image destination preferences

## 1. Purpose

This procedure tests the machine-local destination metadata and transactional save helpers.
It tests scope separation, account identity, credential-free source naming and stale-window refusal.

## 2. Applicability

- Candidate: the destination preference prerequisite for #1597.
- Platforms: Linux and macOS with Git and the repository Rust toolchain.
- This procedure does not test: authentication, image publication, worker allocation or a UI.
- Image omission in YAML and automatic launches remain inactive in this prerequisite.

## 4. Equipment and preconditions

- Use a separate task checkout with a private build cache.
- The tests create synthetic settings and Git metadata in temporary directories.
- No provider account, real credential or paid resource is required.

## 5. Setup

1. Open the candidate checkout.

   Result: The working directory contains the candidate Rust sources.

## 6. Tasks

### 6.1 IP01 — Scope and metadata

1. Run `cargo test -p horizon-core cloud_runtime::publishing -- --test-threads=1`.

   Result: All tests pass. Personal choices stay within their account namespace and checkout/profile scope.

2. Examine the source-identity test results.

   Result: URL credentials do not change the destination or enter output events. Transports, hosts, ports, complete paths and local scopes distinguish sources. The Linux filesystem case refuses non-UTF-8 fallback paths. APFS refuses those directory names before a checkout can exist.

### 6.2 IP02 — Transactional save

1. Run `cargo test -p horizon-core cloud_runtime::setup::publishing -- --test-threads=1`.

   Result: All tests pass. First save and default restore work before a settings file exists.
   Later saves preserve unrelated settings and do not create repository files.

2. Examine the stale-window and changed-grant test results.

   Result: Another immutable account at the same URI and a changed publication grant both prevent a stale save.

3. Examine the cancellation test result.

   Result: Early cancellation creates no settings directory or lock. Settings inside the source repository are refused before a lock.

4. Examine the included-config origin fixtures and duplicate-scope save fixture.

   Result: An included origin matches its direct equivalent. Direct plus included
   origins and malformed included configuration are refused without remote output.
   Replacing or restoring a duplicated preference refuses the file without changing it.

## 7. Pass criteria

- Both commands pass on the candidate sources.
- No test uses a real credential or provider account.
- The metadata helpers do not activate launch or UI behavior.

## 8. Cleanup

1. Let the tests finish.

   Result: The tests remove their temporary directories.

## 9. Record of results

Record the candidate commit, commands, results and private log paths in the pull request.
