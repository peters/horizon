# Browser WebAuthn bridge

The WebAuthn bridge sends the passkey requests of a page in a browser panel to
an external authenticator command. The person configures the command. This
document gives the configuration, the command contract and the trust boundary.

## Configuration

The bridge is off by default. To turn it on, write the argv of the command in
`browser.webauthn_command`:

```yaml
browser:
  webauthn_command: ["/path/to/authenticator", "--profile", "work"]
```

- An empty list, or no key, keeps the bridge off. Horizon then adds no page
  script and changes no browser setting.
- Horizon starts the command without a shell. Horizon does not expand `~` or
  environment variables in the argv.
- The command gets the environment of Horizon. Its stderr goes to
  `/dev/null` or the Windows equivalent.
- A change to the key applies to browser sessions that start after the change.

## Supported panels

| Panel | Bridge | Channel to the page |
|---|---|---|
| Local Chromium | Yes | CDP `Runtime.addBinding` and `Page.addScriptToEvaluateOnNewDocument` |
| Local Firefox | Yes | WebDriver BiDi `script.addPreloadScript` with a `script.channel` |
| Safari | No | The browser keeps its own WebAuthn behavior. |
| Remote session (for example BrowserStack) | No | The browser keeps its own WebAuthn behavior. |

The `horizon-browser` CLI starts its standalone sessions with the default
browser configuration. These sessions do not read `~/.horizon/config.yaml`, so
the bridge is off in them.

## Data flow

1. A page calls `navigator.credentials.get()` or `navigator.credentials.create()`
   with a `publicKey` member.
2. The page script sends the options to the browser driver through the channel.
3. The driver gets the origin of the calling frame from the browser.
4. The driver applies the origin rules and builds `clientDataJSON`.
5. The driver starts the command on a worker thread and writes one JSON object
   to its stdin.
6. The command prints one JSON object on stdout and stops.
7. The driver examines the output and sends the credential, or an error, to the
   calling realm.
8. The page script resolves the promise with a `PublicKeyCredential`, or
   rejects it.

The panel chip shows **passkey request waiting** while the command runs.
The `browser_panel` and `browser_list` MCP results report
`passkey_pending: true` at the same time.

## Trust boundary

### Untrusted inputs

- **The page.** Page script can call the channel directly, with any message.
  It can also replace the page script functions in its own realm.
- **The command output.** The command is the choice of the person, but its
  output is parsed as untrusted data.
- **The agent name.** The `requester.agent` field comes from the coordination
  owner of the panel. An agent sets this name. It is information for the
  person, not an identity.

### What the bridge guarantees

- The host decides the origin. In Chromium, the driver maps the execution
  context of the binding call to its frame. Then it reads the
  `securityOrigin` of that frame and of each ancestor from `Page.getFrameTree`.
  In Firefox, the driver reads the origin of the source realm of
  `script.message` from `script.getRealms`. Then it reads the origin of each
  ancestor context of `browsingContext.getTree`.
- The page never supplies the origin, `clientDataJSON` or `crossOrigin`.
  The driver builds `clientDataJSON` with `type`, `challenge`, `origin` and
  `crossOrigin: false`, in this order.
- The driver refuses a request from a frame that does not have the same
  origin as each of its ancestors. The page gets `NotAllowedError`.
- The driver refuses an origin that is not `https://<domain>` or
  `http://localhost`. An IP address host is refused. The page gets
  `SecurityError`.
- The driver refuses an `rp_id` that is not the host or a parent domain of the
  host. A parent domain must have two labels or more. The page gets
  `SecurityError`.
- In Chromium, only the main world of the page session can use the binding.
  Isolated worlds and out-of-process iframes cannot send a request.
- Conditional mediation (`mediation: "conditional"`) never starts the
  command. `PublicKeyCredential.isConditionalMediationAvailable()` resolves
  `false`. A conditional request stays pending until the page aborts it.
- When the page aborts a request with its `AbortSignal`, the driver kills the
  command process. A navigation of the calling frame also kills it.
- One request runs at a time in a panel. The bridge starts the command a
  maximum of six times in 60 seconds for each panel.
- The driver examines the output before the page gets it:
  - The `authenticator_data` must start with the SHA-256 hash of the `rp_id`.
  - For `get`, a `credential_id` must be in `allow_credentials` when the page
    gave that list.
  - Each binary field must be bounded base64url.
