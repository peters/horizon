---
applyTo: "crates/horizon-browser-mcp/src/remote.rs,crates/horizon-browser-mcp/src/remote/**,crates/horizon-browser-cli/tests/remote_parity.rs"
---

# Trust boundary of the remote browser bridge

A bridge gives the agents of this computer the profile host of another machine
through one SSH transport. The design is in the section "Remote host bridge" of
`crates/horizon-browser-cli/README.md` and in the module comment of
`crates/horizon-browser-mcp/src/remote.rs`.

## Trusted: the settings entry and the SSH target

Horizon writes the entry `browser_host` with an SSH target and an optional
profile name. A person does not edit it by hand.

- The entry refuses an SSH target that `ssh` reads as an option, that holds a
  space or a control character, and that is longer than 255 characters.
- The entry refuses a profile that is not 1 to 40 of `a-z`, `0-9` and `-` with a
  letter or a digit at each end, and a profile without an SSH target.
- The bridge uses the OpenSSH client of this computer. The host key of the
  target is the host key that OpenSSH checks, and the bridge adds no trust of
  its own.
- The bridge holds no credential. A key, an agent and an SSH config belong to
  OpenSSH and to the person.

## Untrusted: the remote host, its record and its results

The remote machine writes the record of its profile host and every result that
the bridge forwards.

- The bridge reads `profile-host.json` and the profile folder names over the SSH
  transport. It takes only `host_instance` from the record, and it accepts that
  value only through `manifest::valid_host_instance`.
- The bridge names the profile of a call only when the profile folder of the
  remote host has that name. It otherwise refuses with a message that lists the
  profiles that the host serves.
- A result is forwarded unchanged, except for the two rewrites. A capture is
  read only when the structured result names a `path`, and only up to 8 MiB.
- The bytes of a capture are the bytes that the page produced. The person reads
  them as data, not as a command.

## Untrusted: the files of this computer

An attachment call names files of this computer. The bridge stages them on the
remote machine.

- Only a path that `AttachmentPolicy` authorizes is staged: a regular file under
  the work root of the agent or under `HORIZON_BROWSER_ATTACHMENT_ROOTS`. A path
  outside those roots is refused before a byte leaves this computer.
- One file keeps the limit of 512 MiB, and one call stages at most 512 MiB.
- The staging folder of one call is `browser-staging/call-<pid>-<count>-<nanos>`
  under the remote runtime root. Its name is fixed by the bridge, so a value of
  a caller never becomes a part of a remote path or of a remote command.
- The bridge removes the folder of a call when its answer arrives, and it
  removes the folder when a staging step fails. A crash or a lost connection can
  leave one folder behind; it holds only copies that the person authorized in
  that call.

## What the bridge does not protect

- A staged file is readable on the remote machine for the duration of the call.
  The person trusts the remote machine with that file, as with any file that a
  person copies there.
- The remote host decides which pages an actor sees. The bridge forwards the
  actor of this computer, and it does not add a scope of its own.
- A process of the operating-system user on the remote machine can read the
  staged copies and the runtime root. Run the remote host as a user that
  untrusted processes can not become.
- The bridge does not limit how much output the remote server sends. It limits
  what it inlines, and it streams every line, so a large result stays the memory
  of one line.

## Lifecycle that a review must keep

- Without a named remote host the bridge does not start: the local entry of an
  agent stays the local server, byte for byte.
- The bridge starts one `ssh` process for the MCP stream, and it starts no
  listener.
- The remote host instance always comes from the record of the remote machine,
  never from `HORIZON_BROWSER_HOST_INSTANCE` of this computer.
