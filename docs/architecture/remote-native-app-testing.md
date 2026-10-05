# Remote native-app testing

Issue #1255 adds native app runs alongside [remote browser sessions](remote-browser-sessions.md).
The reference integration is a native iOS/Android app with an isolated synthetic backend on developer loopback.
Provider credentials remain machine-local. Project files never select credential stores, endpoints or raw capabilities.

## Delivery boundary

`horizon-app-testing` currently provides the validated contract, executable recipe model and native catalog resolution.
This foundation does not allocate devices, execute builds or establish tunnels. Upload, transport, lifecycle,
host/MCP integration, live panels and orchestration are separate implementation increments. A passing contract test
is not evidence that a real device ran an app.

## Project contract

Put exactly one top-level `remote-device-testing` object inside a YAML fence in the selected repository's `AGENTS.md`.
The [generated JSON schema](remote-device-testing.schema.json) describes the wire structure. Additional runtime
validation rejects unsafe paths, undeclared ports, credential-bearing launch-variable names and unsupported bounds.
Regenerate the schema with `cargo run -p horizon-app-testing --example project_schema`.

```yaml
remote-device-testing:
  version: 1
  provider: browserstack
  apps:
    ios:
      build: ["./scripts/build-ios-test.sh"]
      artifact: build/ios/App.ipa
      bundle_id: com.example.app
    android:
      build: ["./scripts/build-android-test.sh"]
      artifact: build/android/App.apk
      package: com.example.app
  launch_arguments:
    BASE_URL: "http://localhost:{tunnel.port.backend}"
  tunnel:
    ports: {backend: 8080}
  matrix:
    - {platform: ios, form: phone, os: latest}
    - {platform: ios, form: phone, os: latest-2}
    - {platform: ios, form: tablet, os: latest}
    - {platform: android, form: phone, os: latest}
  recipes: [docs/features/native-smoke.md]
  evidence: {video: true, screenshots: true, logs_on_failure: true}
  max_parallel: 2
```

Build/reset commands are argument arrays, executed from the selected repository, without implicit shell interpolation.
Use a checked-in script for multi-command setup. The run request authorizes executing that repository's declared
commands; a discovered contract alone does not execute anything. Optional `reset` is an argument array with the
same rules. Both app types are optional individually; every matrix entry must have a declared app.

Artifact and recipe paths are exact, relative paths: no traversal, absolute paths or globs. Existing ancestors and
the resulting file must remain within the canonical repository; symlink escapes, including dangling symlinks, fail.
Check paths before a build and again when opening its artifact. Build output is untrusted: provider upload must use
an already-open verified artifact, never follow a path again after validation.

Only `localhost`, `127.0.0.1` and `::1` launch URLs with declared ports are accepted. Port templates resolve from
the contract, not the process environment. A port declaration authorizes only that loopback service. Tunnel
implementations must enforce the port allowlist, avoid general network exposure and avoid combining restricted
flags with provider options that defeat those restrictions.

The provider field names a machine-configured account. Native discovery uses the App Automate catalog and account
quota, separately from browser discovery and browser quota. Discovery is read-only and reserves no device.

## Matrix resolution

`os` defaults to `latest`. `latest-N` means N **offered major OS generations** below the highest offered version matching
that entry's platform/form/model, not the Nth device row or minor version. Count distinct offered major versions rather
than subtracting numbers: an iOS catalog offering 27, 26, 18 and 17 resolves `latest-2` to 18. An exact numeric version matches
its prefix (`18` includes `18.1`; `18.1` includes `18.1.2`). Resolve the highest matching minor version, then the
lexicographically first model. Optional `device` restricts the model by case-insensitive exact match.

Missing entries fail the entire resolution before allocation. The resulting report records the exact model and OS
for every declared entry; catalog results are not proof of session hardware. After allocation, verify the device
against fresh provider session evidence before declaring it ready.

The first catalog adapter identifies tablets by iPad, Galaxy Tab, Pixel Tablet and Nexus 9/10 model names.
Revisit this classification when new tablet families appear; an unfamiliar family must not qualify tablet coverage.
The runtime must distinguish catalog offering, account entitlement, available capacity and actual allocation.

## Executable recipes

Each declared Markdown recipe contains human-readable context and one executable YAML fence. Pure prose cannot
produce a pass. Recipe authors use stable accessibility identifiers, shared across platforms; labels are useful for
debugging but depend on language. Short-lived refs are session observations, not persistent recipe selectors.

```yaml
device-recipe:
  version: 1
  id: native-smoke
  steps:
    - id: wait-home
      action: wait
      target: {by: identifier, value: home.title}
      state: visible
      timeout_millis: 10000
    - id: open-menu
      action: tap
      target: {by: identifier, value: menu.open}
    - id: menu-visible
      action: assert
      target: {by: identifier, value: menu.signIn}
      state: enabled
```

The optional `platforms` list restricts a recipe to iOS and/or Android; an excluded recipe is explicitly reported
as non-applicable, never passed. Supported actions are tap, long press, type, clear, swipe, scroll, bounded wait, assert,
back, home, rotate, launch, terminate, reset, deep link and screenshot. Target types are identifier, label, current
ref and coordinates. States are visible, hidden, enabled and disabled. Waits are bounded to 60 seconds, gesture
durations to 10 seconds, and IDs are unique within a recipe. A step's evidence records its actual action outcome;
not finding a target or not running a later step cannot become a pass.

## Lifecycle and credential boundaries

The shared runtime will own app references, session IDs and tunnel processes. MCP and CLI receive opaque handles;
provider tokens and credentials never enter outputs, failure strings or action audit. Builds and uploads are
content-hash identified. Upload caching must respect provider expiry and cleanup without deleting another run's
artifact reference. Durable operation IDs distinguish retries from uncertain mutations.

One run has a unique tunnel identifier. Completion, cancellation and expiry release owned sessions and tunnel
processes; crash reconciliation operates on exact recorded ownership, never account-wide termination. Provider
failures and release uncertainty remain visible in the report. A failure on one device does not cancel other devices.
Shared backend fixtures require per-device isolation or serial execution; parallel account slots do not make shared
mutable test data safe.

The run scheduler defaults to the available native account quota, bounded by the matrix size and a local ceiling
of 16; an explicit `max_parallel` can lower that ceiling. It must run concurrent sessions when capacity and isolated
test state allow, queue remaining entries, and retain independent progress, failures and cleanup for each device.
The reference integration supplies isolated synthetic backend state per concurrent device; it does not silently
reduce a requested concurrent matrix to serial testing.

Provider videos and logs are generated and retained on the provider. Horizon downloads private local evidence;
provider retention/deletion is recorded separately. Do not describe provider-generated evidence as local-only.
Screenshots and logs contain synthetic data exclusively. Entitlements removed by provider iOS re-signing, external
payment returns, push notifications and native sign-in providers require explicit evidence boundaries.

## Typed failure modes

| Code | Meaning |
| --- | --- |
| `device_contract_missing` | No contract fence in the selected project |
| `device_contract_invalid` | Unsupported version, fields, commands, IDs, ports or templates |
| `device_path_rejected` | Traversal, unsupported path or symlink escape |
| `device_recipe_invalid` | Missing executable steps, unsupported action or invalid bounds |
| `device_catalog_invalid` | Malformed or oversized provider catalog |
| `device_matrix_unavailable` | At least one declared entry has no exact offered match |
| `device_file_unavailable` | Required local project material cannot be read |

Transport, capacity, tunnel and session failures will add typed codes in their implementation increments.
Parser and provider response bodies must never be embedded in failure messages.
