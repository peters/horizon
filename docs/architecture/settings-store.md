# Settings store

Horizon keeps its settings in one SQLite file, `settings.sqlite`. It is the only
settings file that Horizon reads and writes. There is no settings editor of a
text format: a person changes a setting in the Settings window.

## Where the files are

The store is next to the configuration path that the run resolves. That path
identifies the instance; it is not a file at rest. The store is one file: the
default rollback journal is deleted at each commit, and Horizon writes no other
file after a save.

| Configuration path of the run | Store |
|---|---|
| `~/.horizon/config.json` | `~/.horizon/settings.sqlite` |
| `<dir>/horizon.json` | `<dir>/settings.sqlite` |
| `<dir>/smoke.json` | `<dir>/smoke.settings.sqlite` |

A second configuration in one folder keeps its own store. A run that names
`--config` or `HORIZON_HOME` therefore never reads or writes the store of the
user. See [the Horizon home procedure](../testing/procedures/horizon-home.md).

The stores of a browser panel follow the same rule. A run that names
`HORIZON_HOME` keeps every browser profile under that home, in
`<HORIZON_HOME>/browser-profiles`, and its audit and staging folders under the
same home. A run without one keeps `~/.horizon-browser/profiles`, so a person who
updates Horizon stays signed in to their sites. A `browser.profile_root` setting
still names the profile root itself. Two folders of the person stay visible to
them with an override: the attachments of a file dialog, in
`~/Horizon/browser-attachments`, and the downloads of a page, in
`~/Downloads/Horizon`, because a person opens those files by hand. A
Snap-confined Chromium of Linux keeps its profile in `~/Horizon/browser-profiles`
even with an override, because the confinement cannot write a hidden folder below
the home directory of the person. See
[the browser routines page](browser-routines.md#private-layout).

Every secret value of the home is one file beside these: `secrets.sqlite`. The
home is `0700`, the file is `0600` from the first byte, and it holds the values of
the credential references of the remote browser providers and of the named
secrets. It keeps a rollback journal and no write-ahead log, so it has no `-wal`
or `-shm` file beside it, and one process opens one connection to it. It holds no
setting, and the settings store holds no value. A value is stored as it is, and
the file mode is its whole protection; [the boundary of the named
secrets](../review/trust-boundaries/named-secrets.md) states what that does and
does not protect.

## The tables

The store has schema version 1.

| Table | Rows |
|---|---|
| `settings` | One row for each configuration section: `version`, `window`, `appearance`, `shortcuts`, `overlays`, `canvas`, `features`, `browser`, `remote_hosts`, `browser_host`, `server`, `secrets`, `speech`, `presets`, `workspaces`. |
| `meta` | `schema_version` and `saved_at`. |

Each `settings` row has a `format` column. This build writes `json`. A build that
reads another format refuses the store.

A store of an older Horizon also has a `mirror_sha256` row, which hashed an
exported copy. This build writes no copy, so it does not read that row and does
not delete it: an older Horizon finds its store as it left it.

A row of a section that this build does not know stays in the store. An older
Horizon does not delete the settings of a newer one. A row is the whole value of
its section: a save writes every row, and the two paths of
[a section of the server](#the-settings-that-every-computer-shares) write the rows
of the sections that arrived and no other row.

## The settings file of a run

`--config <path>` names a settings file. The file is JSON: one object with the
sections of the table above. The default path of a run is
`~/.horizon/config.json`.

The format of a document is its content. A JSON object is the settings file of
this build. A document that is not a JSON object and is a YAML mapping is the
settings file of an older Horizon, and a start imports it one time, as the next
section says. A document that reads as neither is refused, and the message says
that the format is JSON.

A run reads that file only while it has no store:

1. Read the store.
2. When there is no store, read the settings file, and run the migrations of the
   configuration version and the checks of the values.
3. Write the store when the start read the file or ran a migration.
4. Leave the imported file exactly where it is, with every byte.

A start that finds a store does not read the file. A file that lies next to a
store stays as the person left it, and one log line names the file that holds the
settings.

## Upgrade from an older Horizon

A person who upgrades keeps the settings and the sessions of the build that they
ran before. A release up to v0.2.7 kept its settings in `config.yaml`, in the
Horizon home, and had no store. A build of `main` wrote `settings.sqlite` and kept
`config.yaml` beside it as a copy.

A start with no store and no JSON settings file reads the YAML settings file of the
older build in the folder of its settings file, `config.yaml` and then
`config.yml`. It migrates the document from its configuration version, checks the
values and writes the store. The YAML file keeps every byte, so a person who goes
back to the older build still has it. One `info` line names the file.

| What a start finds | What it becomes |
|---|---|
| A store and a YAML file | The store is the settings. The YAML file is not read, not imported and not rewritten. |
| A YAML file and no store | The YAML file is imported one time into the store, with the migrations and the checks of this build. The file stays as it is. |
| A YAML file that main moved aside, `config.yaml.imported-<time>` | That name is not one that a start reads. The store of main holds the settings. |
| A store that main wrote | It opens. `schema_version` is 1 in both builds. A section that this build does not know stays in the file and is not read, and the `mirror_sha256` row stays for the older build. A section that main has and this build does not, such as `features.speech`, is ignored. |
| A YAML file that cannot be read, or one of a configuration version above this build | The start stops with a sentence that names the file and the line, and it writes no store. The defaults never replace the settings of a person. |
| A settings file that main wrote, `config.yaml` | The same as the row above: it is imported one time, because its content is YAML. |
| `--config <path>` that names a YAML file | Imported one time, with a log line that says that a YAML file is read for import only. The file stays as it is. |

Every key that this build no longer has is ignored, and every value that it still
has arrives unchanged: the window, the appearance and the theme, the shortcuts,
the presets, the workspaces, the features that remain and the browser settings
that remain. A panel kind that a release removed leaves the presets and the
boards, and the rest loads. The list of the removed features and of what a person
sees in their place is [below](#what-an-upgrade-removes).

### The profile of a home

The session store keeps one profile for each settings path, and the profile id is
the hash of that path. A default run keeps the id that release v0.2.7 and `main`
gave it: the id of `<home>/config.yaml`, which was the settings path of both. The
settings file of this build is `<home>/config.json`, and its hash would be a second
profile with an empty board, so the sessions, the last session and the browser
profiles of a person would look gone, and a person who goes back would not find
what this build wrote. A run that names its settings file, such as a smoke run
with `--config smoke.json`, keeps the id of that path.

### The saved sessions

A session of an older build opens: `sessions/index.yaml` names its profile, and
`runtime.yaml` and `meta.yaml` hold the board and its metadata.

| What a start finds in a session | What it becomes |
|---|---|
| A panel of a kind that this build still has | It comes back in its workspace at its place. |
| A panel of a kind that this build removed | It is left out, and one line in the sidebar names the count of each removed kind. The line stays until the person closes it, so a person who looks away at the start still reads it. |
| A field that this build does not know | It is ignored. The rest of the session loads, and one field never stops a session. |
| A board of an older board version, or one that lost a panel | The first write of this build copies the file to `runtime.yaml.before-<version>` beside it, so a person who goes back finds the board. The load itself writes nothing. |

## What an upgrade removes

| Removed item | What a person sees in its place |
|---|---|
| The Usage panel | Nothing. The panel kind is gone, and the session notice names it. |
| The Git Changes panel | Nothing. The diff of a folder is read in a terminal or in an agent panel. |
| The setup board | The Settings window holds the settings that the board guided a person through. |
| Speech input and its `features.speech` section in `features` | Nothing. The section is ignored. Speech input of this build has its own `speech` section with a recorder line, a transcriber and a hotkey, so a start reads the defaults of that section and a store without the row reads them too. |
| The attention feed and its `overlays.attention_feed_*` values | Nothing. An agent reports in its own panel. |
| The cloud workspace profiles, `remote` | Nothing. A machine of the machine list and a browser host hold the remote work. |
| The casting, native app testing and tailnet settings | Nothing. Their sections are ignored. |
| A credential value of the OS credential store | The reference says that this computer has no value, and the tab "Remote" offers the one-time import below. A person who installs none of the programs of the keyring types the value once more. |

## No setting is lost

| Rule | Reason |
|---|---|
| Only a migration rewrites settings, and it writes the store. | The store is the authority; no other file holds a second version of a setting. |
| A settings file that a start imports keeps every byte. | A person can read it after the start, and nothing rewrites it. |
| A save writes one transaction. | A store that fails leaves the previous settings in place. |
| A store that cannot be read is never replaced by the defaults. | Defaults must not replace the settings of a person. |
| A store of another schema is refused, and no file is written. | The store may hold settings of a newer Horizon. |
| A build of an older Horizon still reads the store. | `schema_version` decides, not the build date. |
| A preset or a panel of a panel kind that a later release removed leaves the settings and the saved board. | The typed form has no variant for that kind, and the rest of the settings and of the board must still load. The configuration version records the change. |

## When a write happens

A save and a reload run on the settings thread, not on the UI thread. The
Settings window reads the version, shows `Saving`, and applies the answer in a
later frame. The reasons are in [the UI thread guide](ui-thread.md): a save
commits the store with `synchronous=FULL`.

| Event | What runs on the settings thread |
|---|---|
| Save in the Settings window | Write the store, answer with the saved settings. |
| Save of a remote-host preference | Write the store, answer with the saved settings. |
| The store changed on disk | Read the settings, answer with them, and apply them to the window. |
| A section of the server, after the link is in step | Write the rows of the sections that arrived, on the worker of the link, after the merged settings passed the rule. |

A reload is pending from the moment it is asked for until its answer arrives, so
the window never starts a second one. The bar at the bottom of the Settings
panel shows `Saving` while a write runs and `Saved` when it ends.

## The settings that every computer shares

A settings store belongs to one computer. A person who names a server in the
settings entry `server` makes the shared rows of every computer of that server the
same, because the owner decided that no computer has rules of its own. One
table in `crates/horizon-core/src/settings_sync.rs` names every section of the
store with its decision, and a test reads the table against the rows that the
store writes, so a new section without a decision fails it.

| Section | Scope | Reason |
|---|---|---|
| `version` | Local | The configuration version of the settings store of this computer. |
| `window` | Local | The size and the position of the window on the screen of this computer. |
| `appearance` | Shared | One theme for every computer of the person. |
| `shortcuts` | Shared | One set of key bindings, so a key does the same thing on every computer. |
| `overlays` | Shared | |
| `canvas` | Shared | The floor of the zoom that a Fit of a workspace chooses. A computer of a smaller screen fits a workspace at the same floor as the others, so the value is shared. |
| `features` | Shared | |
| `browser` | Shared | |
| `remote_hosts` | Shared | One list of remote hosts. |
| `browser_host` | Shared | One browser host: its SSH target or a machine of the machine list, one profile, one engine and the program of its driver. A computer that cannot reach that host shows the notice that it already shows. |
| `server` | Local | How this computer reaches the server. |
| `secrets` | Shared | The names of the named secrets. A value is a row of `secrets.sqlite`, and the values follow the server as sealed secrets of their own. |
| `speech` | Shared | One recorder line and one transcriber. A recorder that this computer does not have fails its take with the sentence that it already says, and an empty line turns speech off with its sentence. |
| `presets` | Shared | |
| `workspaces` | Shared | |

A section that a newer build adds is shared as well, so it is kept and never
deleted: the rule of "no setting is lost" holds across the computers and the
versions of the person. The key bindings of the machines are not a section of the
store. They follow the server as objects of their own, and a credential value is a
row of `secrets.sqlite`, as the machine list page says.

### One section on the server, and one on the computer

One shared section is one object of the kind `setting` of the server of the
person: the id is the name of the section and the body is the JSON of that section
inside an envelope that names its configuration version. The server stores that
JSON and reads none of it; the frames and the rules are in
[the server page](horizon-server.md#the-settings-of-every-computer).

| Event | What happens |
|---|---|
| A save of a shared section | The settings thread writes the store and then one `setting` intent for each section whose value changed. The intent carries the version that this computer saw. |
| A push that the server applied | The version that the push wrote is kept in the mirror of this computer at once, before the answer that brings the row back. A save in that window therefore names that version, and the server decides it. Otherwise the server would answer `already_answered` and the save would lose to the row of the push. |
| A section of the server, after the first link | The row of the store is replaced on the worker of the link, after the merged settings passed the rule of this computer. The window reads the store again through its watch of the settings file. |
| A section of the server that breaks a rule | The row of this computer stays, the section stays on the server, and one sentence shows. |
| A section of a newer configuration version | The row of this computer stays, the section stays on the server, and one sentence says that this computer runs an older Horizon. |
| A store that fails once | The write runs again on the next answer of the server, because a fault of the store is no refusal of the content of a section. Only a row whose content this build refuses stays refused. |
| A change that loses the version rule | The row of the version that decided it is read again. A row with another content replaces the row of this computer and one sentence names the sections that differed. A row with the same content replaces nothing: this computer takes the version of the server and reads no sentence. |
| The first link, a section that the server has | The value of the server wins, and the value of this computer is kept one time in `settings.before-server.<time>.json` beside the store, at mode 0600. |
| The first link, a section that the server does not have | The value of this computer is pushed. |
| A section that this build does not know | It is kept in the store as it arrived, and its fields survive the save of this build. |

The store is written by two workers: the settings thread of the window, and the
worker of the link that applies a section of the server. Each write is short, and
the store waits for the other writer for five seconds instead of failing, so the
two writers do not lose a change.

### The sections that the person edits during a change

The Settings window reads the store again while it is open. A section that
another computer changed and that the person also edits is not replaced and is not
dropped: the footer of the panel shows `This setting changed on another computer.`
with two choices. `Take it` writes the value of the other computer into the draft.
`Keep editing` keeps the draft, and the next Save writes it as the next change of
that section. Every other section applies at once.

## What the Settings window changes

The window has one tab for each group of settings, and every field writes the
typed value of its section:

| Tab | Sections |
|---|---|
| General | `window`, `appearance`, `features`, `overlays`, `canvas`, `browser_host`, `remote_hosts` |
| Keys | `shortcuts` |
| Presets | `presets` |
| Browser | `browser`: the engine programs, the live view, the recording defaults and the passkey command |
| Remote | `browser.remote`: the providers, the targets and their credential bindings |
| Secrets | `secrets`: the names of the named secrets, and no value |
| Server | `server`: the SSH target, the command and the address of the listener, beside the join of this computer, the invitation of another one and the list of the computers. `<home>/secrets.sqlite` holds the token of a join under the row `server/token` |
| Speech | `speech`: the hotkey, the recorder line, the transcriber and the name of the named secret of its key, and no value of a key |

The eight tabs stay on one row at the default width of the panel, so no tab label
wraps onto a second line.

A numeric field does not clamp a value that the store already holds, so a value
that a settings file or an older Horizon wrote stays exactly as it is until a
person edits that field. A value that breaks a rule shows the rule under its
field, and Save reports the same text. The rules are the validation of the
sections, so the window and a start read one sentence per refusal.

### The values that no settings row holds

The `secrets` section holds the names of the named secrets of this computer and
nothing else. A value lives in `secrets.sqlite` of the same home under the id
`named/<NAME>`, and the server link carries it as one sealed secret. The `secrets`
row of the store therefore holds no value, and a copy of the settings of a person
holds no value. See
[the boundary of the named secrets](../review/trust-boundaries/named-secrets.md).

### The values that an older build kept in the system keyring

An earlier version kept the value of a remote browser provider in the Secret
Service of Linux or in the Keychain of macOS. No start reads that keyring, and no
crate of this workspace opens it. This build reads those values one time, when the
person asks for it:

1. The tab **Remote** holds one line and one button while a reference of these
   settings has no value on this computer: **A value that an older version stored
   in the system keyring can be imported.**
2. A press on **Import from the keyring** reads the entries once, on a worker
   thread, and writes a row of `secrets.sqlite` for each reference that these
   settings name and that has no row yet. See
   [the programs of each system](../platform-support.md#the-keyring-import).
3. The tab then says what happened, in one line: **Imported 2 values. 1 was already
   here. 3 entries of the keyring are not used by this version.**

The rules of the import:

| Rule | Reason |
|---|---|
| Only a press reads the keyring. | A start does not open a keyring, and no value moves without a person who asked for it. |
| A reference comes from the settings, never from the keyring. | A row is `credential/<reference>`, so a value under an account that these settings do not name would be a row that nothing reads. |
| A reference that already has a row keeps it. | A person who typed the value again after the upgrade does not lose it. |
| The keyring entry stays as it is. | A person who goes back to the older version still finds its value. |
| A written row reaches the link to a server like a typed value. | The other computers then hold the same value, and the row travels by id. |
| A refusal names its rule and no value. | The output of the programs of the keyring is untrusted text. See [the boundary of the named secrets](../review/trust-boundaries/named-secrets.md). |

The button stays while a reference of these settings has no value, so a person can
press it again after installing the program of the keyring. A name of a named
secret has no keyring entry: no older version kept one, so the Secrets tab says
that a person types such a value once more.

### The OAuth client of a network of an older build

An older build kept the OAuth client of one Tailscale network in the same keyring.
The service `horizon-cloud-tailnet-api` held the client, and the service
`horizon-cloud-tailnets` held the enrollment auth key of the same network. The
account of an item was the slot of its binding, or the saved local id of the
network for an entry that a person pasted before the binding existed. The value of
the client is one JSON document, and only this shape is read:

```text
{"kind":"oauth","client_id":"<client id>","client_secret":"<client secret>"}
```

The older build kept no tag and no name in the keyring. The name is in
`tailnets.json` of the `cloud` folder of the same home, and the slot of a managed
client is in `tailnet-api-bindings.json` beside it. The tag is the tag that the
older build asked for in every token request: `tag:horizon-worker`.

The tab **Remote** offers one line for each saved network whose client the keyring
holds, and the button under it:

1. The line names the three secrets of
   [the machines recipe](../../assets/plugins/claude-code/skills/horizon-machines/references/tailscale.md):
   `TS_<NET>_CLIENT_ID`, `TS_<NET>_CLIENT_SECRET` and `TS_<NET>_TAG`. `<NET>` is
   the saved name of the network in upper case, with `A-Z`, `0-9` and `_` only, `_`
   collapsed and at most 16 characters. Two names that become one `<NET>` take a
   number, such as `HOME_2`.
2. The line says which of the three names this computer holds already. A name with
   a value is never written over. The line and the button follow the secrets file:
   the answer of a press names the secrets that the press created, and a delete in
   the **Secrets** tab takes a name away from the line again.
3. A press on **Create these secrets** reads the keyring again on a worker thread
   and creates the missing names through the path of a value that a person typed:
   the names enter the `secrets` section, the values enter `secrets.sqlite` under
   `named/<NAME>`, and the link to a server sends them like any other named secret.

A value that is not the shape of the older build is counted, and it becomes no
secret. A press never guesses the client id or the client secret. Neither line
draws a value. The two files of the older build hold names and slots
only, and the client stays in the worker thread of the press that creates it.

### The values that have no field

Two sections hold values that no field of the window edits: `workspaces[*]` and
`terminals[*]`. They are the places, sizes and working directories of the
workspaces, and the terminals of a workspace that a start brings back. Horizon
writes them, not a person: the canvas saves a layout that a person moved, and a
workspace that a person placed on a machine saves its place. There is no editor
for a list of them, because a list of positions is not a setting of an entry.

A person who needs a value that the window does not offer today sets it in one
of two ways:

1. Write the section into the settings file of a new instance, and start that
   instance before it has a store. See [the settings file of a run](#the-settings-file-of-a-run):
   the first start imports the file.
2. Edit the `settings` row of that section in the store with a SQLite client, and
   restart Horizon. A save from the Settings window of that instance writes the
   row whole, so a later save keeps the edited value of every field that the
   window does not show.

Both ways need the rules of the sections: an empty name, a zero port or a place
that breaks the SSH route is refused at the next start.

## The portable profile

`horizon --export-remote-profile <path>` writes the shareable half of
`browser.remote`, and `horizon --import-remote-profile <path>` merges such a file
into the store. That file is JSON too, and its name is
`remote-browser-profile.json` by default. It is not a settings file: it carries no
credential binding and no credential value.

## Failures

| Condition | Result |
|---|---|
| A configuration that a person named exists nowhere | Horizon reports an error. It does not start with the settings of the user. |
| The store is damaged and a settings file exists | Horizon reports the store, moves the damaged file to `settings.sqlite.damaged-<time>`, writes a new store from the file, and leaves the file where it is. |
| The store is damaged and no settings file exists | Horizon reports the path and the reason. It does not use the default settings, and the damaged file stays where it is. |
| The store has a schema of another Horizon version | Horizon refuses the store and reports both versions. It writes nothing in its place. |
| A settings file that a start reads is neither JSON nor a YAML mapping | Horizon reports that the format is JSON and keeps the file. |
| A YAML settings file of an older Horizon cannot be read | Horizon reports the file and the line, writes no store, and keeps the file. |
| The store does not exist and no settings file exists | Horizon uses the default settings. It writes no file until a setting changes. |
| `secrets.sqlite` cannot be opened | The Settings window says which file and why under each name of the Secrets tab, and the link writes one line of the log. The settings and the machine keys still work. |

## What is not in the store

| Item | Owner |
|---|---|
| Browser profiles, cookies and site storage | Chromium and Firefox. |
| Audit records in `~/.horizon/audit` | The append-only audit log. |
| Credential values and named secret values | `secrets.sqlite` of the same home. The configuration holds only references and names. |
| Agent plugin manifests | The agents that read them. |
| Board, panels and workspaces of a session | `sessions/<id>/runtime.sqlite`. |

## Rules for a host process

A later session host writes the settings of the person, so these rules keep the
one writer:

1. The store has one writer. Clients never open the file.
2. A save of one section is one transaction.
3. A host on another machine writes the settings through the host, and a client
   reads the sections. A file on one machine is never the record.
