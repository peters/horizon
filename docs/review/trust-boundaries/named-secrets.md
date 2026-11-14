---
applyTo: "crates/horizon-core/src/named_secrets/**,crates/horizon-core/src/secret_store/**,crates/horizon-core/src/config/secrets.rs,crates/horizon-core/src/panel/spawn.rs,crates/horizon-core/src/remote_browser_credential/keyring_import/**,crates/horizon-core/src/remote_browser_credential/workbench.rs,crates/horizon-ui/src/app/settings/secrets.rs,crates/horizon-ui/src/app/settings/remote_browsers.rs,crates/horizon-ui/src/app/server_sync.rs,crates/horizon-sync/src/supervisor/secrets.rs,scripts/device-smoke/serve.py,assets/plugins/claude-code/skills/horizon-machines/references/tailscale.md,assets/plugins/codex/skills/horizon-machines/references/tailscale.md"
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
- **That read answers before an agent panel starts, or the panel says that it did
  not.** The start of a panel can come before the worker of that start published
  the values, so the worker of the start waits for the answer for at most 2
  seconds. A panel that the bound reaches starts without the variables and says in
  one line that the named secrets were not read in time. A plain terminal reads no
  named secret and never waits.
- **No `tracing` line, panel title, work ledger, audit log, crash text or error
  text holds a value.** The only exits of a value are the environment of a child
  process and the body of one sealed server secret.
- **The queue of the link holds the id and never the body**, as it does for every
  secret.

## Untrusted: the output of a program that reads the keyring

An older version of Horizon kept a value of a remote browser provider, and the
OAuth client of a Tailscale network, in the Secret Service of Linux or in the
Keychain of macOS. This version reads such a value once, when a person presses
**Import from the keyring** in the Settings tab "Remote". No start reads a
keyring, and no crate of this workspace opens one: three programs of the system
answer instead.

- **The output of each program is untrusted.** `secret-tool search --all
  --unlock service <service>`, the `gi` binding of libsecret through
  `python3 -I -c`, and `security find-generic-password -s <service> -a <account>
  -w` each print an account name and a value. This import reads the service
  `horizon-remote-browser`, and the services `horizon-cloud-tailnet-api` and
  `horizon-cloud-tailnets` of an older build. A reading is accepted
  only in the exact format of its program: an item without a value, a value with a
  line break, an attribute group that does not belong to one item, or any other
  line is one typed refusal and no value is written. The text of a program never
  reaches a log line, a notice or an error text.
- **A reference comes from the settings, never from the keyring.** The account of
  an item is `<origin>|<slot>` of the provider and the binding of a reference of
  these settings, so an item of the keyring names a row `credential/<reference>`
  only through a reference that the settings name. That reference passes the
  reference rule of a secret id again before any row is written, and a settings
  file that breaks it writes no row at all.
- **A network name comes from the older build's own catalog, never from the
  keyring.** The import reads `tailnets.json` and `tailnet-api-bindings.json` of
  the `cloud` folder of this home: bounded documents with the exact fields of that
  version, whose names and slots must pass the rules of that version. A document
  that this version cannot read is one typed refusal, and an account that no saved
  network names becomes no secret. A `<NET>` is derived from the saved name and
  passes the name rule of a named secret before a line is offered.
- **The OAuth client is the only value shape that this import reads.** The value of
  an item must be the JSON document `kind: oauth` with a bounded `client_id` and
  `client_secret` of printable bytes. A personal token, an enrollment auth key, a
  document with a field that the older build did not write, and a document with a
  line break are counted as entries that this version does not use, and they
  become no secret.
- **A named secret of a network takes the path of a typed value.** The three names
  enter the `secrets` section, the values enter the secrets file under
  `named/<NAME>`, and the link to a server queues them by id. An existing name
  keeps its value, and a click never writes over a value that a person typed.
- **No value of a network reaches the window.** The offer of the Settings tab holds
  the `<NET>`, the three names and the names that this computer holds. The values
  stay in the worker thread that reads them and that creates the secrets, and no
  `Debug` or `Display` of the import holds one.
- **A value passes the value rule of the store.** The row is written through the
  same store path as a value that a person types, so an empty value and a value of
  more than 16 KiB are refused, and the link to a server sends the value like any
  other.
- **A value that this computer holds is never written over.** A reference that has
  a row in `secrets.sqlite` keeps that row, so a value that a person typed after the
  upgrade survives an import.
- **A value reaches no argument, no environment variable and no file of its own.**
  Each program runs as a child of Horizon with a cleared environment that holds
  `PATH`, `HOME`, `DBUS_SESSION_BUS_ADDRESS` and `XDG_RUNTIME_DIR`, with the value on
  its stdout pipe only, and with a bound: 20 seconds for the whole import on Linux,
  and 60 seconds for each entry on macOS, where the system can ask the person for
  permission once. A child that passes the bound ends by its own process id. Every
  buffer of a value is wiped when it goes.
- **The keyring item stays as it is.** An import copies a value and deletes
  nothing, so a person who goes back to the older version finds the item.

What the import does not guarantee:

- **It cannot tell who wrote an item, nor who wrote the older catalog.** Any program of the person, and any
  process of the same user, can write an item under the service name
  `horizon-remote-browser` and the account of a reference. An import cannot tell
  such a value from the value of the older Horizon, so a value that another
  program wrote is imported when its account names a reference of these settings.
  A program of the person can also write `tailnets.json` of this home, and then the
  name of a network and its account come from that program. The three names still
  pass the name rule, and the value still passes the shape rule. Review a provider
  or a network whose value matters with that in mind.
- **It cannot prove that the tag is the tag of the client.** The third secret is
  `tag:horizon-worker`, the tag that the older build asked for in every token
  request and that its documentation required of the client. The import cannot read
  the tag of a client, so a person who changed the tag of that client in Tailscale
  reads the value of `TS_<NET>_TAG` and corrects it.
- **It cannot tell a locked keyring from an unlocked one without the person.** An
  import unlocks an item, so the system can ask the person to unlock the keyring,
  and the question belongs to the bound of the import.
- **The value passes through another program.** The program of the system reads
  the value from the keyring, so that program holds it in its own memory. Horizon
  adds no protection to the programs of the system.
- **An import writes every row at once.** A refusal of the store in the middle of
  an import keeps the rows that were written before it; it never removes a row.

## An OAuth client of a Tailscale network

A person can store the OAuth client of a Tailscale network as three named
secrets, and an agent then joins a rented machine to that network. The recipe is
[`assets/plugins/claude-code/skills/horizon-machines/references/tailscale.md`](../../../assets/plugins/claude-code/skills/horizon-machines/references/tailscale.md),
and its twin in the other bundle holds the same bytes. The names and the values
are those of the section above; this part states what the recipe adds.

- **The client secret is used on this computer only.** The agent makes one API
token from it with `curl` here, and the token lives one hour. The secret and the
token reach no argument of a process, because a process of the same user reads
such an argument: the value goes to the standard input of `curl`
(`--data-urlencode name@-`, `-H @-`), and a `curl` that does not read it there
stops the recipe.
- **One auth key leaves this computer, and it leaves for one use.** The key
travels in the standard input of `ssh` to a file under `/run` of the machine that
only root reads. `tailscale up --auth-key=file:<path>` reads the file, and the
same command removes it, also when that call fails. The key is not reusable, it
is not ephemeral, it is preauthorized, it carries one tag, and it expires in ten
minutes.
- **No secret, token or key reaches a card, a log or a file of this computer.**
The agent reads the token and the key into a shell variable of its own panel, and
it writes the key into the standard input of the `ssh` that it starts. A refusal
names the code of the API and the tag, and never a request body or a header.
- **The person does every step that is not on the rented machine.** A step on a
home computer of the network or in the access controls is a list on the card with
a `Done` button, and the agent goes on only after the proof command answers.
- **A `ProxyJump` line of the person's ssh config reaches a machine-key
terminal.** Horizon passes no `-F`, so `ssh` reads that file for the destination.
The file belongs to the person, and this boundary does not cover a value that it
holds.

What a compromised rented machine can and cannot do:

- **After the key is used, the key is worth nothing.** It is single-use and it
expires in ten minutes. A machine that reads its own `/run` file after the login
holds a spent key and cannot make another one, because it never held the client
secret and never sees the token.
- **Before the key is used, it admits one device of one tag.** A machine that
reads the file first can use the key elsewhere, and the key then admits one
device to the network with the tag of that network. The access controls of the
tag bound what that device reaches, the person removes the row with the admin
console or the same API, and the key expires in ten minutes in any case.
- **It can neither list nor remove a device, and it reads no token.** Those calls
happen on this computer, with a token that the machine never holds.

What this part does not cover:

- **The agent panel holds the client.** The scopes `auth_keys` and `devices:core`
of the whole network are in the environment of the panel, so a tool that the
agent starts can make a key for any device of that tag while the panel runs. Give
the client the smallest tag that covers the machines, and rotate the secret when
a panel held one that it should not have had.
- **A machine that joins takes a tag and not the login of the person.** Every
device of the tag is one trust group, and the access controls of the network are
the only rule that bounds it.
- **The network is outside this boundary.** Tailscale encrypts the wire, approves
the device and holds the access controls. Horizon states nothing about those.

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
- **This build reads the OS credential store only when a person asks.** A value
  that an older build stored in the keyring or the Keychain is not read at a start.
  A reference with no row of the file behaves as a value that this computer does
  not hold: the Settings window shows that it holds no value, and the tab "Remote"
  offers the one-time import above. A person who installs none of the three
  programs types the value once more.

## The secrets file of an isolated fixture

[`scripts/device-smoke/serve.py`](../../../scripts/device-smoke/serve.py) starts
Horizon on an isolated desktop that holds no value of the person. Its
`--named-secret NAME=VALUE` writes one row `named/<NAME>` into the `secrets.sqlite`
of the private home of that fixture with Python's `sqlite3`, and `--secret-row
ID=VALUE` writes one `credential/<reference>` row the same way. `--named-secret-env
NAME` writes one `named/<NAME>` row with the value of the environment variable
`NAME` of the fixture, so the value is in no argument of the fixture and in no
argument of a process that the fixture starts. The fixture refuses a `NAME` that
is not set or empty, and that refusal names the variable and no value. Both run the schema
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
