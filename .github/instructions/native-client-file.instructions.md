---
applyTo: "crates/horizon-app-host/src/bootstrap.rs,crates/horizon-app-host/src/bootstrap/**,crates/horizon-app-runtime/src/account.rs"
---

# Trust boundary of the native client file

`horizon-native --mcp --client <file>` reads its bindings from a private client
file. The client file can name private credential files, so that the host reads
the provider credentials from files and not from the OS keyring. The design is in
the section "A host with credential files" of
`docs/architecture/remote-device-testing.md`.

## Trusted: the client file and the credential files

The operator writes these files.

- The client file names one absolute credential file for each authentication
  reference of the provider.
- The host opens each credential file with no link in any part of its path. The
  file must be a regular file of the effective user, with no group or other
  permission bits, and at most 4 KiB. Its value must fit the scheme.
- A missing or an extra reference, a relative path and a provider with
  `credential_bindings` in the Horizon configuration cause a refusal before the
  host serves a tool.
- With credential files, the host never opens the OS keyring.
- No answer to the MCP client contains a credential value or a credential file
  path.

## Untrusted: the MCP client

- It can not supply a credential, a credential file, an endpoint or a tunnel
  binary. Tool arguments can not replace the bindings of the client file.

## Not protected

- A process of the same operating-system user can read the credential files.
  The MCP client usually runs as that user.
