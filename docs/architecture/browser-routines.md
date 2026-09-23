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
| `horizon-browser-cli` | Durable jobs, `Plan` / `PlanStep`, `$var` substitution, intent/completion checkpoints. The CLI layer adapts routine DTOs into those APIs; it does not edit lifecycle machinery. |
| `horizon-browser-routines` | New workspace crate for the recording protocol, compiler, registry, and credential-broker interface. Depends on `horizon-browser-protocol` only. The compiler emits crate-local `CompiledRoutine` / `McpCall` values, not CLI `PlanStep`. |

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
~/.horizon/browser-routines/.<routine-uuid>.lock
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
routine has one exclusive lock so two processes cannot publish concurrent
definitions against the same UUID. The lock file sits beside the routine
directory rather than inside it, because deletion renames the directory while
holding the lock and Windows refuses that rename while a file inside is open.
Lock files are kept after deletion; removing a lock file that another process
may be waiting on would let two holders proceed at once.

## Lifecycle states

Routine definition status and run status are separate machines. A ready
routine can have a running run and historical succeeded runs at once.

Definition status is exactly one of `draft | ready`. `ready` iff
`verified_plan_version == plan_version`. `needs_reteach` on a run also
clears `verified_plan_version` so the definition returns to `draft`.

A run occupies exactly one of:

```text
running | needs_login | needs_user | needs_reteach
succeeded | failed | cancelled | timed_out
```

| State | Machine | Meaning |
| --- | --- | --- |
| `draft` | definition | `verified_plan_version` is missing or not equal to `plan_version`. |
| `ready` | definition | Verification of this exact `plan_version` succeeded. |
| `running` | run | One lease owns the routine profile. |
| `needs_login` | run | Cookies are insufficient and credentials are missing, locked, or not approved for this origin. |
| `needs_user` | run | MFA, passkey, CAPTCHA, identity-provider change, or an uncertain mutation. |
| `needs_reteach` | run | Target resolution is ambiguous; also returns the definition to `draft`. |
| `succeeded` | run | Completion assertions held after the last step's postcondition. |
| `failed` | run | A step failed without a pause state. |
| `cancelled` | run | Cooperative cancellation from the #324 runner. |
| `timed_out` | run | Bounded run duration elapsed. |

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
| `allowed_origins` | array of exact origins | Scheme + host + port. HTTPS only, plus loopback HTTP fixtures. Navigation templates, recorded `page_origin`, frame origins, and post-redirect origins must be members. Leaving the set is `needs_user` (fail closed). |
| `credential_policy` | object | See below. Default stores no username or password. |
| `variables` | array of `RoutineVariable` | Named parameters. Never secret values. |
| `steps` | array of `RoutineStep` | 1..=256 reviewed compiler outputs (CLI plan bound). |
| `completion_assertions` | array of `Assertion` | At least one user-marked final outcome. Empty is malformed. |
| `plan_version` | `u32` | Increments on every reviewed save. Resume binds to this exact value. |
| `verified_plan_version` | `u32` or omitted | Set only after a verification run of that exact version succeeded. `ready` is derived from `verified_plan_version == plan_version`. An edit that bumps `plan_version` returns the routine to `draft`. |
| `created_at` / `updated_at` | RFC 3339 timestamps | |

`credential_policy` is `{ "mode": ..., "slot": <uuid> | omitted, "allowed_origins": [...] }`.
`mode` is one of `none` (default), `username_only`, or `username_and_password`.
`slot` is present only when the user opted in. `allowed_origins` is the
login-form allowlist used by the broker; it is not the routine-wide
navigation origin list. The policy never contains a secret.

Every `credential_field` on this routine must use exactly `credential_policy.slot`.
`none` forbids any credential field. `username_only` permits `username` only.
`username_and_password` permits `username` and `password`. The broker looks up
`(routine_id, slot)`; a copied slot UUID from another routine does not fill.
Import of an exported routine allocates new ids, sets `credential_policy.mode`
to `none`, rewrites every `credential_fill` to `handoff` / `needs_login`,
clears `verified_plan_version`, and requires the user to bind credentials
again. It does not leave a `credential_field` with a missing slot.

`RoutineVariable` has `name` (bounded identifier, not `panel_id`) and optional
non-secret `default`. Defaults are omitted for fields the user did not
explicitly choose to remember.

## `RoutineStep`

| Field | Type | Notes |
| --- | --- | --- |
| `step_id` | string | Unique in the plan version; 1..=64 ASCII alphanumeric / `_` / `-` (CLI `PlanStep.id`). |
| `target_fingerprint` | `TargetFingerprint` or omitted | Required for click/fill/targeted scroll. |
| `precondition` | `Assertion` | Observed before dispatch during teaching. |
| `action` | `CompiledAction` | Tagged union below. |
| `value_source` | `ValueSource` or omitted | Required on `fill` and `credential_fill`. Generic `fill` allows only `literal` or `variable`. `credential_fill` allows only `credential_field`. Other pairings are malformed. |
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

