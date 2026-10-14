---
procedure: image-publication-credentials
feature: image publication credential isolation
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Image publication credential test procedure

## 1. Purpose

This procedure checks account identity and temporary Docker logins for one publication operation.
The tests use synthetic tokens. They do not change an installed login.

## 2. Applicability

- Candidate: The host publication preparation helper for epic #1597.
- Platforms: Linux, macOS and Windows.
- This helper has no launch caller yet. Existing launches keep their behavior.
- This procedure does not test a live registry push, UI selection or cloud allocation.

## 4. Equipment and preconditions

- A candidate checkout and the supported Rust toolchain.
- No account, registry credential or running Horizon is required.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

## 6. Tasks

### 6.1 IC-01 — Check provider account identity

1. Run the provider identity tests.

   ```sh
   cargo test -p horizon-cloud github
   ```

   Result: A user has a nonzero provider ID. An answer with ID zero is refused.

### 6.2 IC-02 — Refuse another publisher

1. Run the host publication tests.

   ```sh
   cargo test -p horizon-core publication::tests --lib
   ```

   Result: A reused name with another user ID is refused. Different letter case retains the same account.

### 6.3 IC-03 — Keep operation logins separate

1. Examine the two-operation test result from IC-02.

   Result: Each operation has its own Docker configuration and token. The shared configuration stays unchanged.

2. Examine the cleanup assertions.

   Result: Dropping one operation removes only its temporary configuration. The other operation keeps its login.

## 7. Pass criteria

- IC-01 through IC-03 pass.
- Another account cannot replace the expected publisher.
- No token enters serialized account metadata or diagnostic output.
- An operation owns its temporary Docker login until it finishes.

## 8. Cleanup

The tests remove their temporary configurations. They create no external resource and change no installed credential.
The normal Cargo cache can remain.

## 9. Record of results

Put the candidate commit, toolchain, commands and IC task results in the pull request.
Keep private account data out of public evidence.
