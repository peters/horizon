---
applyTo: "crates/horizon-core/src/cloud_runtime/claude_token.rs,crates/horizon-core/src/cloud_runtime/setup/**,crates/horizon-core/src/cloud_runtime/settings.rs,crates/horizon-core/src/cloud_runtime/deployment/agent_credentials.rs,crates/horizon-core/src/cloud_runtime/deployment/write_agent_auth.py,crates/horizon-core/src/cloud_runtime/deployment/clear_agent_auth.py,crates/horizon-ui/src/app/cloud_panel/production/setup/**,examples/cloud-worker/horizon-worker-run,examples/cloud-worker/horizon-worker-session-env"
---

# Claude setup token boundary

Read "Claude with a setup token" in `docs/cloud-workspaces.md` for the guarantee and
limits. This computer and its private credential files are trusted. The worker is
trusted with the token only while Claude is bound with it.

Report these failures:

- The token value reaches `settings.json`, debug output, a log, a progress line, the
  cloud board, a command argument, an environment of a host process or a test fixture
  other than a synthetic value.
- Settings save or load a Claude API key and a setup token together, or a choice other
  than **Setup token** leaves the token bound.
- A reconnect with another Claude sign-in leaves `/workspace/credentials/claude-oauth-token`
  on the worker, or a reconnect removes it while the token stays bound.
- The token reaches a worker whose profile does not permit Claude.
- An image layer's session environment can set `CLAUDE_CODE_OAUTH_TOKEN`.
- The expiry is not counted from the saved date, or the docs claim that Horizon knows
  the real expiry or checks the token.
