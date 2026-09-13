# Browser routine credentials

Threat model and platform-store decision for optional username/password
storage on [#342](https://github.com/peters/horizon/issues/342). Read with
[browser-routines.md](browser-routines.md). This document does not add a
crate dependency and does not implement any store adapter.

The user-facing ability to opt in remains an acceptance criterion. This ADR
decides how secrets are stored and filled. Plaintext, a key-beside-ciphertext
file, the browser password manager, and MCP-visible secret values are not
acceptable alternatives.

## Assets

- Username and password bytes the user explicitly chose to remember for one
  routine.
- Opaque slot identifiers, approved origins, and field fingerprints (metadata,
  not secrets).
- Routine-owned browser profile cookies and site storage (session replay
  material, not OS-store secrets).
- The fill operation's short-lived process buffers, and after dispatch the
  browser driver / page as a secret recipient (not a Horizon artifact).

## Adversaries and non-goals

In scope:

- Malicious page content trying to widen `allowed_origins`, select another
  slot, or exfiltrate a secret through snapshots, evaluate, traces, or
  assertions.
- Another routine, panel, or Horizon session reading this routine's secrets or
  profile.
- Routine export/import, logs, panic formatting, MCP arguments, job reports,
  and screenshots.
- A locked or missing OS credential store.

Out of scope for this feature (fail closed, do not invent a store):

- Same-user filesystem or OS-account compromise after a secret is in the
  platform store. The OS store is the confidentiality boundary.
- MFA seeds, recovery codes, passkeys, security answers, payment-card data,
  and cookies. Those are never written to the OS store by this feature.
- Defending a malicious local owner who can already unlock the OS store.

## User choices

Credential persistence is an explicit prompt after a successful sign-in during
teaching. The default is **do not remember username or password**. Cookies may
still remain in the routine profile.

| Choice | Stored in the OS store | Plan reference |
| --- | --- | --- |
| Do not remember (default) | nothing | no slot |
| Remember username only | username field | `credential_field` + `username` |
| Remember username and password | both fields in one slot | `username` and `password` |

The user can inspect metadata (routine name, approved origins, field kinds,
update time), replace credentials, disable automatic fill, or delete the
record. Metadata APIs and UI never return secret bytes.

## Storage decision

Do not add a dependency in this documentation PR. The credential
implementation slice, after this ADR is merged, should add the latest stable
[`keyring`](https://crates.io/crates/keyring) crate (**4.2.0** as of
2026-08-29: MIT OR Apache-2.0, MSRV 1.88, below Horizon's 1.95).

Default `keyring` 4.2.0 platform backends:

| Platform | Backend | Feature |
| --- | --- | --- |
| macOS | Keychain | `apple-native-keyring-store/keychain` |
| Windows | Credential Manager | `windows-native-keyring-store` |
| Linux | Secret Service via zbus | `zbus-secret-service-keyring-store` |

Forbidden `keyring` features for this product: `db-keystore` and any other
file-backed or in-process fallback. Linux `keyutils` is not the required
Secret Service adapter and must not be the primary store.

Rejected alternatives:

| Option | Why not |
| --- | --- |
| Browser password manager | Backend-specific, not a testable routine contract, not origin-bound the same way across Chromium and Firefox. A backend is not eligible for routine login until its launch path disables the browser password manager and form autofill for that routine profile. |
| Encrypt a file next to `routine.json` | The key would live beside the ciphertext or in config. Forbidden by #342. |
| Direct `security-framework` + `windows-sys` + `secret-service` | Three stacks, more surface, no shared fake-store seam. Revisit only if `keyring` cannot meet the fill/delete/lock tests. |
| Environment variables or agent-visible config | Secrets would enter MCP, traces, and process listings. |

Binary cost is accepted and measured in the platform-adapter PR: Linux grows
by zbus/Secret Service, macOS by Security.framework, Windows by the Credential
Manager bindings. The first-tranche crate uses a `CredentialStore` trait and a
fake store so that cost is not paid until the adapter PR.

## Identity of a stored record

`keyring` entries are addressed by a service name and a user/target name. Use:

- service: `horizon-browser-routine`
- user: `<slot-uuid>:username` or `<slot-uuid>:password` (not the display
  name, not an origin, not a username value)

`keyring` 4.2.0 stores one secret per `(service, user)` pair. Horizon uses
**one native item per field**, never a bundled username+password payload:

- service: `horizon-browser-routine`
- user: `<slot-uuid>:<field>` where `<field>` is `username` or `password`

Rotation and deletion target that exact pair. Plans still store only
`{ "type": "credential_field", "slot": "<uuid>", "field": "username" | "password" }`.

No API lists secret bytes. List/show/export emit field names and
`missing` / `present` markers.

## Origin and frame binding

Every slot records the exact HTTPS origins (scheme + host + port) observed
for that login during teaching. `http://127.0.0.1` and `http://localhost` are
allowed only for local fixtures in tests; they are not a general HTTP
exception.

Before every fill:

1. Resolve the target in the current browsing context.
2. Read that context's origin. Do not inherit the top-level origin for a
   nested frame.
3. Reject if the origin is not an exact member of the slot's allowlist.
4. Reject if the navigation redirected to an origin that is not on the
   allowlist since the last user-approved login.
5. Reject if the field fingerprint is incompatible with the fingerprint
   recorded for that field.
6. Reject if page content supplied the slot id, origin, or field name. Those
   values come from the reviewed routine, never from the DOM.

A successful backend dispatch is not proof of fill success. The step
postcondition is.

## Fill contract

Two types, not one:

- `CredentialStore` is persistence. Its methods are `put`, `delete`,
  `contains`, `is_locked`, and `fill_into(routine_id, slot, field, sink)`.
  None of them return secret bytes. `fill_into` is the only path that copies
  stored bytes, and it copies them only into the provided `FillSink`.
- `CredentialBroker` owns origin/frame/fingerprint checks and is invoked
  only from a runner-held routine lease. Lookup key is `(routine_id, slot)`.
  After those checks it calls `CredentialStore::fill_into`. The broker's
  caller still receives only success or a typed error.

`FillSink` is implemented by the browser engine. Tests use a sink that
records success/failure and drops bytes. There is no API that returns a
`String` password to MCP, the compiler, or UI. Generic `browser_act` callers
cannot nominate a slot.

No production path may copy a secret into `BrowserControlAction::Fill { value }`
or into a plan variable.

Keep secret material scoped to the fill call. Clear reusable buffers where
the platform API allows it. Do not log, `Debug`-print, or include the secret
in `Display` / error types. Tracing fields use routine id, slot UUID, and
field kind only.

## Locked, missing, or unsupported store

If the platform store is missing, locked, or the adapter is not yet shipped
for this OS:

- Credential persistence is unavailable.
- Never write the secret to disk, to the routine directory, or to job state.
- An in-progress run that needs a credential becomes `needs_login` or
  `needs_user` and asks for visible handoff.
- Cookie replay from the routine profile still proceeds when the
  authenticated-state assertion holds.

There is no plaintext fallback and no "remember in this session's memory
across restart" cache.

## Rotation, deletion, and recovery

Rotation overwrites the OS-store item for that slot and field after an
explicit user action. The routine `plan_version` does not need to change when
only secret bytes change.

Deletion of a slot removes the OS-store items and the policy's `slot`
reference. Deletion of a routine offers to delete the slot and the routine
profile. Failure to delete an OS item is reported; it is not retried as a
side effect of an unrelated operation.

Interrupted writes must not leave a second live slot for the same routine.
The fake store and later native tests use disposable records and must clean
them up.

Restoring Horizon, duplicating a session, or copying `routine.json` does not
copy OS-store secrets. Import of an exported routine always requires the user
to bind or enter credentials again.

## Export

Export may include: routine name, origins, field kinds, and `present` /
`missing` markers. Export must not include secret values, cookies, profile
files, raw keyring payloads, or a live `routine_id` used as an overwrite
key. Import always allocates a new `routine_id` / `profile_id`, rewrites
internal bindings, and never replaces an existing routine directory. Collision
with an existing id is a reject, not a merge.

## Test seams

The first credential code PR lands only:

- the `CredentialStore` trait including `fill_into` (no secret-returning methods)
- the `CredentialBroker` / `FillSink` seam with a fake store
- origin-bound fill that returns success/failure
- locked-store behaviour
- export that emits field names and missing-secret markers only
- tests that a slot from another `routine_id` does not fill

The fake store returns success or failure, never a secret, including in test
names, fixtures, and assertion messages. Native Keychain, Credential Manager,
and Secret Service adapters are separate platform PRs after this ADR merges
and the `keyring` 4.2.0 (or then-current latest stable) dependency is added
with the feature set above.

## Secret locations after a successful fill

The OS store and the fill call's buffers are not the only places plaintext
can exist. Once the engine types into the page, the browser driver, the
renderer, the DOM, and possibly visible pixels (especially a username) also
hold it. The page is a secret recipient after fill; this feature cannot
prevent the site from seeing a password it just accepted.

Horizon-controlled capture must not copy that plaintext into routine
artifacts:

- The protected window starts at the first Teach-mode focus of a
  username/password field (manual typing, before the opt-in prompt) and at
  broker dispatch during replay. It lasts until a verified navigation leaves
  the filled document or an explicit field-clear postcondition holds.
  Horizon screenshots, `browser_video` / WebM capture, `browser_network`
  (including WebSocket payloads), CDP snapshots, `evaluate`, and MCP
  results that could read the field are stopped or blocked for that window.
  A capture started before the field was focused must be paused and must
  not restart until protection ends. Ending protection at fill postcondition
  alone is not enough while the value can still sit in the DOM.
- The existing redacted audit continues to store character counts, not
  values.
- Horizon still must not copy DOM values back into plans, drafts, traces,
  reports, or exports.

## Invariants

- Default policy stores no username or password.
- Horizon APIs never return secret bytes. Persistence is the OS store;
  in-process plaintext is limited to the broker-to-engine `FillSink` call and
  then to the page as a secret recipient.
- Page content cannot widen origins or choose a slot. Broker lookup is
  `(routine_id, slot)` from the reviewed routine, not from the DOM.
- Fill is rejected outside approved origins/frames or against an incompatible
  field fingerprint.
- Missing or locked storage degrades to handoff, never plaintext.
- Import clears slot bindings; a copied slot UUID cannot fill another
  routine's OS-store item.
