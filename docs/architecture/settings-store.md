# Settings store

Horizon keeps its settings in one SQLite file. `config.yaml` stays the file that
a person can read and edit, and Horizon exports it. The store is the authority.

## Where the files are

The store is next to the configuration file. It is one file at rest: Horizon
removes no other file after a save, and the default rollback journal is deleted
at each commit.

| Configuration file | Store |
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
| `meta` | `schema_version`, `mirror_sha256` (the hash of the exported copy), and `saved_at`. |

Each `settings` row has a `format` column. This build writes `json`. A build that
reads another format refuses the store.

A row of a section that this build does not know stays in the store. An older
Horizon does not delete the settings of a newer one.

## What Horizon does at a start

1. Read the store.
2. Compare the `config.yaml` copy with `mirror_sha256`.
3. Import the copy when it differs, when the store does not exist, or when the
   copy does not exist and the store does.
4. Run the migrations of the configuration version and check the values.
5. Write what the start must write:
   - an import writes the store only;
   - a migration writes the store and the migrated copy;
   - a start with a store and no copy writes the copy from the store.

## No setting is lost

| Rule | Reason |
|---|---|
| An import does not change the copy. The bytes stay as the person wrote them. | A comment, the order of the keys, a key that this build does not model and an unknown section stay in the file. |
| A migration writes the copy in the canonical form and first keeps the old text as `config.yaml.imported-<time>`. | The migrated copy must show the version that the store holds. The words of the person stay in the kept file. |
| A save writes the text that a person saved. | The Settings window does not lose a comment either. |
| The copy is written before the store. | A store that fails leaves a newer copy, and the next start imports it. |
| A store that cannot be read is an error. | Defaults never replace the settings of a person. |
| The copy of an older Horizon still loads the store and the migrated copy. | A return to an older build loses nothing. |

## Failures

| Condition | Result |
|---|---|
| A configuration that a person named does not exist in either place | Horizon reports an error. It does not start with the settings of the user. |
| The store is damaged and the copy exists | Horizon reports the store, moves the damaged file to `settings.sqlite.damaged-<time>`, and writes a new store from the copy. The settings of the person come back, and the damaged file stays for examination. |
| The store is damaged and the copy is missing | Horizon reports the path and the reason. It does not use the default settings, and the damaged file stays where it is. |
| The store has a schema of another Horizon version | Horizon refuses the store and reports both versions. It writes neither the store nor the copy in its place, because the store may hold newer settings. |
| The copy does not parse | Horizon reports an error. |
| The copy is missing and the store exists | Horizon writes the copy again from the store. |
| The copy is missing and the store is missing | Horizon uses the default settings. It writes no file. |

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
3. The copy next to the store is a convenience for one machine. A host on
   another machine writes the store only, and a client reads the sections.
