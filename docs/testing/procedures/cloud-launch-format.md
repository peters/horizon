---
procedure: cloud-launch-format
feature: retain reviewed image intent without changing existing launch behavior
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

This is a serialization prerequisite. All production constructors retain the
legacy `None` choice. Publisher envelopes remain refused until the engine can
consume them. No selector, publication, credentials or compute is activated.

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

### 6.2 LF-02 — Publisher fence

1. In the same tests, encode a reviewed personal publisher.

   Result: It uses a version-2 envelope. Both the previous reader and this staged
   prerequisite refuse it; neither silently falls back to the recipe image.

2. Test incomplete, unsupported, downgraded and mixed envelopes, reserved null
   keys and a recipe confirmation naming another image.

   Result: JSON and YAML refuse malformed intent before any launch can occur.
   Decode errors do not echo rejected private field values.

### 6.3 LF-03 — Runtime consumers

1. Run the full applicable local matrix, including the core and UI test tiers.

   Result: Existing constructors explicitly use None. Stored state, lifecycle,
   preview and UI tests retain existing behavior. There is no visible UI change.

## 7. Pass criteria

Applicable checks pass. Legacy encodings stay compatible and publisher intent
cannot be silently discarded. The later engine integration must enable envelope
reads and forward the retained choice atomically.

## 8. Cleanup

1. Retain logs; test-owned temporary files clean up normally.

   Result: User settings, accounts, running applications and workers stay unchanged.

## 9. Record of results

Record candidate, toolchain, commands, logs, new/reused results and CI in the PR.
No native GIF is needed because the change has no visible UI behavior.

Recipe/profile equality is enforced before serialization as well as during
deserialization. An inconsistent in-memory recipe emits no standalone launch
JSON, and JSON/YAML serialization fail before it can reach an older reader.
