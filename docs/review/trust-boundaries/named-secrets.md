---
applyTo: "crates/horizon-core/src/named_secrets/**,crates/horizon-core/src/secret_store/**,crates/horizon-core/src/config/secrets.rs,crates/horizon-core/src/panel/spawn.rs,crates/horizon-ui/src/app/settings/secrets.rs,crates/horizon-ui/src/app/server_sync.rs,crates/horizon-sync/src/supervisor/secrets.rs,scripts/device-smoke/serve.py"
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

The names live in the `secrets` section of the settings. The values live in the one
secrets file of this Horizon home under the id `named/<NAME>`, and, when a server
is set, in the server as one sealed secret of the id `named/<NAME>`. The id rules
of a secret, the sealing of a server body and the link that carries it are the
boundary of [`server-link.md`](server-link.md); this page states what a name and a
value are, where a value is allowed to go, and what it never reaches.

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

## The secrets file of this computer

- **Every value is a row of one file.** The file is
  `<Horizon home>/secrets.sqlite`. Schema version 1 holds the table
  `secrets(id TEXT PRIMARY KEY, body BLOB NOT NULL, version INTEGER NOT NULL,
  updated_millis INTEGER NOT NULL)` and the table `meta`. A named secret is the id
  `named/<NAME>`, and a provider credential is the id `credential/<reference>`.
- **The file mode is the whole protection.** The Horizon home is `0700`, and the
  file is `0600` from the first byte: the store makes the file with `create_new`
  and that mode before SQLite opens it. A file that a copy or a restore left
  readable is made private again at the next open, by its path. The file keeps a
  rollback journal, so it holds no `-wal` or `-shm` file beside it.
- **A value is stored as it is, and it is not sealed.** A person who can read this
  file reads every value, and that is the same rule as the machine keys under
  `machine-keys/` of this home and as the person's own keys under `~/.ssh`.
- **A process of the same user can read it.** The file mode protects a value from
  another user and from nothing else. An agent, a script or a tool that runs as the
  person reads the file as the person does. A named secret is as private as the
  login of the person, not more.
- **A backup of the home holds every value.** A copy of `~/.horizon` holds every
  value in clear text, and so does a copy of the disk. Keep that copy as private as
  the computer. A file that a person copies to another place carries the values.
- **The server copy is sealed, and its key file is beside it.** The server seals a
  secret body with the key in `<server home>/secret.key`, mode `0600`, and keeps
  that key in the same home as the database. A person who can log in to the server
  account reads the database and the key, so the sealing protects a copy of
  `server.sqlite` alone. The boundary of that copy is
  [`server-link.md`](server-link.md).
- **One writer per process, and no write on the UI thread.** The settings worker,
  the named-secrets worker and the link to a server each reach the file on a worker
  thread, and all of them share one connection: a second connection to one file
  cannot be made in a process. Each write is one transaction with `synchronous=FULL`
  and the rollback journal, and a reader of another process waits up to five
  seconds for a write of this one.
- **A file that cannot be read is one typed error with its path.** The Settings
  window shows that error under a name, the link writes one line of the log, and
  the machine keys still follow the server. There is no state that says that this
  computer has no credential store: a SQLite file can always be opened, and a file
  that cannot be opened is a fault with a reason.
- **This build does not read the OS credential store.** A value that a person
  stored in the keyring or the Keychain with an older build stays there and is not
  read. A reference or a name with no row of the file behaves as a value that this
  computer does not hold: the Settings window shows `no value on this computer`, and
  the person types the value once more.

## The secrets file of an isolated fixture

[`scripts/device-smoke/serve.py`](../../../scripts/device-smoke/serve.py) starts
Horizon on an isolated desktop that holds no value of the person. Its
`--named-secret NAME=VALUE` writes one row `named/<NAME>` into the `secrets.sqlite`
of the private home of that fixture with Python's `sqlite3`, and `--secret-row
ID=VALUE` writes one `credential/<reference>` row the same way. Both run the schema
of [`crates/horizon-core/src/secret_store/schema.sql`](../../../crates/horizon-core/src/secret_store/schema.sql),
which the store of Horizon includes, so the fixture and Horizon write one file and
cannot drift. A test of the crate runs that Python path and opens its file.

The values in that file are in clear text, as they are in the file of a person. The
file belongs to one fixture state, at `0600` in a `0700` folder, and it is deleted
with that state.

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
- **A values file that a person deletes loses every value.** A row that is gone is
  no value: the names stay in the settings, the panels get no variable, and the
  Settings window says that this computer holds no value for the name. The server
  sends a value again to a computer that has none only after a new change of that
  secret. Keep a backup of the file if a value must survive.
- **A value that the person typed into the window is in the memory of this
  process.** The field is masked, its undo history is cleared and its buffer is
  wiped when the value is sent, but the window still holds it while it is typed.
