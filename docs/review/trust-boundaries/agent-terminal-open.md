---
applyTo: "crates/horizon-browser-control/src/manifest/block_terminal.rs,crates/horizon-browser-control/src/manifest/block_keys.rs,crates/horizon-browser-control/src/manifest/block_machines.rs,crates/horizon-core/src/ssh/mod.rs,crates/horizon-core/src/ssh/machine_keys.rs,crates/horizon-core/src/ssh/machine_keys/**,crates/horizon-core/src/machines.rs,crates/horizon-core/src/machines/**,crates/horizon-ui/src/app/block_terminals.rs,crates/horizon-ui/src/app/block_terminals/**,crates/horizon-ui/src/app/block_machine_keys.rs,crates/horizon-ui/src/app/block_machine_keys/**,crates/horizon-ui/src/app/block_machines.rs,crates/horizon-ui/src/app/block_machines/**,crates/horizon-ui/src/app/machine_open.rs,crates/horizon-ui/src/app/machine_open/**,crates/horizon-ui/src/app/machine_click.rs,crates/horizon-ui/src/app/machine_click/**,crates/horizon-ui/src/app/sidebar/machines.rs,crates/horizon-ui/src/app/sidebar/machines/**"
---

# Trust boundary of an agent-opened SSH terminal and of a machine record

An agent asks Horizon to open an SSH terminal panel for the person with the
`open_terminal` operation of `block_panel`. Horizon builds the `ssh` command
line itself, so the agent chooses a host, a command and, when it wants one, the
name of a key that Horizon keeps. The module comment of
[`block_terminal.rs`](../../../crates/horizon-browser-control/src/manifest/block_terminal.rs)
and the comment of
[`block_terminals.rs`](../../../crates/horizon-ui/src/app/block_terminals.rs)
describe the same boundary. The key half is described by
[`block_keys.rs`](../../../crates/horizon-browser-control/src/manifest/block_keys.rs),
[`machine_keys.rs`](../../../crates/horizon-core/src/ssh/machine_keys.rs) and
[`block_machine_keys.rs`](../../../crates/horizon-ui/src/app/block_machine_keys.rs).

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

## Machine records

An agent also writes a **machine record** with `machine_set`: a name, a route, a
key name, a tmux session and a provider line. The person then connects to that
machine with one click in the sidebar, without an agent, so the record is the
input of a click. It is stored as one file per Horizon home,
`machines/registry.json`.

### Untrusted: every field of a record

- **The name is a key label.** It is 1 to 40 characters of `a-z`, `0-9` and `-`,
  with no `-` at either end, so the record and its key pair always have one
  name. A violation is `invalid_machine_name`.
- **The route is the route of a terminal.** `ssh` is checked with
  `block_terminal::check`, so the host, the user and the port rules of
  `open_terminal` apply and a violation is `invalid_ssh_route`.
- **The tmux session is one word.** `session` is 1 to 40 characters of `A-Z`,
  `a-z`, `0-9`, `_` and `-`, and it cannot start with `-`. Horizon builds
  `tmux new -A -s <session>` itself from that name, so a space, a quote, a `;`
  or a leading `-` never reaches the command. A violation is
  `invalid_session_name`.
- **The key name is a key name.** It passes the key rule
  (`invalid_key_name`) and its file must be there (`key_unavailable` when it is
  not).
- **The provider line is a claim.** It is folded to one line and cut to 60
  characters. Horizon draws it in a low-emphasis style under the machine name. It
  is never read by a program and never becomes a command, a path or a title.
- **No command line is saved.** A record holds no argv and no remote command.
  Horizon builds both from the record when a panel opens.

### Guarantees of a record and of a click

- **The click re-validates.** At click time Horizon checks the record again with
  the same rules (`Machine::checked_terminal`) and re-derives the key path from
  the key name on the coordination worker. A record that no longer passes a rule
  opens nothing, and the sidebar says the typed reason.
- **The argv is Horizon's.** A click runs `ssh` with the argv of
  `SshConnection::for_agent_terminal_with_key`: the key of the record, the two
  keepalive options, `-p` when the record has a port, `-t` when it has a session,
  `user@host` as one argument and `tmux new -A -s <session>` last. A hand-edited
  registry cannot add an ssh option, a proxy, an identity file or a second
  command.
- **The list is private to the person.** It is one file of the Horizon home, in a
  `0700` directory, written as `0600` through a temporary file and a rename, on
  the coordination worker. It is not part of a session or a workspace.
- **The record is not a reachability claim.** `terminal_open` reports whether a
  terminal of the machine is open now. Horizon never polls the machine, and the
  row says `no terminal open` rather than `offline`.

### Does not guarantee

- **The host is the agent's choice.** A click connects to the host that an agent
  named, with the account in the record. Horizon does not verify that the host is
  the one the agent meant, and first contact with its host key is accepted and
  pinned for that key.
- **A record is agent text about a machine.** The provider line is the agent's
  reading of its CLI, and the record outlives the agent panel that wrote it. A
  person who keeps a record keeps an agent's claim about a host.
- **A forgotten record does not close a terminal.** `machine_delete` forgets the
  record and keeps the key. A terminal that the machine opened keeps its own saved
  connection, reconnects after a restart and is not tied to the record any more.
