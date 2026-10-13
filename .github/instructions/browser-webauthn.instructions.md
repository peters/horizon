---
applyTo: "crates/horizon-browser/src/webauthn.rs,crates/horizon-browser/src/webauthn/**,crates/horizon-browser/src/session/webauthn.rs,crates/horizon-browser/src/webdriver/session/webauthn.rs,crates/horizon-browser/tests/webauthn_live.rs,crates/horizon-browser/tests/webauthn_live/**"
---

# Trust boundary of the browser WebAuthn bridge

This code sends the passkey requests of a page in a browser panel to an
external authenticator command. `docs/architecture/browser-webauthn-bridge.md`
gives the design, the command contract and the full boundary.

## Untrusted: the page and the command output

- Page script can send any message through the channel and can replace the
  page script in its own realm. The page message is untrusted.
- The output of the command is untrusted data, with size limits.
- The origin, `clientDataJSON` and `crossOrigin` must come from the host:
  the CDP frame tree of the binding's execution context, or the `BiDi` realm
  and context tree of `script.message`. Never from a page message.

## Rules that must hold

- A frame that does not have the same origin as each ancestor is refused.
- Only `https://<domain>` and `http://localhost` origins are served.
- The `rp_id` is the host or a parent domain with two labels or more.
- Conditional mediation never starts the command.
- An abort from the page, a navigation of the calling frame, the time limit
  and the end of the session kill the command process.
- The bridge is off unless `browser.webauthn_command` names a command. When it
  is off, no page script and no extra CDP domain is installed.
- The page gets the same `NotAllowedError` for a denial, a missing credential,
  a timeout and a failure.

## Deliberate limits: do not report

- There is no public suffix list. A two-label parent such as `co.uk` passes.
- The host does not verify signatures, attestation or user verification.
  The relying party does.
- A page can detect the page script and, in Chromium, the Runtime domain.
- A cross-site iframe in Chromium keeps the browser's own WebAuthn behavior.
- Popups in a new top-level context are not covered.
- Child processes of the command can outlive a kill.

## Report

- A path where a page value decides the origin, the `rp_id` check or
  `crossOrigin`.
- A refused request that still starts the command.
- A command process that continues after an abort, a navigation, the time
  limit or the end of the session.
- A page message or command output that the host reads without a size limit.
