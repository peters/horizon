# Remote browser orientation

Remote devices can start in portrait or landscape and rotate during a session.
Target configuration, UI buttons, MCP and CLI use the same provider adaptation,
credential policy, driver verification and lifecycle. This document describes
the feature introduced by [#1182](https://github.com/peters/horizon/issues/1182).
Qualification results belong to the exact candidate's PR; this document is not
evidence that a particular device, provider or operating system passed.

For provider setup and cleanup, see [remote browser sessions](remote-browser-sessions.md).
For repeatable qualification and future changes, use the permanent
[orientation test procedure](../testing/remote-browser-orientation-smoke.md).

## Select the starting orientation

Add the optional field to an existing target in `browser.remote.targets`:

```yaml
tablet_demo:
  provider: device_cloud
  browser_name: safari
  platform_name: iOS
  device: { kind: physical, model: configured-tablet-model, os_version: "18" }
  orientation: landscape
```

Use the real model and OS version from the provider's catalog. Provider
credentials remain in existing bindings, separate from target configuration.
Export/import preserves orientation and strips machine-local credential bindings.
Old configurations without the field keep their previous provider defaults.

A create call can override the target for this session without changing it:

```json
{"target":"tablet_demo","orientation":"portrait","url":"https://example.org/"}
```

The same `browser_create` option works with an opaque catalog target returned by
`browser_provider_devices`. Discover the target rather than supplying raw driver
capabilities. Orientation requires a remote target; a local create with an
orientation override is invalid. Supported values are exactly `portrait` and
`landscape`.

The shared adapter maps the normalized field as follows:

| Adapter | Allocation capability |
| --- | --- |
| `browserstack` | `bstack:options.deviceOrientation`, lower case |
| `webdriver` / Appium | `appium:orientation`, upper case |

Orientation keys in capability extensions are rejected to prevent competing
settings. Multiple provider profiles and credential sets retain their existing
scope. Selecting or validating a target does not allocate a device.

## Readiness and observed status

Allocation uses a bounded orientation GET to discover endpoint support. It does
not measure or apply orientation from the provider's temporary page, which can be
unmeasurable or have different geometry. The driver publishes unknown applied
orientation before startup navigation and measures the committed document before
publishing `Ready` for an explicit start orientation.

An explicit configured orientation or create override must match the device,
inner viewport and visual viewport geometry. Nonzero, nonsquare geometry must
have the requested width/height relationship; a reported screen orientation
must also agree. If the first page is still pending at the bounded startup
navigation wait, failed, unsupported, contradictory or cannot be measured, create
returns a typed error and attempts to release that exact session. It does not
publish a ready panel that might become correct later. Cancellation also releases
the allocation. Release uncertainty stays visible and retains its allocation
record for reconciliation.

Without an explicit orientation, unavailable geometry remains nonfatal. The
existing `navigation: pending` create behavior remains available; inspect
`browser_panel` or wait for the page before acting. A new document never inherits
verified orientation from its predecessor. Navigate, reload and history commands
publish cleared status before entering the transport; observed document changes
also invalidate it until a new measurement completes.

| Public field | Meaning |
| --- | --- |
| `orientation_support: supported` | The endpoint returned a valid device orientation. Applied orientation may still be unknown. |
| `orientation_support: unsupported` | The endpoint explicitly rejects the orientation operation. |
| `orientation_support: unverified` | Support could not be established, including transient or malformed replies. |
| `remote_orientation: portrait` or `landscape` | Device and measured current-document geometry agree. |
| `remote_orientation` omitted | No current applied orientation is verified. |

Support and applied orientation are separate facts. `supported` alone is not a
successful rotation. Viewport shape alone cannot establish physical device
identity or override contradictory device evidence.

## Rotate through UI, MCP or CLI

Remote panel chrome has **Portrait** and **Landscape** buttons. Selection reflects
measured applied orientation, with **Rotating…**, **Verified**, **Unverified** or
**Unsupported** status. Controls wrap in narrow panels and are disabled during
startup, shutdown, pending rotation, Teach mode or explicit lack of support.
Local panels do not show them. A failed rotation leaves a nonfatal message and
keeps the usable browser frame.

The equivalent public MCP operation is:

```json
{"panel_id":"<owned-panel-id>","orientation":"landscape","timeout_millis":15000}
```

Call `browser_orientation` with that input. The timeout is 1–60000 ms and defaults
to 15000 ms. Success contains `panel_id`, `action_id`, `requested`, `applied` and
`viewport: [width, height]` measured in CSS pixels. `requested` and `applied` must
agree. Read support/applied state through `browser_panel` or `browser_list`.

The direct CLI operation uses the same public operation through the durable
MCP plan runner:

```bash
horizon-browser orientation <owned-panel-id> landscape \
  --timeout-millis 15000 --output orientation-result.json
```

Use `--output -` for standard output. Create-time orientation is also available
in CLI plans:

```json
{
  "version": 1,
  "steps": [
    {
      "id": "create",
      "tool": "browser_create",
      "arguments": {"target": "tablet_demo", "orientation": "landscape"}
    }
  ]
}
```

These interfaces require a live matching Horizon host with the configured
provider binding. The settings window need not be open. Older hosts cannot
silently ignore the new create contract. A cloud-hosted panel uses the same
orientation state and shared provider policy; cloud status retains bounded
per-request completion so another viewer or a later poll can see the outcome.

## Verification, input and failure handling

Runtime success requires matching device and page geometry, a fresh frame, the
same document and current event-loop ownership. A request for the already applied
orientation still requires measured acknowledgement. Human requests retain
priority while pending; page input, replacement requests, document changes or
ownership changes can supersede an acknowledgement. The caller must not treat a
late or different request's result as its own.

Rotation invalidates semantic refs, scroll geometry and native-select coordinates.
Take a fresh `browser_snapshot` or `browser_query` before the next element action.
Semantic input resolves fresh element and visual-viewport coordinates. Host
panel resize/fit changes presentation rather than physical device dimensions.
Remote `browser_resize` continues to return `remote_viewport_fixed`; rotation is
not an arbitrary viewport size operation. Manual screenshot-coordinate steering
on remote devices remains unsupported.

| Failure | Caller action |
| --- | --- |
| `orientation_unsupported` | Use the reported endpoint capability; do not substitute resize or allocate a helper session. |
| `orientation_unverified` | Inspect support, page and device state. An explicit start is rejected and release is attempted. |
| `remote_orientation_mismatch` | The explicit start did not match; inspect its release outcome before creating again. |
| `orientation_timeout` | The device may have rotated. Inspect before retrying; timeout is not rollback. |
| `browser_unavailable` | The session stopped or startup was cancelled; inspect exact allocation cleanup. |

Other ownership, stale-ref and navigation failures retain their existing typed
contracts. Unknown allocation/release outcomes must be reconciled using their
returned safe reference through `browser_remote_allocations`; do not blindly
create again. Audit preserves user/agent identity and redacts credentials and
raw provider session identity.

## Maintenance boundaries

Normalized options and presentation types live in
[`horizon-browser-protocol`](../../crates/horizon-browser-protocol/src/remote/orientation.rs).
Provider mapping is shared with remote configuration. The driver's
[`orientation` module](../../crates/horizon-browser/src/webdriver/session/orientation.rs)
owns request servicing, document invalidation, acknowledgement and startup
rejection. Desktop and cloud adapters consume those results; UI rendering must
not implement another provider policy. MCP and CLI must not add a separate
driver controller.

For any change, update the relevant deterministic regressions and run the
change-to-test matrix in the [test procedure](../testing/remote-browser-orientation-smoke.md).
Changes to verification, input geometry, provider mapping, ownership or UI
behavior require fresh physical-device evidence on the final head. Keep the
candidate SHA, executable hashes, device/OS evidence, actual measurements,
recording and release result together. A test on an earlier head does not qualify
changed behavior.
