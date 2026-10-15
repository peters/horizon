---
procedure: cloud-launch-format
feature: retain and forward reviewed image intent through saved launch consumers
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: synthetic-only
owner: peters
---

# Saved cloud launch format

## 1. Purpose

Prevent a saved image-publishing confirmation from silently becoming a launch
of the recipe's original image when read by an incompatible application.

## 2. Applicability

New cloud forwards its gated selector's confirmed choice. Other constructors keep
legacy `None`. Compatible consumers retain version-2 metadata and forward it to
the engine; automatic launches remain disabled for ordinary use. JSON/YAML checks run on all
listed platforms. Durable cloud-record UI preparation is supported on Unix only.
No provider account, publication or compute is activated by these tests.

## 3. Safety

Use synthetic serialization fixtures only. No credentials, provider access,
settings changes or live worker actions are required.

## 4. Equipment and preconditions

- The candidate checkout and repository Rust toolchain.
- Synthetic JSON/YAML launch fixtures; no provider account is needed.

## 5. Setup

1. Record the candidate and use this checkout's own Cargo target directory.

   Result: Test evidence identifies the source inputs being checked.

## 6. Tasks

### 6.1 LF-01 — Legacy compatibility

1. Run `cargo test -p horizon-core cloud_panel::launch_serde --lib`.

   Result: A launch with no reviewed choice retains its flat encoding. An explicit
   recipe confirmation stays flat, round-trips in JSON/YAML, and preserves the
   profile image for an older reader. Ordinary unknown legacy keys are tolerated.

### 6.2 LF-02 — Compatible publisher consumers

1. In the same tests, encode personal, automatic and registry publisher choices.

   Result: Each uses a version-2 envelope and round-trips intact in a compatible
   reader. Previous readers refuse it. No reader silently falls back to the recipe image.

2. Test incomplete, unsupported, downgraded and mixed envelopes, reserved null
   keys, a publisher choice without a build recipe, and a recipe confirmation naming another image.

   Result: JSON and YAML refuse malformed intent before any launch can occur.
   Decode errors do not echo rejected private field values.

3. Change either side of the in-memory recipe/profile image equality.

   Result: JSON/YAML serialization fails before an older reader can consume it.
   A standalone JSON writer emits no bytes. The same invariant holds on read.

### 6.3 LF-03 — Runtime consumers

1. Run the actual UI preparation regression.

   ```sh
   cargo test -p horizon-ui --bin horizon a_saved_reviewed_choice_cannot_be_replaced_by_a_new_local_preference_during_ui_preparation
   ```

   Result: A saved choice for the first registry cannot be replaced by the current
   preference for another, or silently retained after that preference is removed.
   Both stale cases refuse before a durable record is written. The UI forwards
   both the named profile and full choice.
   A legacy start without a publisher choice also retains its recipe image instead
   of adopting a changed preference. An automatic profile without an override
   prepares on its first Start without an empty recipe confirmation. Existing
   durable records stay authoritative.
   Refusal creates no deployment record, registry journal or Docker login.

   Run `cargo test -p horizon-ui --bin horizon disabled_publishing_cannot_create`
   and `cargo test -p horizon-ui --bin horizon legacy_automatic_intent_cannot`.

   Result: Registry, personal and automatic choices refuse with the setting off
   before creating a publisherless record, even with an explicit recipe image.
   An unconfirmed automatic launch refuses a later scoped override before any
   record. Explicit legacy images still prepare with the setting off. Settings
   bytes remain unchanged and no authentication or registry operation starts.

   Run `cargo test -p horizon-ui --bin horizon an_unbound_automatic_retry`.

   Result: A first automatic preparation with no override can save an unbound
   record. After an override is added, an unconfirmed retry refuses while keeping
   both the record and settings byte-identical. Already recorded publishers stay
   authoritative; no sign-in, build or allocation starts.

   Run `cargo test -p horizon-ui --bin horizon an_omitted_image_retry`.

   Result: An omitted-image launch can retry a legacy record that retains an
   explicit image, even after another local destination is saved. The old image
   remains authoritative and the record and settings stay byte-identical.

2. Run the actual UI rebuild-preview regression.

   ```sh
   cargo test -p horizon-ui --bin horizon desktop_rebuild_preview_keeps_the_retained_publisher_and_profile
   ```

   Result: The preview accepts the original recipe image with its retained destination.
   Asking for another profile refuses. It reads no credentials and starts no rebuild.

3. Run the full applicable local matrix, including the core and UI test tiers.

   Result: New cloud retains its reviewed choice. Other constructors use None, and
   legacy stored state, lifecycle and preview tests retain their behavior. The
   selector remains behind the staging gate.

## 7. Pass criteria

Applicable checks pass. Legacy encodings stay compatible and publisher intent
cannot be silently discarded. Envelope reads, actual preparation forwarding and
record-aware rebuild preview must be delivered atomically. Metadata never grants
publication authority; the shared engine still validates the machine-local choice.

## 8. Cleanup

1. Retain logs; test-owned temporary files clean up normally.

   Result: User settings, accounts, running applications and workers stay unchanged.

## 9. Record of results

Record candidate, toolchain, commands, logs, new/reused results and CI in the PR.
No native GIF is needed because the change has no visible UI behavior.
