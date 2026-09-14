# Remote browser sessions on real mobile devices

Status: accepted for [#628](https://github.com/peters/horizon/issues/628) phase 1
on 2026-09-14, informed by the real-device evidence in
[`docs/testing/2026-09-14-remote-mobile-webdriver-spike.md`](../testing/2026-09-14-remote-mobile-webdriver-spike.md).
Nothing below is implemented yet; the phases at the end name the PRs that will
carry each piece. Provider credentials are separate from the routine login
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
  applications that choose their stores to `keyring-core`. No file store, no
  environment interpolation.
- Exported profiles carry providers, targets, limits and authentication
  references; machine-local `credential_bindings` are stripped, and whether a
  reference has a value on this machine is a live readiness query rather than
  part of the file. Import never overwrites an existing provider's endpoint or
  rebinds an existing reference to another origin without an explicit user
  confirmation.
- Existing files without `browser.remote` load unchanged and local backends keep
  their defaults. Parsing and readiness checks never allocate.

## Device identity and evidence

Requested versus actual identity is compared from provider session metadata or
an explicitly trusted self-hosted device map, never from a user-agent string or
viewport size. The spike showed a provider resolving `18` to `18.6`, so the
comparison is by version component prefix and the resolved version is shown.
Evidence is one of `physical`, `emulated` or `unknown`; a `physical` requirement
that resolves to anything else releases the session and reports the unmet
requirement instead of a ready panel.

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
  unique request id. Concurrency is enforced across Horizon instances that
  share the same configured provider identity through a private lock keyed by
  endpoint and username reference; provider quotas stay authoritative.
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
