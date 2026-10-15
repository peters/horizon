---
procedure: portable-build-engine
feature: retain an authorized publisher through image preparation and retries
platforms: [linux, macos]
cost: none
destructive: no
secrets: owned synthetic registry grants only
owner: peters
---

# Portable build engine

## 1. Purpose

Check image omission, destination confirmation, retained identity and operation-private
credentials through the completed ordinary publishing flow.

## 2. Applicability

Use a supported Unix host and Docker with buildx. The development CLI exercises the
shared engine. This procedure creates no compute resource and changes no installed login.
Desktop preparation forwards the reviewed publisher metadata. UI, CLI and MCP
use the shared policy; no manual staging switch is required.

## 3. Safety

> **CAUTION:** USE ONLY OWNED REGISTRY ACCOUNTS AND PRIVATE FIXTURE SETTINGS.
> Do not use `deploy`, change installed credentials or send a synthetic compute key to a provider.

Keep temporary trust process-local. Keep all credentials and journals outside the source.
Record owned container, volume and image identities before creating resources.

## 4. Equipment and preconditions

- The candidate source and its supported toolchain.
- An owned loopback registry with two separate writer/reader pairs and repository scopes.
- A pinned public worker base that satisfies the current worker contract.
- Private fixture settings with both registry bindings and synthetic local compute metadata.

## 5. Setup

1. Build `cargo build -p horizon-core --example cloud_deploy`.

   Result: The binary comes from the candidate checkout. Record its hash.

2. Commit a fixture repository with a worker Dockerfile and this profile.

   ```yaml
   version: 1
   default: fixture
   profiles:
     fixture:
       provider: runpod
       cpu: 2
       memory_gb: 4
       build: {context: ., dockerfile: Dockerfile, platform: linux/amd64}
       capabilities: {agents: [], browsers: [], desktop: false}
   ```

   Result: The profile contains no publication account or image destination.

3. Use ordinary private fixture settings without a publishing feature switch.

   Result: The test exercises the ordinary default. Installed settings stay unchanged.
   Configure both exact registry grants as described in `docs/private-registry-bindings.md`.
   A fixture preference uses the canonical checkout, `fixture` profile and selected repository.
   Its `binding` is SHA-256 of the compact UTF-8 JSON array `[publish.username,publish.secret_file]`.
   This derives identity from file references, never token contents. It is test setup, not a user flow.

## 6. Tasks

### 6.1 PE-01 — Admission and retained authority

