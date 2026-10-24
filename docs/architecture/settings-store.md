# Settings store

Horizon keeps its settings in one SQLite file, `settings.sqlite`. It is the only
settings file that Horizon reads and writes. YAML is an export that a person asks
for, not a file that Horizon reads again.

## Where the files are

The store is next to the configuration path that the run resolves. That path
identifies the instance; it is not a file at rest. The store is one file: the
default rollback journal is deleted at each commit, and Horizon writes no other
file after a save.

| Configuration path of the run | Store |
|---|---|
| `~/.horizon/config.yaml` | `~/.horizon/settings.sqlite` |
| `<dir>/horizon.yaml` | `<dir>/settings.sqlite` |
| `<dir>/smoke.yml` | `<dir>/smoke.settings.sqlite` |

A second configuration in one folder keeps its own store. A run that names
`--config` or `HORIZON_HOME` therefore never reads or writes the store of the
user. See [the Horizon home procedure](../testing/procedures/horizon-home.md).

## The tables

The store has schema version 1.

| Table | Rows |
|---|---|
| `settings` | One row for each configuration section: `version`, `window`, `appearance`, `shortcuts`, `overlays`, `features`, `browser`, `remote_hosts`, `presets`, `workspaces`. |
| `meta` | `schema_version`, `saved_at`, and `mirror_sha256` (the hash of the canonical YAML of the last save). |

Each `settings` row has a `format` column. This build writes `json`. A build that
reads another format refuses the store.

A row of a section that this build does not know stays in the store. An older
Horizon does not delete the settings of a newer one.

## What Horizon does at a start

1. Read the store.
2. Import a `config.yaml` copy **only when the store does not exist**.
3. Run the migrations of the configuration version and check the values.
4. Write the store when the start imported a copy or ran a migration.
5. Keep an imported copy aside as `config.yaml.imported-<time>`.

A start that finds a store does not read a YAML copy. A copy that lies next to a
store stays exactly as the person left it, and one log line names the file that
holds the settings. A copy that this start imported is kept as
`config.yaml.imported-<time>`, with every byte, so it cannot come back as a
setting.

## No setting is lost

| Rule | Reason |
|---|---|
| Only a migration rewrites settings, and it writes the store. | The store is the authority; no other file holds a second version of a setting. |
| An imported copy keeps every byte in `config.yaml.imported-<time>`. | Comments, key order, a key that this build does not model and an unknown section stay in the file. |
| A save writes one transaction. | A store that fails leaves the previous settings in place. |
| A store that cannot be read is never replaced by the defaults. | Defaults must not replace the settings of a person. |
| A store of another schema is refused, and neither file is written. | The store may hold settings of a newer Horizon. |
| A build of an older Horizon still reads the store. | `schema_version` decides, not the build date. |

## The export

`horizon --export-config [<path>]` writes the settings as YAML. Without a path it
writes the configuration path of the run, which is `~/.horizon/config.yaml` by
default. With `--config <path>` it writes `<path>`.

The export serves a person who reads the settings and a tool that reads them. It
is not a setting: Horizon never reads it back, and a change in it does nothing. A
setting changes in the Settings window, in `horizon --config`, or in the store.
`--export-config` reads the store only, never a YAML copy, and it reports an
error when the store does not exist, because there is then no setting to export.
An export replaces an existing file at its destination, so a hand-written
`config.yaml` that no start has imported yet is not a destination that an export
chooses by itself: without a store, the command stops before it writes.

## When a write happens

A save and a reload run on the settings thread, not on the UI thread. The
Settings window reads the version, shows `Saving`, and applies the answer in a
later frame. The reasons are in [the UI thread guide](ui-thread.md): a save
commits the store with `synchronous=FULL`.

| Event | What runs on the settings thread |
|---|---|
| Save in the Settings window | Write the store, answer with the saved settings as YAML. |
| Save of a remote-host preference | Write the store, answer with the saved settings. |
| The store changed on disk | Read the settings, answer with them. |

A reload is pending from the moment it is asked for until its answer arrives, so
the window never starts a second one. The bar at the bottom of the Settings
panel shows `Saving` while a write runs and `Saved` when it ends.

## Failures

| Condition | Result |
|---|---|
| A configuration that a person named exists nowhere | Horizon reports an error. It does not start with the settings of the user. |
| The store is damaged and a `config.yaml` copy exists | Horizon reports the store, moves the damaged file to `settings.sqlite.damaged-<time>`, and writes a new store from that copy, then keeps the copy aside. |
| The store is damaged and no copy exists, but a `config.yaml.imported-*` file does | Horizon repairs the store from the newest kept file. The settings of the person come back, and the damaged file stays for examination. |
| The store is damaged and no copy exists | Horizon reports the path and the reason. It does not use the default settings, and the damaged file stays where it is. |
| The store has a schema of another Horizon version | Horizon refuses the store and reports both versions. It writes nothing in its place. |
| A copy that the first start imports does not parse | Horizon reports an error and keeps the copy. |
| The store does not exist and no copy exists | Horizon uses the default settings. It writes no file until a setting changes. |

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
