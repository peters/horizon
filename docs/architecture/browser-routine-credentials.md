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
- The fill operation's short-lived process buffers.

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
| Browser password manager | Backend-specific, not a testable routine contract, not origin-bound the same way across Chromium and Firefox. |
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
- user/target: the opaque slot UUID (not the routine display name, not an
  origin, not a username value)

One slot holds the opted-in fields for one routine. Username and password are
distinct fields inside that slot (or distinct `keyring` items keyed by
`slot` + field name). Plans store only `{ "type": "credential_field", "slot":
"<uuid>", "field": "username" | "password" }`.

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

`CredentialStore::fill_field` (name to be used by the crate) takes the slot,
field, approved origin, and target fingerprint. It returns success or a typed
error. It never returns the secret.

The only consumer is the browser engine fill path, after the claimed additive
MCP change described in [browser-routines.md](browser-routines.md) (fill by
slot instead of by `value`). Until that change exists, no production path may
copy a secret into `BrowserControlAction::Fill { value }` or into a plan
variable.

Keep secret material scoped to the fill operation. Clear reusable buffers
where the platform API allows it. Do not log, `Debug`-print, or include the
secret in `Display` / error types. Tracing fields use slot UUID and field
kind only.

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

Export may include: routine name, `routine_id`, origins, field kinds, slot
UUIDs, and `present` / `missing` markers. Export must not include secret
values, cookies, profile files, or raw keyring payloads.

## Test seams

The first credential code PR lands only:

- the `CredentialStore` trait
- a fake store used by unit tests
- origin-bound `fill_field` that returns success/failure
- locked-store behaviour
- export that emits field names and missing-secret markers only

The fake store returns success or failure, never a secret, including in test
names, fixtures, and assertion messages. Native Keychain, Credential Manager,
and Secret Service adapters are separate platform PRs after this ADR merges
and the `keyring` 4.2.0 (or then-current latest stable) dependency is added
with the feature set above.

## Invariants

- Default policy stores no username or password.
- Secrets exist only in the OS store and in the fill operation's ephemeral
  buffers.
- Page content cannot widen origins or choose a slot.
- Fill is rejected outside approved origins/frames or against an incompatible
  field fingerprint.
- Missing or locked storage degrades to handoff, never plaintext.