- **Every agent panel of this Horizon reaches every machine key.** The keys
  belong to the Horizon home, so the workspace boundary is not a key boundary.
  An agent that the person runs in another workspace of the same Horizon reaches
  the same machines by name.

## Machine keys

A person's ssh agent can hold more keys than a server allows: 18 of 18
authentication attempts failed at three providers when `ssh` offered the agent's
keys, and `-o IdentitiesOnly=yes -o IdentityAgent=none -i <key>` logged in at
once. A rented machine must also not receive the person's own key. Horizon
therefore keeps its own key pair for each machine name, and an agent names the
key instead of supplying one.

### Untrusted: the name of the key

`machine_key` and `delete_machine_key` take a `name`, and `open_terminal` takes
an optional `key` with the same rule. `machine_keys` takes no input.

- **A name is a name.** It is 1 to 40 characters of `a-z`, `0-9` and `-`, with
  no `-` at either end, so it is one file name in the key directory and never a
  path, a leading `-` or another file of that directory. A violation is
  `invalid_key_name`, and the message names the rule.
- **The check runs twice.** The MCP server refuses a bad name before the
  request reaches the queue, and the UI checks the name again when it uses it.
- **A name belongs to the Horizon home.** The keys are the person's and not a
  workspace's, in `machine-keys/<name>`, so every agent panel of this Horizon
  may list, use and delete any of them by name. A workspace boundary is
  therefore not a key boundary: an agent that a person runs in another
  workspace of the same Horizon reaches the same machines.

### Guarantees for machine keys

- **Fixed argv.** With `key`, Horizon adds exactly `-i <private key path>`,
  `-o IdentitiesOnly=yes`, `-o IdentityAgent=none`,
  `-o UserKnownHostsFile=<machine-keys dir>/<name>.known_hosts` and
  `-o StrictHostKeyChecking=accept-new` to the command line that it builds. The
  key options come before the destination and a remote command stays last. The
  agent never passes a path, a passphrase, a key file or an ssh option.
- **The person's own keys and agent are not offered.** `IdentitiesOnly` and
  `IdentityAgent=none` keep the login to that one key, so a long agent key list
  cannot exhaust `MaxAuthTries`, and a rented machine never holds the person's
  public key.
- **A per-key known-hosts file.** First contact with a host is accepted and
  pinned for that key alone. A recycled address therefore cannot collide with
  the person's own `known_hosts`, and a changed host key refuses that key's
  login.
- **The private half stays in the Horizon home.** No answer, log line, card or
  error message holds the text of the private key. The answer names the two
  files, `private_key_path` and `known_hosts_path`, so an agent can run its own
  `ssh` for the commands that set the machine up; the key directory is `0700`
  and the private key `0600`, and `ssh-keygen` makes an ed25519 pair with no
  passphrase. One workspace holds at most 32 pairs (`too_many_keys`).
- **Workspace scope, not agent scope.** Every agent panel of the workspace may
  use the workspace's keys, so an agent that starts later reaches a machine that
  an earlier agent rented; an agent of another workspace cannot.
- **No blocking frame.** The name rule runs on the frame; the directory work
  and `ssh-keygen` run on the coordination worker, and the answer is written
  when that work is done.
- **The saved panel keeps the name.** The key name and its directory are saved
  with the SSH panel and restored with it, so a restored panel builds the same
  command line. A restored panel whose key was deleted starts and shows the
  error of `ssh`.

### Does not guarantee (machine keys)

- **The agent gets the path and may run its own connection with it.** An agent
  panel runs as the same local user as Horizon, so the private key is a file that
  it can read. `machine_key` and `machine_keys` therefore answer
  `private_key_path` and `known_hosts_path`: the agent may run `ssh` and `scp`
  with `-i <private_key_path>`, its own known-hosts file and its own command, and
  the command it runs on the machine is then its own choice, outside the route
  rule of `open_terminal`. Horizon still guarantees the fixed argv of the terminal
  panel that it opens itself, and that the person's own keys and ssh agent are
  not offered to the machine. Nothing here makes the key a secret from a process
  of the person.
- **First contact is trust on first use.** Horizon pins the host key that the
  first login sees. It does not verify that the machine is the one the agent
  meant, and the person's own known-hosts entries do not apply to that key.
- **Horizon does not put the public key on the machine.** The agent does that at
  rent time, so Horizon cannot prove that the machine holds the key, and a
  machine that the agent prepares with another key still accepts it.
- **A deleted key is gone, not revoked.** `delete_machine_key` removes the
  local files and the known-hosts file of that key. If the public key is still
  in the `authorized_keys` of a machine, the machine still accepts it until the
  person or the agent removes it.

## Does not guarantee

- **The host is the agent's choice.** The agent selects the machine, and the
  person's OpenSSH configuration and known-host entries decide whether Horizon
  reaches it, unless the agent names a machine key: then the known-hosts file of
  that key decides. Horizon trusts no host key and accepts no key, fingerprint or
  option from an agent.
- **The host key question and the login are the person's.** `ssh` runs in the
  new panel, so an unknown host key and a password prompt wait for the person in
  that terminal, unless the terminal names a machine key: then the known-hosts
  file of that key accepts the first host key and pins it. Horizon answers
  neither question, and it does not prove that a login succeeded.
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
