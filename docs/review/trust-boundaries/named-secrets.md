---
applyTo: "crates/horizon-core/src/named_secrets/**,crates/horizon-core/src/config/secrets.rs,crates/horizon-core/src/panel/spawn.rs,crates/horizon-ui/src/app/settings/secrets.rs,crates/horizon-ui/src/app/server_sync.rs,crates/horizon-sync/src/supervisor/secrets.rs,scripts/device-smoke/serve.py"
---

# Trust boundary of the named secrets

A named secret is one value that a person stores once, under a name that is an
environment variable name. Every agent panel that Horizon starts on this computer
gets it as an environment variable, so a tool that an agent starts reads its key
without an export in the shell of every computer. The design page is
[`docs/architecture/horizon-server.md`](../../architecture/horizon-server.md) for
the server half, and the module comment of
[`crates/horizon-core/src/named_secrets/mod.rs`](../../../crates/horizon-core/src/named_secrets/mod.rs)
for this half.

The names live in the `secrets` section of the settings. The values live in the OS
credential store under the reference `named/<NAME>`, and, when a server is set, in
the server as one sealed secret of the id `named/<NAME>`. The id rules of a
secret, the sealing of a server body and the link that carries it are the boundary
of [`server-link.md`](server-link.md); this page states what a name and a value
are, where a value is allowed to go, and what it never reaches.

## Untrusted: the name and the value that a person types

- **A name is checked before it is used.** A name is 1 to 64 characters of `A-Z`,
  `0-9` and `_`, and it starts with `A-Z`. Anything else is a typed refusal and no
  name enters the list: not a path, not a shell word, not a name with a `.` or a
  `-`.
- **The names that Horizon and the system own are refused.** `PATH`, `HOME`,
  `USER`, `SHELL`, `TERM`, `LANG`, `DISPLAY`, `WAYLAND_DISPLAY`, `TMPDIR`, and
  every name that starts with `LD_`, `DYLD_`, `SSH_`, `HORIZON`, `CLAUDE`, `CODEX`,
  `ANTHROPIC`, `OPENAI`, `XDG_` or `DBUS_`. A named secret therefore never decides
  how a program finds its home, its shell, its display or its agent, and it never
  replaces a variable that Horizon sets for its own tools.
- **A value is text of one to 16 KiB.** An empty value, a value above 16 KiB, a
  NUL byte, a line break and a byte that is not UTF-8 text are typed refusals. A
  value becomes one environment variable, so a value with a line break would be
  two secrets and no secret.
- **A refusal names the secret and never quotes it.** Every refusal of this module
  is `NamedSecretError`, whose text holds a name and a rule and no value. A log
  line names the secret, its code and the rule.
- **At most 64 named secrets.** A 65th name is refused, and the list of the
  settings is cut at 64, so one list cannot fill the environment of a panel.

## Untrusted: a row of a server

- **A secret row is an id and a body.** An id passes `SecretId::parse`, so it is
  `named/<NAME>` with the name rule above, and a body is checked against the value
  rule before it reaches the store. A row that breaks either rule is one line of
  the log and the local value stays.
- **A name that arrives from the server becomes a name of the settings.** A
  computer that follows a server lists the names that the server holds, so a
  second computer gives its agent panels the same variable without a step. The
  write of the settings happens on the settings worker thread.
- **A restored or malicious server can add a named secret, replace one and delete
  one.** That is what the person asked for, and it is bounded by the two rules
  above: a name is never a path, a shell word or a name that Horizon owns, and a
  value is never longer than 16 KiB of text.

## Where a value goes, and where it never goes

- **It becomes one environment variable of an agent panel.** Only the panel kinds
  that `PanelKind::is_agent` answers for get it, and only on this computer: shell,
  ssh, command, editor, browser, device, usage and block panels get none.
- **No ssh command line and no machine holds one.** A terminal on a machine is an
  `ssh` panel of kind `ssh`, so it gets no named secret. The argv of every ssh of
  Horizon is built from a record and from Horizon's own text, so no value and no
  name reaches a remote shell, a tmux command or a login command.
- **A saved session holds no value.** A saved session holds the kind, the command
  and the arguments of a panel. A restored agent panel reads the values from the
  store again at its start, because the environment of a panel comes from this
  process and never from a file.
- **No `tracing` line, panel title, work ledger, audit log, crash text or error
  text holds a value.** The only exits of a value are the environment of a child
  process and the body of one sealed server secret.
- **The queue of the link holds the id and never the body**, as it does for every
  secret.

## The store of an isolated fixture

[`scripts/device-smoke/serve.py`](../../../scripts/device-smoke/serve.py) starts
Horizon on an isolated desktop that has no OS credential store of its own. Its
`--named-secret NAME=VALUE` writes the values into the private state of that
fixture, in one JSON file at `0600`, and names that file in
`HORIZON_TEST_NAMED_SECRETS_FILE`. A Horizon that this variable names reads that
file instead of the OS credential store
([`crates/horizon-core/src/named_secrets/test_file.rs`](../../../crates/horizon-core/src/named_secrets/test_file.rs)).
The values in that file are in clear text. The file belongs to one fixture state,
it is deleted with that state, and a Horizon that a person starts has no such
variable, so it reads the OS credential store and no file.

## What the guarantees do not cover

- **An agent reads the value, and that is the purpose.** A named secret is an
  environment variable of an agent panel, so the agent and every tool it starts
  can read it, print it, and send it wherever that tool sends anything. Give a
  tool a key of the smallest scope that it needs, and rotate the key when its
  agent had one it should not have had.
- **A second process of the same user can read the environment of a panel.** On
  Linux the environment of a running process is in `/proc/<pid>/environ`, and on
  macOS a process of the same user can read it with the tools of the system. A
  named secret is as private as the login of the person, not more.
- **A panel keeps the environment of its start.** A value that the person adds,
  replaces or deletes applies to the panels that start after that. A running panel
  keeps the value that it started with, and the Settings text says so.
- **A computer without a credential store keeps no value.** Its names stay in the
  settings, its panels get no variable, and the Settings window says that this
  computer holds no value for the name.
- **A value that the person typed into the window is in the memory of this
  process.** The field is masked, its undo history is cleared and its buffer is
  wiped when the value is sent, but the window still holds it while it is typed.