1. Run the focused engine and retained-grant tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib cloud_runtime::publishing
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib publishing_fence
   RUSTFLAGS="-D warnings" cargo test -p horizon-core --example cloud_deploy
   ```

   Result: Omission requires a build. Legacy explicit images retain their target.
   Changed accounts, grants and confirmed selections stop before image work.
   Pull rotation does not replace a publication grant. Existing image reuse needs no publisher login.
   CLI planning reads existing state without creating directories or taking an operation lock.
   A built image can be planned without a publication grant. Image-only preparation refuses
   a cloud with an existing worker image before changing its record, even when the
   request changes CPU or memory. Its normal rebuild flow authorizes any replacement.

### 6.2 PE-02 — Real source identity

1. Run the ignored read-only identity gate against a real checkout.

   ```sh
   HORIZON_TEST_REPOSITORY="$PWD" HORIZON_TEST_SOURCE="https://github.com/peters/horizon" \
     RUSTFLAGS="-D warnings" cargo test -p horizon-core --lib \
     cloud_runtime::publishing::tests::launch::source::real_repository_identity -- --ignored --exact
   ```

   Result: Its configured origin has that normalized source identity. If testing another
   checkout, supply its expected credential-free normalized origin. No source file changes.

### 6.3 PE-03 — Real image preparation and retry

> **CAUTION:** PUBLISH ONLY TO THE FIXTURE'S OWNED REPOSITORIES.
> Use dedicated grants. A successful read is not proof of issuer-wide read-only scope.

1. Select the first fixture destination and run its plan.

   ```sh
   target/debug/examples/cloud_deploy plan-deploy <settings> <source> fixture <state-a> fixture-a
   ```

   Result: The plan names the selected destination and prints its confirmation token.

2. Run `prepare-image` with the same arguments and `--plan-token <printed-token>`.

   Result: The real build passes the worker contract, pushes to the first repository,
   verifies its immutable digest with the separate reader, and retains its publisher.
   No provider registry binding or compute allocation is created by image-only preparation.

3. Select the second grant and repeat with `<state-b>` and `fixture-b`.

   Result: The same committed source publishes to the second authorized repository.
   Each reader can read only its repository and cannot write.

4. Keep the second local preference and repeat the plan and preparation for `<state-a>`.

   Result: The first cloud still uses its recorded destination and grant.
   The source YAML and installed Docker configuration stay unchanged.

5. Run the applicable full local matrix and independent reviews.

   Result: Blocking and strict checks pass. Record advisory results separately.
   Repeat the real source and registry checks on the final reviewable head.

### 6.4 PE-04 — Historical working-copy retry

1. Run the actual CLI regression.

   ```sh
   cargo test -p horizon-core --example cloud_deploy a_historical_image_retry_keeps_the_recorded_working_copy_fence
   ```

   Result: After HEAD advances outside the build context, a retry naming the original
   revision still enforces its recorded working-copy confirmation. Both an originally
   clean plan and a plan with edits refuse later unconfirmed Dockerfile content before
   a build or release lookup. The saved record remains unchanged.

2. In the owned registry fixture, prepare revision A with a confirmed Dockerfile label.
   Advance HEAD outside the build context, then retry revision A in the same state folder.

   Result: Both published images retain that label and the image label's revision/hash
   matches the saved working-copy confirmation. No compute is allocated.

### 6.5 PE-05 — Reconnect after removing a recipe image

1. Run the retained-publisher reconnect regression.

   ```sh
   cargo test -p horizon-core --lib a_rebuilt_explicit_recipe_can_omit_its_image_when_reconnecting_to_the_retained_publisher
   ```

   Result: A cloud created from an explicit recipe with a local destination override
   rebuilds from a recipe that omits `image`. Prepare, replacement reopen and sizing
   accept the omitted image, original recipe image and resolved image. A different
   explicit image refuses in all three paths; another named profile refuses during
   preparation. The publication, worker specification and record bytes stay unchanged,
   and no credential files are required or Docker login created.

### 6.6 PE-06 — Canonical temporary scope

1. Set `TMPDIR` to an owned symbolic link to a private temporary directory.
   Run `cargo test -p horizon-core --lib cloud_runtime::publishing::tests::launch`.

   Result: The positive registry fixtures store the canonical repository path, as
   the preference writer does. Explicit reviewed registry choices make a scope
   mismatch refuse before authentication, rather than opening a real sign-in.
   Both initial CPU admission and retained retry pass through the alias.

## 7. Pass criteria

All applicable tasks pass. Distinguish real local registry behavior, synthetic issuer
fixtures and hosted platform checks. No live-account or allocation proof is implied.

## 8. Cleanup

> **CAUTION:** REMOVE ONLY THE RESOURCES RECORDED FOR THIS FIXTURE.
> Do not use global container, image or volume cleanup.

1. Remove the owned containers, volume, image tags, temporary trust and credential files.

   Result: The resource ledger confirms their absence. Installed credentials stay unchanged.

## 9. Record of results

Record candidate, binary hash, input comparison, commands, outcomes, digests and cleanup.
Do not publish passwords, private keys, operational identifiers or signed upload URLs.

The rebuild regression also supplies another named profile with the same image
and with an omitted image. Both must refuse before using its different Dockerfile;
matching-profile recipe edits keep the recorded destination.

Desktop preparation now preserves saved publisher choices, and rebuild previews
use the cloud's retained publication and profile. New cloud reviews the publisher
before Start. Metadata forwarding is not authorization to publish; the separate
selector procedure qualifies its visible flow.

### Retained rebuild confirmation regression

Run the `rebuild_confirmation_cannot_cross_publication_grants_with_matching_images_and_files`
core test. Two plans with identical images, profiles and files but different grants
must produce different tokens. A stale or absent publication confirmation must
refuse against the locked record before credential reads, with unchanged record
bytes. `plan-rebuild` also shows the recorded destination and account. Record-backed
tests are Unix-only; pure naming and policy tests still run on Windows.

Run the actual CLI example regression
`a_clean_retained_rebuild_requires_its_printed_publisher_token`. A clean retained
cloud refuses a tokenless rebuild before credential checks. Its correct printed
token reaches the original grant check, without creating a login or changing state.
