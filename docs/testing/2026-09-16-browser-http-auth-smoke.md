# HTTP Basic and Digest browser smoke

Temporary execution plan for [#717](https://github.com/peters/horizon/issues/717).
Another agent or machine can run it without extra context. Delete this file
after the UI/browser validation pass is complete unless asked to keep it.

## Scope

Prove that a live Horizon browser panel can complete HTTP Basic and Digest
authentication when credentials are supplied through the public MCP tool
`browser_http_auth`. That same tool is the CLI contract: `horizon-browser run`
plans and prompt jobs call it like any other `browser_*` tool.

Backends: Chromium and Firefox on Linux (this host). Safari is out of scope and
must keep returning `unsupported_backend`.

## Exact candidate

Run in the checkout and commit that will be pushed. Build once:

```bash
cargo build
```

Dirty trees are allowed only while iterating (`--allow-dirty`). Final evidence
must identify one exact HEAD.

## Fixture self-check

The loopback fixture serves `/basic-auth` and `/digest-auth` with username
`smoke-user` and password `smoke-pass-zephyr`. The smoke runner checks these
with `curl` before launching Horizon. Anonymous requests must not include
`authenticated-*-zephyr`.

## Lanes

On a task-owned display (existing `DISPLAY` or Xvfb plus a lightweight WM):

```bash
python3 scripts/browser-smoke/http_auth_smoke.py \
  --backend chromium \
  --horizon target/debug/horizon \
  --ephemeral

python3 scripts/browser-smoke/http_auth_smoke.py \
  --backend firefox \
  --horizon target/debug/horizon \
  --ephemeral
```

Each lane must:

1. Create a hidden panel through public MCP.
2. Navigate to `/basic-auth` with no credentials and leave the success marker
   absent (no hang).
3. `browser_http_auth` set with the wrong password, navigate, marker still
   absent.
4. Set the correct username/password and optional origin, navigate to
   `/basic-auth`, wait for `#auth-marker`, evaluate
   `authenticated-basic-zephyr`.
5. Navigate to `/digest-auth` with the same credentials, evaluate
   `authenticated-digest-zephyr`.
6. Clear credentials (typed success, `active: false`).
7. `browser_audit` contains `http_auth` entries and does not contain the
   password.
8. Normal window close, no surviving task-owned browser processes or live
   manifests.

Pass is `http-auth-result.json` with `"passed": true` on the exact HEAD.

## CLI check (same MCP tool)

Not a second API. Optional after the live lanes:

```bash
horizon-browser run crates/horizon-browser-cli/examples/http-auth.json
```

Plan variables carry username/password. The report must redact those fields.
This example expects a live panel and a matching fixture origin; the
unattended MCP smoke above is the required proof.

## Not in this plan

- In-panel username/password overlay
- Safari or remote WebDriver
- NTLM/Negotiate/proxy authentication
- Persisted OS-store credentials
