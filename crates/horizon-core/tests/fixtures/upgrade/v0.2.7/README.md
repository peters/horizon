# A session and settings file of release v0.2.7

A person made these files with the release binary of v0.2.7, and they are the
input of the upgrade test of this folder.

| File | What it is |
|---|---|
| `runtime.yaml` | The board of one saved session: a Shell panel, a Markdown panel, and a workspace whose only panels are the removed Usage and Git Changes kinds. |
| `meta.yaml` | The metadata of that session. |
| `index.yaml` | The session index of the profile. |
| `config.yaml` | The settings file of that release. This build does not read YAML settings, so no test reads this file; it is here because the person's home had it. |

The session id and the profile id are the ones that the release wrote there.

Every path that started with `/var/tmp/hz-slim-bh/up/v027b/data` in the source
home is `/home/user/project` here. No other byte changed, and the
`transcripts` folder of the session is not copied.