Tagged `type`, `deny_unknown_fields`. Click, fill, credential fill, and
targeted scroll use the step's `target_fingerprint`, not a CSS selector.
`count` is 1..=3.

```text
{ "type": "navigate", "navigation": <NavigationTemplate> }
{ "type": "click", "count": 1 }
{ "type": "fill" }
{ "type": "credential_fill" }
{ "type": "scroll", "delta_x": 0.0, "delta_y": 120.0 }
{ "type": "wait", "selector": "#status", "state": "present" | "visible" | "hidden" }
{ "type": "reload" }
{ "type": "back" }
{ "type": "forward" }
{ "type": "handoff", "pause": "needs_login" | "needs_user" | "needs_reteach" }
```

Viewport scroll omits `target_fingerprint`. Targeted scroll requires one.
Navigate persists the
[`NavigationTemplate`](#navigation-template); the runner builds the URL at
each run from origin, path, and current non-secret variables. It never stores
or replays a `redact_url` string. `credential_fill` is not an MCP tool (see
compiler mapping). `handoff.pause` is the durable run state the runner
enters; it is required.

## `Assertion`

Tagged `type`, `deny_unknown_fields`. Text `value` is 1..=4 KiB printable and
must not contain `?` or `#`.

```text
{ "type": "url_pattern", "value": "/reports/done" }
{ "type": "heading", "value": "Report ready" }
{ "type": "element_present", "target": <TargetFingerprint> }
{ "type": "element_absent", "target": <TargetFingerprint> }
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
| `candidates` | ordered `TargetCandidate` array, at least one. Each candidate carries its own `{ "match_count", "unique" }`. |
| `selected` | index of the candidate the compiler used for a selector, when any |
| `frame` | `{ "top_level": bool, "origin": Origin, "chain": durable iframe descriptors }` |
| `digest` | compact non-secret element fingerprint |

`TargetCandidate` is a tagged union. Multi-component kinds have explicit
fields; `value` is only for scalar ids/CSS. Each string field is 1..=4 KiB
printable. `reviewed` defaults to false.

```text
{ "kind": "role_name", "role": "button", "name": "Generate report", "reviewed": false, "match_count": 1, "unique": true }
{ "kind": "label_control", "label": "Month", "control": "textbox", "reviewed": false, "match_count": 1, "unique": true }
{ "kind": "test_id", "attribute": "data-testid", "value": "row-save", "reviewed": false, "match_count": 1, "unique": true }
{ "kind": "unique_id", "value": "generate", "reviewed": false, "match_count": 1, "unique": true }
{ "kind": "visible_text", "text": "Save", "context": "row 3", "reviewed": false, "match_count": 2, "unique": false }
{ "kind": "css_fallback", "value": "div > button", "reviewed": true, "match_count": 1, "unique": true }
```

Replay uses candidates in order and succeeds only when exactly one current
match is compatible with `digest`. A weak or ambiguous match becomes
`needs_reteach` and does not click.

A frame target cannot inherit credential permission or `allowed_origins`
membership from the top-level page. The frame's own origin is authoritative.
`chain` is not a session-scoped browser frame id. Each entry is
`{ "origin": Origin, "name": <accessible iframe name or title> }` so a later
session can relocate the frame. Unknown or empty names make the target
non-MCP (`handoff` / `needs_reteach`) until the engine resolver lands.

Today's MCP `browser_act` has no digest check, so a taught selector can match
a different element on a later page. v1 therefore does **not** dispatch
MCP click/fill from a selector alone. Those steps wait for the claimed
engine click/fill-by-fingerprint (item 3), which re-resolves candidates,
requires `unique` and digest compatibility on the current document
generation, and fails closed to `needs_reteach`. Nested-frame targets use
the same engine path. Until that engine operation exists, click/fill steps
are `handoff` / `needs_reteach` and are not MCP `PlanStep`s. Navigate, wait,
viewport scroll, reload, back, and forward may still emit MCP.

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
opts into a mode that permits that field. Username/email login fields also become `handoff` / `needs_login` unless
the user opted into `credential_field` for username. They must not be stored
as `variable` or `literal` (that would serialize the username into
`Plan.variables` and MCP arguments).

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
  "path": [ { "source": { "type": "literal", "value": "app" } } ],
  "query": [ { "name": "month", "source": { "type": "variable", "name": "report_month" } } ],
  "fragment": null
}
```

`origin` is an exact origin as defined above. `path` is an ordered array of
segments, each `{ "source": <literal|variable> }`, joined with `/` and
percent-encoded at run time. `$var` is never a substring inside a stored
string. Segments classified as secret (token-like, reset/magic tails) are
omitted from the template; without a user-supplied non-secret variable the
step is `handoff` / `needs_user`. Each `query` entry maps a parameter name to
a non-secret `literal` or `variable`. Secret query values observed while
teaching are dropped. Fragment follows the same rule via `fragment`.

The persisted `RoutineStep` keeps this template on
`CompiledAction::navigate`. The runner constructs and encodes the URL when
each run starts, substituting current non-secret variables. `$var` is not
applied as a substring inside a stored URL.

## Recording protocol

The crate's first code module stores a versioned envelope, not a bare action
list. It is not a durable job and does not use the #324 run directory.

```text
{ "schema_version": 1, "recording_id": "<uuid>", "actions": [ <RecordedAction>, ... ] }
```

`schema_version` is required and must be `1`. Unknown versions are rejected.

`RecordedAction` JSON (`deny_unknown_fields`):

| Field | Type |
| --- | --- |
| `action_id` | bounded identifier |
| `recorded_at_millis` | non-negative i64 |
| `kind` | `RecordedKind` tagged union (`navigate`, `click` with `count` 1..=3, `fill`, `scroll` with finite `delta_x`/`delta_y`, `wait` with selector and `SelectorState`, `reload`, `back`, `forward`, `handoff` with `pause`) |
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
choices to a `CompiledRoutine`. It does not depend on `horizon-browser-cli`
and does not return CLI `PlanStep`. The CLI adapter later copies `McpCall`
fields into `PlanStep` where a tool exists.

| Recorded kind | Crate output | CLI/MCP adapter |
| --- | --- | --- |
| navigation | `navigate` + template | `browser_navigate` only if origin+path stay in `allowed_origins`; else `needs_user` |
| click | `click` + fingerprint | engine fingerprint dispatch, **not** MCP |
| fill (non-login) | `fill` + fingerprint | engine fingerprint dispatch, **not** MCP |
| fill (credential) | `credential_fill` | runner-to-engine `FillSink`, **not** MCP |
| viewport scroll | coalesced `scroll` without fingerprint | `browser_act` `scroll` |
| targeted scroll | `scroll` + fingerprint | engine fingerprint dispatch, **not** MCP |
| wait | `wait` | `browser_wait` |
| back / forward / reload | same | `browser_act` |
| user handoff / needs_login | `handoff` | **not** a successful `browser_handoff` `PlanStep`. The routine runner suspends with `needs_login` / `needs_user` and a checkpoint. Today's `browser_handoff` is checkpointed as success and then cleared on teardown, so it cannot be the pause. |

The durable runner still dispatches ordinary MCP `PlanStep`s. Credential fill
and login pause are **not** MCP steps. The routine runner, holding the lease,
calls `horizon-browser` in-process (`CredentialBroker` → `FillSink`) between
MCP steps. That transport is the engine API, not `browser_act`. Generic
agent-facing `browser_*` calls cannot request credential fill.

No new agent-facing MCP primitive is in scope for the first usable release.
Engine-side click/fill-by-fingerprint (item 3) is a claimed
`horizon-browser` change. Routine-step checkpoint grouping (dispatch +
postcondition as one unit) is a claimed additive runner hook; see execution.

The compiler rejects: coordinate-only clicks; empty fingerprints; credential
literals; a `credential_field` that does not match `credential_policy`;
unknown `schema_version`; steps whose action is not in the table above;
`consequential` steps without a resume policy of
`never_replay_if_uncertain`; navigate steps that still use a redacted audit
URL as the destination.

## Assertions

Assertions are evaluated from a snapshot/query, never from page-supplied
scripts.

Allowed kinds are the `Assertion` variants above. `accessible_state` is not
v1: today's `BrowserNode` has no selected/checked/expanded field. Completion
assertions are the user-marked subset and must be non-empty. A routine stays
`draft` until a verification run observes all of them on the exact
`plan_version`.

## Execution, leases, and resume

Replay is a later slice. Today's runner records completion as soon as an MCP
`PlanStep` returns success, before any later snapshot could prove a
postcondition. Routine replay therefore cannot put a mutating `browser_act`
and its postcondition in two independent MCP steps.

The claimed additive hook is a grouped routine-step unit: persist intent,
dispatch (MCP or in-process engine fill), evaluate the postcondition, then
record completion. Consume existing `RunCheckpoint` / `CheckpointIntent`
fields; do not replace the runner. Until that hook exists, mutating routine
steps are not executed through the current per-tool completion path.

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
