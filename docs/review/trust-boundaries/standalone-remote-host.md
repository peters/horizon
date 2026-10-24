---
applyTo: "crates/horizon-browser-cli/src/standalone/remote.rs,crates/horizon-browser-cli/src/standalone/remote/**,crates/horizon-browser-cli/tests/standalone_remote.rs"
---

# Trust boundary of the standalone remote host

`horizon-browser mcp --standalone --config <host file>` serves remote targets over
MCP without Horizon. The design is in the section "Standalone remote host" of
`docs/architecture/remote-browser-sessions.md`.

## Trusted: the host file and the credential files

The host file decides which endpoint gets which credential.

- The host refuses a host file that the group or other users can write, and a
  credential file that the group or other users can read or write.
- Each credential file holds one value. The host reads it each time it forms a
  header, so a changed or removed file applies to the next use.
- No answer to the MCP client contains a credential value or a credential file
  path. Only the operator's log names the path.

## Untrusted: the MCP client

- It can name a target and use the public tools. It can not supply an endpoint,
  a capability or a credential.
- The host claims only the requests of its own agent identity and host
  instance.
- The host starts no local browser: a create without a target fails.

## Not protected

- Another process of the same operating-system user can read the credential
  files and the runtime root. Run the host as a user that the MCP client can not
  become.

## Lifecycle that a review must keep

- A hold is saved before its driver starts, and its files go only after the
  release is confirmed, by the driver or by the exact-session check at the
  provider.
- A close answers `closed` only for a confirmed release; otherwise it answers
  `release_failed` or `release_unknown` and keeps the provider slot.
- A host takes over only the hold folders whose lock no live host holds.
