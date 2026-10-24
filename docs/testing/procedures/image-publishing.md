---
procedure: image-publishing
feature: worker image destinations
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Worker image destination test procedure

## 1. Purpose

This procedure tests the reusable destination policy for epic #1597.
It checks account separation, registry paths and retained publisher identities.

## 2. Applicability

- Candidate: The `horizon-cloud` destination policy prerequisite.
- Platforms: Linux and macOS.
- The automatic launch path is not active yet.
- This procedure does not test registry authentication, image publication or cloud allocation.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository's supported Rust toolchain.
- No account, registry credential or running Horizon is required.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

## 6. Tasks

### 6.1 IP-01 — Separate accounts and source identities

1. Run the destination tests.

   ```sh
   cargo test -p horizon-cloud image_publishing
   ```

   Result: The tests pass for separate accounts, source hosts, repository owners and profiles.

### 6.2 IP-02 — Reject malformed destinations

1. Examine the malformed-input cases in `crates/horizon-cloud/src/image_publishing/tests.rs`.

   Result: The tests reject credentials, tags, digests, invalid accounts, invalid references and invalid registry paths.

2. Examine the accepted registry-path cases.

   Result: The tests accept nested projects, explicit ports and valid image-name separators.

### 6.3 IP-03 — Retain the publisher identity

1. Examine the account and credential-reference tests.

   Result: A saved destination refuses another account or credential reference. Different letter case retains the same account.

2. Examine the serialized-destination test.

   Result: The JSON round trip retains the destination and publisher. It contains no token.

## 7. Pass criteria

- IP-01, IP-02 and IP-03 pass.
- Each personal destination stays in its publishing account's namespace.
- Source text cannot select an account or redirect the image to another registry.
- A retained destination rejects another publisher.

## 8. Cleanup

The procedure creates no external resource and changes no account or credential.
The normal Cargo build cache can remain for subsequent checks.

## 9. Record of results

Put the candidate commit, toolchain, command and IP task results in the pull request.
Keep private paths and account data out of public evidence.
