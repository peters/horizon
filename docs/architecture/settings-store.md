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

## The tables

The store has schema version 1.

| Table | Rows |
|---|---|
| `settings` | One row for each configuration section: `version`, `window`, `appearance`, `shortcuts`, `overlays`, `features`, `browser`, `remote_hosts`, `browser_host`, `server`, `secrets`, `presets`, `workspaces`. |
| `meta` | `schema_version` and `saved_at`. |

Each `settings` row has a `format` column. This build writes `json`. A build that
reads another format refuses the store.

A store of an older Horizon also has a `mirror_sha256` row, which hashed an
exported copy. This build writes no copy, so it does not read that row and does
not delete it: an older Horizon finds its store as it left it.

A row of a section that this build does not know stays in the store. An older
Horizon does not delete the settings of a newer one.

## The settings file of a run

`--config <path>` names a settings file. The file is JSON: one object with the
sections of the table above. The default path of a run is
`~/.horizon/config.json`. A file that is not JSON is refused, and the message
says that the format is JSON.

A run reads that file only while it has no store:

1. Read the store.
2. When there is no store, read the settings file, and run the migrations of the
   configuration version and the checks of the values.
3. Write the store when the start read the file or ran a migration.
4. Leave the imported file exactly where it is, with every byte.

A start that finds a store does not read the file. A file that lies next to a
store stays as the person left it, and one log line names the file that holds the
settings.

The file of an older Horizon was YAML. This build does not read a YAML file. A
start that finds only an old `config.yaml` and no store begins with the default
settings, and it leaves that file exactly as it is.

## No setting is lost

| Rule | Reason |
|---|---|
| Only a migration rewrites settings, and it writes the store. | The store is the authority; no other file holds a second version of a setting. |
| A settings file that a start imports keeps every byte. | A person can read it after the start, and nothing rewrites it. |
| A save writes one transaction. | A store that fails leaves the previous settings in place. |
| A store that cannot be read is never replaced by the defaults. | Defaults must not replace the settings of a person. |
| A store of another schema is refused, and no file is written. | The store may hold settings of a newer Horizon. |
| A build of an older Horizon still reads the store. | `schema_version` decides, not the build date. |

## When a write happens

A save and a reload run on the settings thread, not on the UI thread. The
Settings window reads the version, shows `Saving`, and applies the answer in a
later frame. The reasons are in [the UI thread guide](ui-thread.md): a save
commits the store with `synchronous=FULL`.

| Event | What runs on the settings thread |
|---|---|
| Save in the Settings window | Write the store, answer with the saved settings. |
| Save of a remote-host preference | Write the store, answer with the saved settings. |
| The store changed on disk | Read the settings, answer with them. |

A reload is pending from the moment it is asked for until its answer arrives, so
the window never starts a second one. The bar at the bottom of the Settings
panel shows `Saving` while a write runs and `Saved` when it ends.

## What the Settings window changes

The window has one tab for each group of settings, and every field writes the
typed value of its section:

| Tab | Sections |
|---|---|
| General | `window`, `appearance`, `features`, `overlays`, `browser_host`, `server`, `remote_hosts` |
| Shortcuts | `shortcuts` |
| Presets | `presets` |
| Browser | `browser`: the engine programs, the live view, the recording defaults and the passkey command |
| Remote browsers | `browser.remote`: the providers, the targets and their credential bindings |
| Secrets | `secrets`: the names of the named secrets, and no value |

A numeric field does not clamp a value that the store already holds, so a value
that a settings file or an older Horizon wrote stays exactly as it is until a
person edits that field. A value that breaks a rule shows the rule under its
field, and Save reports the same text. The rules are the validation of the
sections, so the window and a start read one sentence per refusal.

### The values that no settings row holds

The `secrets` section holds the names of the named secrets of this computer and
nothing else. A value lives in the OS credential store under the reference
`named/<NAME>`, and the server link carries it as one sealed secret. The `secrets`
row of the store therefore holds no value, and a copy of the settings of a person
holds no value. See
[the boundary of the named secrets](../review/trust-boundaries/named-secrets.md).

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
| A settings file that a start reads is not JSON | Horizon reports the format and keeps the file. |
| The store does not exist and no settings file exists | Horizon uses the default settings. It writes no file until a setting changes. |

## What is not in the store

| Item | Owner |
|---|---|
| Browser profiles, cookies and site storage | Chromium and Firefox. |
| Audit records in `~/.horizon/audit` | The append-only audit log. |
| Credential values | The OS credential store. The configuration holds only references. |
| Agent plugin manifests | The agents that read them. |
| Board, panels and workspaces of a session | `sessions/<id>/runtime.sqlite`. |

## Rules for a host process

A later session host writes the settings of the person, so these rules keep the
one writer:

1. The store has one writer. Clients never open the file.
2. A save of one section is one transaction.
3. A host on another machine writes the settings through the host, and a client
   reads the sections. A file on one machine is never the record.