- The page gets the same `NotAllowedError` message for a denial, an expiry, a
  missing credential, a timeout and a command failure. Only `excluded` on
  `create` becomes `InvalidStateError`.

### What the bridge does not guarantee

- Horizon has no public suffix list. A parent domain with two labels, for
  example `co.uk`, passes the `rp_id` rule. The command must keep each
  credential with its `rp_id`.
- The bridge does not check the signature, the attestation or the user
  verification flag. The relying party checks them.
- The page script is visible to the page. A page can find the bridge and can
  detect it.
- In Chromium, the bridge enables the CDP Runtime domain at attach. Without
  it, the binding does not survive a navigation to another process. Some sites
  can detect the Runtime domain.
- The bridge does not cover a popup that the page opens in a new top-level
  context or target.
- A cross-site iframe in Chromium runs in another process. It keeps the
  browser's own WebAuthn behavior and never reaches the command.
- When the driver kills the command, child processes of the command can
  continue.

## Command contract

The command reads one JSON object from stdin and writes one JSON object to
stdout. All binary values are base64url without padding.

### Input

| Field | Operation | Value |
|---|---|---|
| `version` | both | `1` |
| `op` | both | `"get"` or `"create"` |
| `rp_id` | both | The relying party ID after the origin rules |
| `origin` | both | The origin of the calling frame |
| `client_data_json` | both | The `clientDataJSON` that the driver built. Sign its SHA-256 hash. |
| `user_verification` | both | `"required"`, `"preferred"` or `"discouraged"` |
| `reason` | both | A sentence for the person, for example `Sign in to example.com in a browser panel` |
| `requester` | both | `{"app": "Horizon"}`, with `"agent"` when an agent owns the panel |
| `allow_credentials` | get | The credential IDs that the page allows. Empty means any. |
| `rp_name` | create | The name of the relying party |
| `user` | create | `{"id", "name", "display_name"}` |
| `algorithms` | create | COSE algorithm IDs in page order. The default is `[-7, -257]`. |
| `exclude_credentials` | create | The credential IDs that must not exist |
| `resident_key` | create | `"discouraged"`, `"preferred"` or `"required"` |

### Output

A successful `get`:

```json
{"version": 1, "status": "ok", "credential_id": "<b64u>",
 "authenticator_data": "<b64u>", "signature": "<b64u>", "user_handle": "<b64u>"}
```

A successful `create`:

```json
{"version": 1, "status": "ok", "credential_id": "<b64u>",
 "attestation_object": "<b64u>", "authenticator_data": "<b64u>",
 "public_key": "<b64u SPKI>", "public_key_algorithm": -7, "transports": ["internal"]}
```

A failure:

```json
{"version": 1, "status": "denied", "message": "The person denied the request."}
```

| Status | Page error |
|---|---|
| `denied`, `expired`, `not_found`, `error` | `NotAllowedError` |
| `excluded` (on `create`) | `InvalidStateError` |

The driver writes the `message` to the debug log only. The page never gets it.

### Limits

| Limit | Value |
|---|---|
| Time for one request | The page `timeout`, from 10 to 120 seconds. 120 seconds when the page gives none. |
| Command stdout | 256 KiB |
| Page message | 192 KiB |
| Challenge | 1 to 1024 bytes |
| Credential ID | 1 to 1023 bytes |
| User ID | 1 to 64 bytes |
| Credential lists | 64 entries |

When the time limit ends, the driver kills the command and the page gets
`NotAllowedError`.

## The page credential

The page gets an object with the `PublicKeyCredential` prototype. It has
`id`, `rawId`, `type`, `response`, `authenticatorAttachment` (`"platform"`),
`getClientExtensionResults()` and `toJSON()`. The response of `create` has
`getAuthenticatorData()`, `getPublicKey()`, `getPublicKeyAlgorithm()` and
`getTransports()`.

## Tests

- Unit tests in `crates/horizon-browser/src/webauthn/` cover the origin rules,
  the command contract and the process lifecycle.
- `crates/horizon-browser/tests/webauthn_live.rs` runs a local relying party
  and a fake authenticator with a test P-256 key in Chromium and Firefox. It
  runs only with `--ignored`.
- The [test procedure](../testing/procedures/browser-passkey-bridge.md) gives
  the manual smoke.
