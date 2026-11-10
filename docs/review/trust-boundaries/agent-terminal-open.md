---
applyTo: "crates/horizon-browser-control/src/manifest/block_terminal.rs,crates/horizon-core/src/ssh/mod.rs,crates/horizon-ui/src/app/block_terminals.rs,crates/horizon-ui/src/app/block_terminals/**"
---

# Trust boundary of an agent-opened SSH terminal

An agent asks Horizon to open an SSH terminal panel for the person with the
`open_terminal` operation of `block_panel`. Horizon builds the `ssh` command
line itself, so the agent chooses a host and a command, and nothing else. The
module comment of
[`block_terminal.rs`](../../../crates/horizon-browser-control/src/manifest/block_terminal.rs)
and the comment of
[`block_terminals.rs`](../../../crates/horizon-ui/src/app/block_terminals.rs)
describe the same boundary.

## Untrusted: every field from the agent

`open_terminal` takes `ssh.host`, `ssh.user`, `ssh.port`, `remote_command` and
`title` from an agent.

- **The route holds no free field.** `ssh` is an object with `host`, `user` and
  `port` and `deny_unknown_fields`, so an identity file, a proxy, an ssh option
  and an extra argument are refused as a request that Horizon does not read.
- **A label is a label.** `host` and `user` are trimmed and checked with the same
  code as the route of a Device viewer (`SshRoute::normalize`): letters, digits,
  `.`, `_` and `-`, plus `:` in `host` for an IPv6 address, at most 253 and 64
  characters, never a leading `-`, and no whitespace or control character. A
  value that breaks a rule is `invalid_ssh_route`, and the message names the
  field.
- **A port is a port.** `port` is 1 to 65535. Zero, a negative number and a
  larger number are `invalid_ssh_route`, because the route is read as a wide
  integer before it becomes a port.
- **The command is one plain line.** `remote_command` is trimmed, 1 to 512
  characters, and holds only letters, digits, spaces and `. _ : / , = @ + -`. It
  cannot start with `-`, so it never reads as an ssh option. A quote, `;`, `&`,
  `|`, `$`, a backtick, `<`, `>`, `(`, `)`, a backslash, a newline or any other
  control character is `invalid_remote_command`, and the message names the field
  and the character.
- **The check runs twice.** The MCP server refuses the route and the command
  before the request reaches the queue, and the UI checks them again before it
  changes the board.

## Horizon guarantees the command line

- **The shape is fixed.** `SshConnection::for_agent_terminal` copies the three
  route fields and the checked command into one `SshConnection` that carries no
  identity file, no proxy jump and no extra argument beyond the `-t` that
  Horizon adds. Tests hold the argv: `-p <port>`, the two keepalive options,
  `-t` when a remote command is present, `user@host` as one argument, and the
  command last as one argument.
- **`-t` is Horizon's decision.** It is added only when `remote_command` is
  present, because `ssh host command` allocates no terminal. An agent cannot ask
  for another ssh option.
- **No request text becomes a shell word on this computer.** Horizon runs
  `ssh` with an argument vector and no local shell. The command reaches the
  remote shell as the single argument above, and the character set keeps it
  from being a second command or a substitution there.
- **Workspace scope.** Only an agent panel that Horizon launched, in the
  workspace that holds it, reaches `open_terminal`; a caller without that panel
  is `caller_unavailable`, and a request of another host is `wrong_host`. The
  panel goes beside the calling agent panel in that workspace.
- **The bound and the focus.** One agent panel opens at most eight terminals in
  one Horizon run (`too_many_terminals`); closing a terminal does not free its
  place, and the count is not persisted. The new panel does not take keyboard
  focus, and a failure to create it is `terminal_unavailable`.
- **The mark is Horizon's text.** The titlebar shows the chip `opened by <agent>`
  from the display name of the panel kind that Horizon launched, never from a
  request field. It is saved with the panel as `opened_by` and comes back after
  a restart.
- **The panel is then the person's.** No operation of `block_panel` reads,
  writes to or closes the terminal. The agent cannot type into it, and it has no
  card document and no `next_click`.

## Does not guarantee

- **The host is the agent's choice.** The agent selects the machine, and the
  person's OpenSSH configuration and known-host entries decide whether Horizon
  reaches it. Horizon trusts no host key and accepts no key, fingerprint or
  option from an agent.
- **The host key question and the login are the person's.** `ssh` runs in the
  new panel, so an unknown host key and a password prompt wait for the person in
  that terminal. Horizon answers neither, and it does not prove that a login
  succeeded.
- **The remote command is the agent's choice.** The character set keeps it from
  being a second command in this shell, but it still runs on that host with the
  account that the person logs in as. A command such as `rm -rf` is a valid
  plain line.
- **What the person types in the panel goes to that host.** Horizon reads the
  terminal to draw it and writes nothing into it for this operation; the text
  the person types leaves this computer. Horizon does not inspect it.
- **The mark says who opened the panel, not who controls it.** A terminal with
  the mark behaves as any other SSH panel, and a person may find it later with
  no agent present.
- **A restored panel keeps the saved mark.** The mark is the device name that
  Horizon wrote into the saved panel state. A snapshot that a person edits by
  hand can carry another name, and it is not a security property.
