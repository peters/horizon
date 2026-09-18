# Remote browser sessions on real mobile devices

Status: accepted for [#628](https://github.com/peters/horizon/issues/628) phase 1
on 2026-09-14, informed by the real-device evidence in
[`docs/testing/2026-09-14-remote-mobile-webdriver-spike.md`](../testing/2026-09-14-remote-mobile-webdriver-spike.md).
The decision and phase list record the original implementation plan. Provider
profiles, credential binding, remote allocation and cleanup are implemented; the
shared usage and capacity behavior is described in the implementation notes below. Provider credentials are separate from the routine login
credentials in [`browser-routine-credentials.md`](browser-routine-credentials.md).

## Decision

Classic W3C WebDriver over HTTPS is the portable baseline for a remote
real-device session. The spike controlled a real iPhone 16 and a real Pixel 9
with nothing but standard HTTP requests: New Session allocated the device,
navigation, `execute/sync`, screenshots, element find, value and click, frame
switching and the Appium orientation extension all worked, and DELETE released
the device with the provider confirming `done`. The Horizon-internal client
therefore stays a WebDriver HTTP client. WebDriver BiDi, CDP and provider
streaming are optional negotiated enhancements, never prerequisites.

Three responsibilities stay in three places:

| Responsibility | Lives in | Never contains |
| --- | --- | --- |
| Remote target configuration and provider adaptation: endpoint, authentication, device requirements to capabilities, optional allocation and cleanup hooks, artifacts | `horizon-browser` remote adapter module and `horizon-core` config | page semantics, panel state |
| Browser session transport: authenticated HTTPS, New Session, commands, typed transport errors, screenshots, actions | `horizon-browser` remote transport and session owner | provider branches, credential values in errors |
| Panel and MCP behavior: refs, ownership, navigation outcomes, handoff, audit, capability disclosure | existing `horizon-core` browser manifest and `horizon-browser-mcp` | provider names, endpoints, capability namespaces |

The existing loopback WebDriver transport in `crates/horizon-browser/src/webdriver/http.rs`
keeps its loopback-only rule. The remote transport is a second, separate
implementation built on the workspace's existing `ureq` with rustls, configured
like the Azure and RunPod transports: HTTPS only, no redirects, bounded response
size, global timeout, Horizon user agent.

## Configuration contract

Connection profiles, targets and limits live in the existing `browser:` section
of `~/.horizon/config.yaml` under a new `remote` key. The section is additive
and optional, so existing files need no migration step and the config version
stays at 10 until a later phase changes an existing key. Unknown keys are
rejected.

```yaml
browser:
  remote:
    providers:
      device_cloud:
        adapter: webdriver            # generic adapter; named adapters only for demonstrated differences
        endpoint: https://grid.example.net/wd/hub
        authentication:               # exactly one variant, tagged by kind
          kind: basic                 # basic: username_ref + password_ref
          username_ref: device-cloud-user
          password_ref: device-cloud-key
          # kind: bearer              # bearer: token_ref only
          # kind: none                # none: no other fields allowed
        credential_bindings:
          device-cloud-user: { store: os_keychain, slot: remote-browser/device-cloud/username }
          device-cloud-key: { store: session }
          # Unattended / container alternative (variable names only; values come
          # from the launching process, never from this file):
          # device-cloud-user: { store: environment, slot: REMOTE_BROWSER_USERNAME }
          # device-cloud-key: { store: environment, slot: REMOTE_BROWSER_ACCESS_KEY }
        limits:
          max_sessions: 1
          allocation_timeout_seconds: 120
          idle_release_seconds: 180
          max_session_seconds: 1800
    targets:
      ios_phone:
        provider: device_cloud
        browser_name: safari
        platform_name: iOS
        device: { kind: physical, model: iPhone 16, os_version: "18" }
        capability_extensions:        # namespaced only; device, browser, platform and credential keys are rejected
          appium:automationName: XCUITest
```

Rules the configuration PR enforces:

- `authentication` is a tagged variant with variant-specific fields: `none`
  has no fields, `basic` requires `username_ref` and `password_ref`, `bearer`
  requires `token_ref`. Unknown or cross-variant fields are rejected, and
  every reference named by the selected variant must have a binding.
- `endpoint` must be `https`, must not carry userinfo or a query string, and is
  the only origin that ever receives the provider credential. A loopback `http`
  endpoint is allowed only for a self-hosted local grid and is labelled as such.
- `capability_extensions` keys must be namespaced (`vendor:name`). Keys, or
  nested option keys, that duplicate a normalized field (browser, platform,
  device name, OS version, real-device flag) or that carry a credential
  (user, access key, password, token) are rejected by name; the adapter
  derives those capabilities from the normalized fields and the bindings.
- `device.kind: physical` is a Horizon requirement, not a WebDriver capability.
  The adapter maps it to the provider's real-device request; comparing the
  allocated device against the requirement (provider session evidence, the
  spike's `realMobile` plus session record check) is lifecycle work that
  follows the wiring, and until it lands the requirement only shapes the
  request. The `webdriver` adapter sends the normalized
  device fields as `appium:deviceName` and `appium:platformVersion`; the
  `browserstack` adapter sends them inside `bstack:options` as `deviceName`,
  `osVersion` and `realMobile: "true"` for a physical requirement, merged with
  any other `bstack:options` keys the target's extensions carry.
- `credential_bindings` hold references only. `store: session` binds a value
  entered in Horizon and held in a scoped in-memory sink until explicit clear or
  exit. `store: os_keychain` uses `keyring-core` with the native store crates
  the routine-credential ADR selected (macOS Keychain, Windows Credential
  Manager, Linux Secret Service), under the distinct service name
  `horizon-remote-browser` and items keyed by endpoint origin plus slot. The
  `keyring` facade crate itself is not linked: its 4.x documentation directs
  applications that choose their stores to `keyring-core`. `store: environment`
  names a process environment variable in `slot` (for example
  `REMOTE_BROWSER_USERNAME`). Horizon copies that variable from the launching
  process into the in-memory sink at startup and does not look at session or
  OS-store values for the same reference. There is no implicit interpolation of
  arbitrary `$VARS` in YAML, and no silent fallback to another store. Spawned
  agent and terminal children inherit these variables normally. The Horizon
  process, its children, the container specification and process environment
  inspection can expose launch-time values. Use session or keychain bindings
  when credentials should not be delivered through the environment. No file store.
- Exported profiles carry providers, targets, limits and authentication
  references; machine-local `credential_bindings` are stripped, and whether a
  reference has a value on this machine is a live readiness query rather than
  part of the file. Import never overwrites an existing provider's endpoint or
  rebinds an existing reference to another origin without an explicit user
  confirmation: a conflicting file is refused whole and the local definition
  is left as it was.
- The portable profile is its own document, `horizon_remote_browser_profile: 1`
  followed by a `remote` map with the same `providers` and `targets` schema as
  `browser.remote` (`horizon_core::browser::remote_profile`). Settings > Remote
  browsers exports and imports it from a path next to the configuration file
  by default (`remote-browser-profile.yaml`); an import lands in the editing
  configuration and is written by Save. The same two operations are available
  without a window as `horizon --export-remote-profile <path>` and
  `horizon --import-remote-profile <path>`, which rewrite the configuration
  file atomically and exit, so a second computer reached over SSH can take a
  profile before its owner enters credentials. Documents are bounded at 256 KiB
  and rejected on an unknown key, another format number, any binding, or an
  endpoint or authentication conflict with a trusted local provider.
- Existing files without `browser.remote` load unchanged and local backends keep
  their defaults. Parsing and readiness checks never allocate.

### Unattended containers and CI

Every credential reference selects its own variable. Multiple providers may
use distinct variables simultaneously, even when their reference names match.
A provider may also mix environment, session and keychain bindings; only the
explicitly selected source is consulted. Settings changes capture newly bound
variables from the same launching process and drop unused snapshots.

Configuration names variables only. Values come from the invoking environment
or CI secret bindings; they do not belong in the image, the target profile, or
command-line arguments.

```yaml
browser:
  remote:
    providers:
      device_cloud:
        adapter: webdriver
        endpoint: https://grid.example.net/wd/hub
        authentication:
          kind: basic
          username_ref: device-cloud-user
          password_ref: device-cloud-key
        credential_bindings:
          device-cloud-user: { store: environment, slot: REMOTE_BROWSER_USERNAME }
          device-cloud-key: { store: environment, slot: REMOTE_BROWSER_ACCESS_KEY }
      second_cloud:
        adapter: webdriver
        endpoint: https://second-grid.example.net/wd/hub
        authentication: { kind: bearer, token_ref: api-token }
        credential_bindings:
          api-token: { store: environment, slot: SECOND_BROWSER_TOKEN }
    targets:
      ios_phone:
        provider: device_cloud
        browser_name: safari
        platform_name: iOS
        device: { kind: physical, model: configured-ios-model, os_version: "18" }
```

```bash
docker run --rm \
  -e REMOTE_BROWSER_USERNAME \
  -e REMOTE_BROWSER_ACCESS_KEY \
  -e SECOND_BROWSER_TOKEN \
  -v "$PWD/config.yaml:/config/config.yaml:ro" \
  example/browser-test-runner \
  --config /config/config.yaml --ephemeral
```

GitHub Actions placeholder (the secret values stay in the runner's secret
store; the workflow file contains only names):

```yaml
jobs:
  remote-browser:
    runs-on: ubuntu-latest
    container: example/browser-test-runner
    env:
      REMOTE_BROWSER_USERNAME: ${{ secrets.REMOTE_BROWSER_USERNAME }}
      REMOTE_BROWSER_ACCESS_KEY: ${{ secrets.REMOTE_BROWSER_ACCESS_KEY }}
      SECOND_BROWSER_TOKEN: ${{ secrets.SECOND_BROWSER_TOKEN }}
    steps:
      - run: horizon --config /config/config.yaml --ephemeral
```

Remote sessions still use the public `browser_*` MCP tools (`browser_create`
with a configured `target`, then snapshot/actions, then `browser_close`). No
alternate control API is added for environment credentials.

## Device identity and evidence

Requested versus actual identity is compared from provider session metadata or
an explicitly trusted self-hosted device map, never from a user-agent string or
viewport size. The spike showed a provider resolving `18` to `18.6`, so the
comparison is by version component prefix and the resolved version is shown.
Evidence is one of `physical`, `emulated` or `unknown`; a `physical` requirement
that resolves to anything else releases the session and reports the unmet
requirement instead of a ready panel.

Where the evidence comes from is the adapter's business. The `browserstack`
adapter reads the grid's own session record (`GET /automate/sessions/<id>.json`
at `api.browserstack.com`, with the hub's authorization and no redirects): the
record names the real device the session runs on and its resolved OS version,
and a record that cannot be fetched leaves the identity unknown. The generic
`webdriver` adapter reads the capabilities the endpoint echoes in its New Session
reply (`appium:deviceName`, `appium:platformVersion`, and a real-mobile flag when
the endpoint states one); a hosted grid that echoes nothing therefore cannot
satisfy a `physical` requirement through that adapter. The verified identity
reaches the manifest and the MCP panel as `remote_device`, and a rejection ends
the create with `remote_device_rejected` after an immediate release attempt
whose outcome the panel note states; a session that could not be allocated or
safely started ends it with `remote_allocation_failed` (nothing held); a refusal
is classified from the provider's answer so the agent learns which of the
credential (`remote_authentication_failed`), the account's automation
entitlement (`remote_not_entitled`) or the device request
(`remote_device_unavailable`: unknown device, none free, parallel limit) was
refused, with a fixed public text each and the provider's own words only in the
panel note. A failure status with a non-JSON body counts as a refusal by status
(`http 401`, `http 403`), never as an unknown outcome, since no session was
created. Successful allocation is its own event. A hosted grid may queue rather
than refuse when its parallel limit is reached; that shows as a slow allocation
bounded by the configured allocation timeout, and one
whose allocation or cleanup got no trustworthy answer with
`remote_allocation_unknown` (the slot stays counted).

## Session lifecycle and ownership

States: `validating`, `allocating`, `connecting`, `ready`, `releasing`,
`released`, plus the terminal outcomes `failed`, `allocation_unknown` and
`release_unknown`. Transitions are cancellable before and after allocation;
cancelling after allocation releases the owned session.

- Allocation timeout and command timeout are separate. Both spike platforms
  needed 16 to 19 seconds for New Session, so allocation reports progress and
  never pretends the first page is ready; navigation outcomes reuse the
  existing committed, pending, failed and superseded contract.
- New Session is never retried after an ambiguous timeout. The outcome is
  `allocation_unknown` with the provider build and session name retained for
  reconciliation through the adapter's optional lookup hook. Mutations (click,
  value, submit, actions) are likewise never replayed after an ambiguous
  response.
- Every allocation is bound to the Horizon instance, workspace, panel and a
  unique request id. For generic `webdriver` grids, concurrency is enforced across Horizon instances that
  share the same configured provider identity through a private lock keyed by
  endpoint and username reference; provider quotas stay authoritative. The
  lock is `max_sessions` slot files under the Horizon home
  (`remote-slots/<sha256 of endpoint and reference>/slot-N.lock`), each held
  with an exclusive advisory file lock from before New Session until the
  release is established; every slot file in the identity's directory counts
  whatever its index, so a quota reduced in configuration still counts leases
  taken under the larger one, and the scan and grant run under a per-identity
  coordination lock that is tried without blocking for at most twenty
  milliseconds (`remote_quota_contended` asks the agent to create again). The operating system frees a dead instance's slots, a
  host whose slot files cannot be used falls back to its own count, and holds
  a replaced board left unreleased stay leased by the host for the rest of the
  process.
- The `browserstack` adapter delegates capacity to the provider. It takes no
  local quota lease and does not enforce `max_sessions`; existing fields remain
  readable for compatibility. Allocation records still retain ownership and
  uncertain outcomes for exact-session cleanup. Allocation, idle, and lifetime
  timeouts remain enforced. Provider usage never decides local admission.
- Settings > Remote browsers shows each supported provider's shared running /
  allowed and queued sessions. `browser::remote_usage` normalizes provider APIs;
  its first adapter reads `GET /automate/plan.json` at `api.browserstack.com`
  with the configured session credentials. The displayed allowance is the
  smaller of the plan and team limits when both are returned. This snapshot
  includes competing clients and can change before the next allocation.
  Refresh runs on opening the view, every 30 seconds while open, or manually.
  Network and OS-store reads run on a worker; requests have a ten-second timeout,
  a 64 KiB response limit, and never follow redirects. Failures show an error
  and mark the last successful sample stale. Profile/binding changes and local
  credential edits invalidate cached results. Multiple provider rows refresh
  independently; unsupported adapters do not make usage API requests.
- DELETE that times out is `release_unknown`, retried in a bounded way for that
  exact session id and shown as unresolved cleanup. Release is `released` only
  when the provider reports a terminal status where such an API exists;
  otherwise the state is `released_unverified` and the documentation says so.
- Provider idle and lifetime expiry are requested where supported and a local
  hard-deadline and idle watchdog always runs on its own thread, started when
  allocation succeeds, so a driver blocked in a long classic command cannot
  delay release; the driver reads the settled outcome instead of deleting
  again. Keepalive traffic never extends the local lifetime or resets the
  user-activity idle policy.
- Minimal private recovery metadata (endpoint origin, credential reference,
  session id, request id, deadline) is persisted for owned sessions. Session ids
  and signed artifact links are sensitive; MCP exposes a correlation id.

## Capabilities

Capabilities are advertised individually from negotiated and verified behavior,
not from the backend enum. A remote panel reports at least: `dom`, `touch`,
`screenshot`, `live_view`, `network_capture`, `response_bodies`, `recording`,
`orientation`, each `supported`, `unsupported` or `unverified`. The spike fixes
the initial expectations:

- `dom`, `screenshot`, `orientation`, script scrolling and element value entry:
  supported on both spike targets.
- `touch`: verified per session by a probe, because a W3C touch swipe scrolled
  real iOS Safari but not the provider's Android Chrome path, and the wheel
  source is rejected or ignored on both. Script scrolling is the portable
  fallback the semantic `scroll` action uses when touch is not verified.
- `network_capture`, `response_bodies`, `recording`: unsupported on remote
  targets until a later phase verifies a transport; screenshot polling never
  implies them.
- `live_view`: adaptive screenshot polling with bounded rate, payload and
  memory, repainting on frame or state change. Richer streaming is optional.

## Coordinates and screenshots

CSS pixels come from `visualViewport` and the element rect, never from
`window.innerWidth`, which the spike showed inflated by a constant factor on
the Android path. Screenshot pixels are visual-viewport CSS pixels times the
device pixel ratio, with a per-session origin offset: zero on Android, where the
screenshot is the page area, and the status bar plus browser chrome height on
iOS, where the screenshot is the whole screen. The offset is calibrated once
per session from a known element rect and re-derived after orientation or
keyboard changes. The physical device is never resized because the panel is.

## Failure outcomes

Every user-facing failure is typed and redacted:

`config_invalid`, `credential_missing`, `credential_locked`,
`authentication_failed`, `entitlement_missing` (authenticated but the account
cannot start automation sessions), `device_unavailable`, `allocation_timeout`,
`allocation_unknown`, `device_mismatch` (requested physical, got emulated or
unknown), `transport_error`, `session_expired`, `rate_limited`,
`release_failed`, `release_unknown`. Authentication success, entitlement, device
availability and allocation are reported separately, and none of them appears
as a ready state.

## Phases

1. This document and `scripts/remote-browser-spike/` (done in this PR).
2. Configuration and credential bindings, in three PRs: the `browser.remote`
   schema with validation, portable export and import (`horizon-browser-protocol::remote`);
   the session-only sink and the `keyring` adapter behind a fake-store seam;
   then credential entry and redacted readiness in the UI.
3. Remote transport and lifecycle: `ureq` transport, generic WebDriver session
   owner, timeouts, cancellation, limits, watchdog, ambiguous allocation and
   release handling, deterministic mock server tests.
4. Panel and MCP integration: `remote_target` on creation, target discovery,
   semantic actions with the scroll fallback, adaptive frames, coordinate
   mapping, per-session capability reporting, `browser_close`.
5. Provider compatibility: the same spike and contract tests against a
   self-hosted Appium endpoint and any second provider; adapter hooks only for
   demonstrated differences.
6. Evidence and documentation: setup examples, rotation, support matrix,
   measured latency, real-device smoke including provider-side release.


### Recovery after a panel disappears

`browser_remote_allocations` lists allocations owned by the calling agent in
its current workspace. `operation: reconcile` with a returned safe `reference`
checks the exact retired session using its original transport and authorization.
Settings > Remote browsers exposes the same operation for the host user.

The engine keeps private session identity in a shared `RemoteAllocation` handle,
including allocations rejected during startup. Driver completion makes the handle
eligible for a bounded asynchronous probe. Core's `RemoteAllocations` registry
associates each handle with its own quota lease and retained owner/workspace scope.
Board retirement and session switching retain the handle; no credentials or raw
provider session identifiers are written to the public request/result queue.

An exact invalid-session response frees that allocation's lease. Active sessions,
missing identity, rejected original credentials, outages, malformed responses,
and nonspecific HTTP 404s keep capacity held. Credential replacement never retargets
an old allocation to a new account or origin. Released entries remain idempotently
queryable in a bounded history of 128 records; unresolved records are never evicted.
Recovery does not reconstruct identities discarded by an older running binary.

## Provider usage across interfaces

The UI and the public `browser_provider_usage` MCP tool both use
`horizon_core::browser::remote_usage` for provider policy, credential snapshots,
API adaptation, and background reads. CLI plans call that same MCP tool. The host
serves requests with settings closed, validates the calling agent against its
live board, and returns only provider names, aggregate counts, sample timestamps,
and value-free failures. It neither allocates sessions nor reserves capacity.

`horizon-browser-control::manifest::provider_usage` owns the bounded private
host request/reply queue. The MCP controller transports the request and the UI
host bridge drives the shared usage model. These files are an internal transport;
agents must use the public MCP tool rather than reading the queue directly.
A standalone local-browser host has no configured remote-provider credentials,
so the tool reports that a live Horizon host identity is required.
