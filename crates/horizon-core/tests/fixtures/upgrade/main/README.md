# A session and settings file of the `main` build

A person ran a build of `main` (commit `d192d408c`, 2026-11-11). That build kept
the settings in the store `settings.sqlite` and also wrote the YAML copy
`config.yaml`, so a home of that build holds both. The files here are what it
wrote, and they are the input of the upgrade test of this folder.

| File | What it is |
|---|---|
| `runtime.yaml` | The board of one saved session: five workspaces, of which one is empty, one holds a Shell panel, one a Usage panel, one a Git Changes panel and a setup board, and one a Markdown panel. |
| `meta.yaml` | The metadata of that session. |
| `index.yaml` | The session index of the profile. |
| `config.yaml` | The settings file with every default value, including the sections and the panel kinds of the features that the current build removed. Its types and its key order follow `crates/horizon-core/src/config.rs`, `config/presets.rs`, `config/remote_hosts.rs`, `config/browser_host.rs`, `speech_config.rs` and `panel.rs` of that build. |
| `config-all-values.yaml` | The same shape with every section set to a value that is not the default. Its `browser.remote` provider binds one credential to the OS credential store (`store: os_keychain`), which the current build reads as the secrets file of the home. |

The session id and the profile id are the ones that the build wrote there. The
profile id is the hash of the absolute path of the settings file, so the files
name the synthetic home `/home/user/project/.horizon/config.yaml` and the hash of
that path; a real home would name the person's own path.

The home of that build also holds `settings.sqlite` with a `mirror_sha256` meta
row and one row for each of its sections. A binary fixture of that file is not
kept here: `crates/horizon-core/src/config/store.rs` builds it in a test from the
statements of `config_store/schema.rs` and `config_store/sections.rs` of that
build, so the test holds the same bytes that the build wrote.

Every path that would name a real computer is `/home/user/project` here, and
the session's `transcripts` folder and runtime index are not copied.
