---
name: horizon-net
description: Configure or verify the standalone Horizon Net agent for explicitly authorized private TCP service tests.
---

# Standalone Horizon Net

Use this skill with the standalone `horizon-net` library and agent. Read the
[crate guide](../../README.md) for configuration and API requirements. Use the
[data plane procedure](../../../../docs/testing/procedures/horizon-net-data-plane.md)
for local, provider and cleanup evidence.

1. Confirm the installed candidate version and supported platform. Persistent
   state requires Unix directory durability. Windows and other non-Unix
   platforms return I/O `Unsupported` before state ownership or mutation. Use
   `Agent::bind` and `Controller::new` for trusted in-memory operation there.
   These APIs deny remote topology updates. Caller-managed Windows
   configuration must have user-only ACLs. The crate does not validate them.
2. Use an explicit self-hosted HTTPS relay on TCP port 443. Do not substitute a
   public relay. Set `relay_only: true` for a TCP-only host.
3. Keep endpoint secret keys in user-owned private files. On Unix, use mode
   `0600` for configuration and `0700` for the state directory.
4. Run `horizon-net agent --config /absolute/path/config.json` for an already
   authorized enrollment. Keep its committed `config.state` directory when it
   restarts. Each persistent restart denies service access until the pinned
   authority confirms the exact retained topology or an accepted newer one.
   Check `policy_state: awaiting_confirmation`; grants stay inactive in that state.
   A visible snapshot does not prove a successful previous update.
   Do not remove state to bypass a withdrawal, pending confirmation or key mismatch.
5. Name each TCP service and its destination node. Give each grant an absolute
   finite expiry. A caller cannot choose a different backend address or port.
6. Keep the embedding application's `Forwarder` alive for its listener lifetime.
   A revoked grant closes existing connections and denies new connections.
7. Use only configured relays in destination endpoint addresses. An additional
   relay fails before a connection or forwarding listener starts.
8. Keep all persistent controller owners until their work ends. Drop the agent
   and its controller clones before reopening the same state directory.
   `Controller::new` has no durable storage. An embedded host must save its
   policy before it calls `apply` or `revoke`. Use a persistent agent's
   controller for remote policy changes and restart storage. A trusted host can
   confirm the retained exact policy with an identical `apply` plan. The call
   repeats durability barriers before it enables service access; `changed` stays
   false. A failed retry keeps access denied.
9. Treat reachability and remote acknowledgement as separate evidence. A saved
   topology does not prove that a remote endpoint accepted it.

`horizon-net relay-config --config relay.json` prints upstream relay TOML. It
does not install a relay or authorize its allocation. Obtain authorization for
provider resources, operator service changes and new enrollments before use.
Preserve uncertain resource identities and verify deletion through the provider.

This standalone slice does not provide the workspace UI, CLI plan runner or
public network MCP tools. Do not invent an approval flag or treat a library
`Controller::apply` call as a person's UI confirmation.
