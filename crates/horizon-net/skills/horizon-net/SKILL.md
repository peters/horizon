---
name: horizon-net
description: Configure or verify the standalone Horizon Net agent for explicitly authorized private TCP service tests.
---

# Standalone Horizon Net

Use this skill with the standalone `horizon-net` library and agent. Read the
[crate guide](../../README.md) for configuration and API requirements. Use the
[data plane procedure](../../../../docs/testing/procedures/horizon-net-data-plane.md)
for local, provider and cleanup evidence.

1. Confirm the installed candidate version and supported platform.
2. Use an explicit self-hosted HTTPS relay on TCP port 443. Do not substitute a
   public relay. Set `relay_only: true` for a TCP-only host.
3. Keep endpoint secret keys in user-owned private files. On Unix, use mode
   `0600` for configuration and `0700` for the state directory.
4. Run `horizon-net agent --config /absolute/path/config.json` for an already
   authorized enrollment. Keep its committed `config.state` directory when it
   restarts. Do not remove state to bypass a withdrawal or key mismatch.
5. Name each TCP service and its destination node. Give each grant an absolute
   finite expiry. A caller cannot choose a different backend address or port.
6. Keep the embedding application's `Forwarder` alive for its listener lifetime.
   A revoked grant closes existing connections and denies new connections.
7. Treat reachability and remote acknowledgement as separate evidence. A saved
   topology does not prove that a remote endpoint accepted it.

`horizon-net relay-config --config relay.json` prints upstream relay TOML. It
does not install a relay or authorize its allocation. Obtain authorization for
provider resources, operator service changes and new enrollments before use.
Preserve uncertain resource identities and verify deletion through the provider.

This standalone slice does not provide the workspace UI, CLI plan runner or
public network MCP tools. Do not invent an approval flag or treat a library
`Controller::apply` call as a person's UI confirmation.
