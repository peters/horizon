# Browser routines

This document is the schema and trust-boundary source of truth for
[#342](https://github.com/peters/horizon/issues/342). It is an architecture
decision, not a claim that Teach mode, recording, or replay exist in the
product yet. Implementation lands in later serial PRs. This file does not
authorize edits to `horizon-browser`, `horizon-browser-mcp`,
`horizon-browser-cli`, or `horizon-ui`.

Routines are named, versioned, reviewable plans learned from an explicit user
demonstration in a Horizon browser panel. They execute through the existing
`browser_*` MCP contract and the settled durable runner from #324. They are
not coordinate macros, not a second action API, and not a replacement for the
redacted user audit.

Credential storage is specified in
[browser-routine-credentials.md](browser-routine-credentials.md). This document
only names the opaque slot references that plans may carry.

## Relationship to existing crates

| Crate | Role relative to routines |
| --- | --- |
| `horizon-browser-protocol` | Action model (`BrowserControlAction`, `BrowserTarget`, `AgentActionResult`). Routines compile into that model; they do not fork it. |
| `horizon-browser` | Process, profile, semantic resolution. Teach recording and Chromium/Firefox target resolution claim files here in later slices. |
| `horizon-browser-mcp` | Sole agent-facing `browser_*` contract. A new primitive is added only when a demonstrated interaction cannot be expressed, and only after a claimed additive change. |
| `horizon-browser-cli` | Durable jobs, `Plan` / `PlanStep`, `$var` substitution, intent/completion checkpoints. Routine replay consumes these APIs; it does not edit lifecycle machinery. |
| `horizon-browser-routines` | New workspace crate for the recording protocol, compiler, registry, and credential-broker interface. Depends on `horizon-browser-protocol` only. |

Safari remains unavailable for persistent routine login while
`BackendKind::SafariWebDriver` reports `persistent_profile: false`.

## Trust boundaries

Page content is untrusted data. It cannot become control instructions,
credential policy, origin allowlist entries, or a request for a different
credential slot.

A physical demonstration authorizes only the reviewed plan and run policy. It
does not authorize later consequential mutations, silent target patches, or
credential fill outside the recorded origins and field fingerprints.

The existing redacted audit continues independently. Teach mode must not
weaken it or copy typed secrets, cookies, or raw passwords into audit JSON.

Agent steering and Teach recording are mutually exclusive on one panel.
Recording acquires exclusive ownership for the demonstration; an agent cannot
drive the same panel until recording pauses or stops.

## Schema version

Every persisted routine, draft, and recording document carries
`schema_version`. Version `1` is the only accepted value in the first
implementation. Decoders use `deny_unknown_fields`. Unknown versions, missing
required fields, and extra fields are malformed input, not silently repaired.

A later version is a dedicated migration PR with tests. There is no
format-detection fallback between versions.

## Private layout

Routine definitions are stored separately from durable run instances. Filesystem
paths use an opaque UUID, never the user-provided name.

```text
~/.horizon/browser-routines/<routine-uuid>/routine.json
~/.horizon/browser-routines/<routine-uuid>/draft.json
~/.horizon/browser-routines/<routine-uuid>/lock
<existing browser profile root>/routines/<routine-uuid>/<backend>/
~/.horizon/browser-jobs/<run-uuid>/          # unchanged #324 job tree
```

`<existing browser profile root>` is the engine's current profile root
(`~/.horizon-browser/profiles` by default, or a configured `profile_root`).
Routine profiles live under a `routines/` prefix so they cannot collide with
panel-local profile directories.

On Unix, routine directories are mode `0700` and routine files are mode `0600`,
owned by the effective user. Symlinked ancestors, other-user ownership, and
group/world-writable parents are refused using the same private-store checks
already required of Horizon's local identity files. Windows uses the same
relative paths; owner-only ACLs land with the routine-registry implementation
slice and are not claimed by this document as already implemented.

Deleting a routine must offer to delete its profile and credential records and
must never affect another panel, routine, or shared profile. Session
duplication cannot share or orphan those records.

Writes are atomic (temp file in the same directory, `fsync`, replace). Each
routine directory has one exclusive lock so two processes cannot publish
concurrent definitions against the same UUID.

## Lifecycle states

A routine or an in-flight run occupies exactly one of:

```text
draft | ready | running | needs_login | needs_user | needs_reteach
succeeded | failed | cancelled | timed_out
```

| State | Meaning |
| --- | --- |
| `draft` | Saved recording or compiled plan that has not passed verification on the exact `plan_version`. |
| `ready` | Verification policy succeeded; eligible for manual (later, scheduled) runs. |
| `running` | One lease owns the routine profile. |
| `needs_login` | Cookies are insufficient and credentials are missing, locked, or not approved for this origin. Visible handoff. |
| `needs_user` | MFA, passkey, CAPTCHA, unexpected identity-provider change, or an uncertain mutation that cannot be proven complete. |
| `needs_reteach` | Target resolution is ambiguous or incompatible; no click is dispatched. |
| `succeeded` | Completion assertions held after the last step's postcondition. |
| `failed` | A step failed without a pause state. |
| `cancelled` | Cooperative cancellation from the #324 runner. |
| `timed_out` | Bounded run duration elapsed. |

Stopping a recording does not mark the routine `ready`. The user must mark
final assertions and a verification run of that exact `plan_version` must
succeed.

## `RoutineDefinition`

JSON object, `schema_version` 1, `deny_unknown_fields`:

| Field | Type | Notes |
| --- | --- | --- |
| `schema_version` | `u32` | Must be `1`. |
| `routine_id` | UUID string | Opaque; matches the directory name. |
| `name` | string | User-facing, not used in paths. Bounded printable text. |
| `backend_requirement` | `"chromium"` or `"firefox"` | Safari is not a valid requirement while it cannot persist a profile. |
| `profile_id` | UUID string | Routine-owned profile; equal to `routine_id` in v1. |
| `allowed_origins` | array of exact origins | Scheme + host + port. No path, userinfo, or query. HTTPS only, plus `http://127.0.0.1` and `http://localhost` for local fixtures. |
| `credential_policy` | object | See below. Default stores no username or password. |
| `variables` | array of `RoutineVariable` | Named parameters. Never secret values. |
| `steps` | array of `RoutineStep` | Reviewed compiler output. |
| `completion_assertions` | array of `Assertion` | User-marked final outcome. |
| `plan_version` | `u32` | Increments on every reviewed save. Resume binds to this exact value. |
| `created_at` / `updated_at` | RFC 3339 timestamps | |

`credential_policy` is `{ "mode": ..., "slot": <uuid> | omitted }`. `mode` is
one of `none` (default), `username_only`, or `username_and_password`. `slot`
is present only when the user opted in. The policy never contains a secret.

Every `credential_field` on this routine must use exactly `credential_policy.slot`.
`none` forbids any credential field. `username_only` permits `username` only.
`username_and_password` permits `username` and `password`. The broker looks up
`(routine_id, slot)`; a copied slot UUID from another routine does not fill.
Import of an exported routine clears `slot` and every `credential_field.slot`
and requires the user to bind credentials again.

`RoutineVariable` has `name` (bounded identifier, not `panel_id`) and optional
non-secret `default`. Defaults are omitted for fields the user did not
explicitly choose to remember.

## `RoutineStep`

| Field | Type | Notes |
| --- | --- | --- |
| `step_id` | string | Stable within the plan version. |
| `target_fingerprint` | `TargetFingerprint` or omitted | Required for click/fill/targeted scroll. |
| `precondition` | `Assertion` | Observed before dispatch during teaching. |
| `action` | `CompiledAction` | Tagged union below. |
| `value_source` | `ValueSource` or omitted | Fill/select only. |
| `postcondition` | `Assertion` | Observed after the demonstration step. |
| `mutation_class` | enum | `read_only`, `idempotent`, `mutating`, `consequential`. |
| `resume_policy` | enum | `retry_if_idempotent` or `never_replay_if_uncertain`. |

`consequential` covers submit, purchase, send, delete, publish, and other
irreversible mutations. Verification of a consequential routine requires an
explicit strategy or a user-approved live test. Horizon must not create
duplicate real-world mutations merely to validate a plan.

`never_replay_if_uncertain` is required for `mutating` and `consequential`.
`retry_if_idempotent` is allowed only for `read_only` and `idempotent`.

## `CompiledAction`

Tagged `type`, `deny_unknown_fields`. Selector strings are 1..=16 KiB
printable CSS derived from a durable fingerprint candidate (`unique_id`,
`test_id`, or reviewed `css_fallback`). `count` is 1..=3.

```text
{ "type": "navigate", "navigation": <NavigationTemplate> }
{ "type": "click", "selector": "#generate", "count": 1 }
{ "type": "fill", "selector": "#month" }
{ "type": "credential_fill", "selector": "#password" }
{ "type": "scroll", "selector": "#list", "delta_x": 0.0, "delta_y": 120.0 }
{ "type": "wait", "selector": "#status", "state": "present" | "visible" | "hidden" }
{ "type": "reload" }
{ "type": "back" }
{ "type": "forward" }
{ "type": "handoff" }
```

`scroll.selector` may be omitted for viewport scrolls. Navigate persists the
[`NavigationTemplate`](#navigation-template); the runner builds the URL at
each run from origin, path, and current non-secret variables. It never stores
or replays a `redact_url` string. `credential_fill` is not an MCP tool (see
compiler mapping).

## `Assertion`

Tagged `type`, `deny_unknown_fields`. Text `value` is 1..=4 KiB printable and
must not contain `?` or `#`.

```text
{ "type": "url_pattern", "value": "/reports/done" }
{ "type": "heading", "value": "Report ready" }
{ "type": "element_present", "target": <TargetFingerprint> }
{ "type": "element_absent", "target": <TargetFingerprint> }
{ "type": "accessible_state", "target": <TargetFingerprint>, "value": "selected" }
{ "type": "text_shape", "value": "NNN rows" }
```

`url_pattern` is a path or origin+path with no query or fragment. It is not a
`browser_navigate` URL.

## Target fingerprints

A durable target is a ranked list of backend-neutral candidates plus
uniqueness evidence, frame identity, origin, and a compact element digest.
Pointer coordinates may exist only as transient diagnostics on a recording
that has not been compiled. A compiled `RoutineStep` that has a click or fill
action and no non-coordinate candidate is invalid.

Candidate preference order:

1. Accessible role plus accessible name.
2. Associated label and control type.
3. Stable unique application attribute (explicit test id).
4. Stable unique `id`.
5. Bounded visible text and nearby semantic context.
6. Structural CSS, only as a reviewed fallback.

`TargetFingerprint`:

| Field | Type |
| --- | --- |
| `candidates` | ordered `TargetCandidate` array, at least one |
| `uniqueness` | `{ "match_count": u32, "unique": bool }` for the selected candidate |
| `frame` | `{ "top_level": bool, "origin": Origin, "chain": opaque frame ids }` |
| `digest` | compact non-secret element fingerprint |

`TargetCandidate` is a tagged union. Multi-component kinds have explicit
fields; `value` is only for scalar ids/CSS. Each string field is 1..=4 KiB
printable. `reviewed` defaults to false.

```text
{ "kind": "role_name", "role": "button", "name": "Generate report", "reviewed": false }
{ "kind": "label_control", "label": "Month", "control": "textbox", "reviewed": false }
{ "kind": "test_id", "value": "row-save", "reviewed": false }
{ "kind": "unique_id", "value": "generate", "reviewed": false }
{ "kind": "visible_text", "text": "Save", "context": "row 3", "reviewed": false }
{ "kind": "css_fallback", "value": "div > button", "reviewed": true }
```

Replay uses candidates in order and succeeds only when exactly one current
match is compatible with `digest`. A weak or ambiguous match becomes
`needs_reteach` and does not click.

A frame target cannot inherit credential permission or `allowed_origins`
membership from the top-level page. The frame's own origin is authoritative.

Today's MCP `browser_snapshot` / `browser_query` / `browser_act` inspect only
the top-level document and have no digest check. v1 MCP-emitted clicks and
fills are therefore top-level, unique, selector-capable targets only
(`unique_id`, `test_id`, or reviewed `css_fallback`). Nested-frame targets
and digest-gated replay are recorded on the fingerprint and execute as
`handoff` / `needs_reteach` until the Chromium resolver slice claims an
engine-side fill/click-by-fingerprint. That is not a new agent-facing MCP
tool.

## `ValueSource`

```text
{ "type": "literal", "value": "<non-secret>" }
{ "type": "variable", "name": "report_month" }
{ "type": "credential_field", "slot": "<uuid>", "field": "username" | "password" }
```

Ordinary fields default to `variable`. The user may explicitly choose
`literal` for a non-sensitive remembered value.

Password-like fields (`type=password`, password `autocomplete` tokens, or
user-marked sensitive) never become `literal` or `variable`. If
`credential_policy.mode` does not permit a password field (the default
`none`, or `username_only`), the fill is compiled to `handoff` and later
runs become `needs_login`. `credential_field` is emitted only after the user
opts into a mode that permits that field. Username/email login fields stay
`variable` unless the user chose to remember them.

`literal` and `variable` values may appear in compiled MCP plans using the
existing `Plan.variables` / `{"$var":"..."}` substitution. `credential_field`
must not. Putting a secret into a plan variable would serialize it into MCP
arguments, job state, and traces.

A `credential_field.slot` that is missing, or that does not equal
`credential_policy.slot`, or whose `field` is not permitted by `mode`, makes
the document malformed.

## Navigation template

`horizon_browser_protocol::redact_url` replaces query and fragment with the
literals `?<redacted>` and `#<redacted>`. That string is diagnostic-only. It
must not be a `browser_navigate` destination.

Navigate actions carry a separate `navigation` object:

```text
{
  "origin": "https://reports.example",
  "path": "/app",
  "query": [ { "name": "month", "source": { "type": "variable", "name": "report_month" } } ],
  "fragment": null
}
```

`origin` is an exact origin as defined above. `path` is 1..=8 KiB, starts with
`/`, and has no query or fragment. Each `query` entry maps a parameter name to
a non-secret `literal` or `variable`. Secret query values observed while
teaching are dropped; they never become literals. If the route cannot run
without a dropped secret parameter, the step is not auto-replayed
(`needs_user` / reteach). Fragment follows the same rule via `fragment`.

The persisted `RoutineStep` keeps this template on
`CompiledAction::navigate`. The runner constructs and encodes the URL when
each run starts, substituting current non-secret variables. `$var` is not
applied as a substring inside a stored URL.

## Recording protocol

The crate's first code module stores a backend-neutral `RecordedAction` list.
It is not a durable job and does not use the #324 run directory.

`RecordedAction` JSON (`deny_unknown_fields`):

| Field | Type |
| --- | --- |
| `action_id` | bounded identifier |
| `recorded_at_millis` | non-negative i64 |
| `kind` | `RecordedKind` tagged union (`navigate`, `click` with `count`, `fill`, `scroll` with finite `delta_x`/`delta_y`, `wait` with selector and `SelectorState`, `reload`, `back`, `forward`, `handoff`) |
| `target` | optional `TargetFingerprint` (required for click/fill/targeted scroll) |
| `page_origin` | `Origin` |
| `url_pattern` | `redact_url` diagnostic string; may contain `?<redacted>` |
| `navigation` | optional `NavigationTemplate`; required on `navigate` |
| `value_source` | optional; required on `fill` |
| `field_classification` | `ordinary` (default), `username`, `password` |
| `mutation_class` | enum above |
| `precondition` / `postcondition` | optional `Assertion` |

Gesture buffering (press/release → click, scroll coalescing, per-field typing)
is controller work, not schema.

A redaction pass refuses to serialize typed secrets. Tests must cover round
trips, `deny_unknown_fields` drift, malformed input, and that password-shaped
fields persist as `credential_field` or `handoff`, never as a literal.

## Compiler mapping to MCP

The compiler is a pure function from a semantic recording plus reviewer
choices to a validated list of MCP `PlanStep`s. It does not call a model.

| Recorded kind | MCP tool | Notes |
| --- | --- | --- |
| navigation | `browser_navigate` | URL from `NavigationTemplate`, never from `url_pattern`. Wait defaults to the protocol commit wait. |
| click / double-click | `browser_act` `click` | `count` 1 or 2. Target is a reviewed selector derived from the fingerprint, never coordinates. |
| fill (literal/variable) | `browser_act` `fill` | `value` is a literal or `{"$var":"..."}`. |
| fill (credential) | *not expressible today* | See below. |
| scroll | `browser_act` `scroll` | Only coalesced bursts required to reach a target. |
| wait | `browser_wait` | Selector state from the postcondition. |
| back / forward / reload | `browser_act` | |
| user handoff | `browser_handoff` | Login/MFA pauses. |

Existing `browser_act` `fill` requires a `value` string and is agent-facing.
Credential fill must not be exposed as `browser_act` `{ credential_slot }` —
any MCP caller who saw an exported slot UUID could then nominate a target.
The runner instead holds a scoped capability created when it acquired the
routine lease (`routine_id`, `plan_version`, profile lease). Only that
capability may ask the engine to call `CredentialBroker`, which already
knows the slot, allowed field, origins, and fingerprint. Generic
`browser_*` calls cannot request credential fill.

No new agent-facing MCP primitive is in scope for the first usable release.
Engine-side click/fill-by-fingerprint (item 3) is a claimed
`horizon-browser` change, not a second MCP contract.

The compiler rejects: coordinate-only clicks; empty fingerprints; credential
literals; a `credential_field` that does not match `credential_policy`;
unknown `schema_version`; steps whose action is not in the table above;
`consequential` steps without a resume policy of
`never_replay_if_uncertain`; navigate steps that still use a redacted audit
URL as the destination.

## Assertions

Assertions are evaluated from a snapshot/query, never from page-supplied
scripts.

Allowed kinds: URL path/pattern (already redacted), unique heading or status
element, element presence/absence, accessible state or selected value, and
bounded text shape that does not contain secrets. Completion assertions are
the user-marked subset. A routine stays `draft` until a verification run
observes all of them on the exact `plan_version`.

## Execution, leases, and resume

Replay is a later slice. It must consume the settled #324 APIs:

- Persist intent before dispatch and completion only after the postcondition
  holds (`RunCheckpoint` / `CheckpointIntent`).
- An interrupted mutating step is `uncertain` and is never blindly replayed.
- Resume first re-evaluates the recorded postcondition; if it already holds,
  checkpoint without repeating the step.
- Resume binds `plan_version`, `routine_id`, `profile_id`, and the job id.
- One lease per routine profile; concurrent scheduled runs are refused.

Cookies live only in the routine-owned profile. Later runs try that profile
first and continue without the OS credential store when an authenticated-state
assertion holds.

## Scheduling

Out of scope until manual run and resume are proven. The first scheduling
version may use a documented platform scheduler instead of an in-process
daemon. Routine semantics and reports stay identical. Background credential
fill is forbidden unless both the routine policy and the schedule explicitly
allow it.

## Invariants

- Recording starts only through an explicit, visible user action.
- Inactive Teach mode adds no per-frame or pointer-move work.
- Compiled plans contain no coordinate-only clicks and no secret values.
- Exports omit secrets, cookies, and profile bytes; they may include field
  names and missing-secret markers.
- Site drift does not auto-approve a changed target for credential fill or a
  consequential mutation.
- Full Horizon validation runs on every implementation slice before push.
