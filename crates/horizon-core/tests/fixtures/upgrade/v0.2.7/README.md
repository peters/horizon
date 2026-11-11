# A session and settings file of release v0.2.7

A person made these files with the release binary of v0.2.7, and they are the
input of the upgrade test of this folder.

| File | What it is |
|---|---|
| `runtime.yaml` | The board of one saved session: a Shell panel, a Markdown panel, and a workspace whose only panels are the removed Usage and Git Changes kinds. |
| `meta.yaml` | The metadata of that session. |
| `index.yaml` | The session index of the profile. |
| `config.yaml` | The settings file that the release wrote on that computer. The person had set `appearance.theme` to `dark`. |
| `config-default.yaml` | The same file with every default value, which is what the release wrote on a computer where nobody changed a setting. It differs from `config.yaml` in the theme line only. |
| `config-all-values.yaml` | A settings file of the release with every section set to a value that is not the default, including the sections and the panel kinds of the features that the current build removed. Its types and its key order follow `crates/horizon-core/src/config.rs`, `speech_config.rs` and `panel.rs` of that release. |

The session id and the profile id are the ones that the release wrote there. The
profile id is the hash of the absolute path of the settings file, so the files
name the synthetic home `/home/user/project/.horizon/config.yaml` and the hash of
that path; a real home would name the person's own path.

Every path that started with `/var/tmp/hz-slim-bh/up/v027b/data` in the source
home is `/home/user/project` here. No other byte changed, and the `transcripts`
folder of the session is not copied.
