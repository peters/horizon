# Hetzner Cloud workers

Hetzner is being added as a second provider for CPU clouds (#972). This page
covers what is configurable today. Deployment on Hetzner is not wired yet: a
cloud whose profile names `hetzner` is validated and recorded, and deploying it
fails with "Hetzner clouds cannot be deployed yet" before anything is created.

## Profile

A profile selects Hetzner with `provider: hetzner`:

```yaml
profiles:
  cheap:
    provider: hetzner
    image: registry.example.com/team/worker
    cpu: 8
    memory_gb: 16
    storage:
      volume_gb: 100
```

- CPU only. Hetzner has no hourly GPUs, so `gpu: true` is refused.
- The workspace volume must be 10 to 10,240 GB.
- Hosted devices (`capabilities.browserstack`) are refused for now.

## Machine settings

Add a `hetzner` section to the cloud `settings.json`. It is optional; settings
without it are read and written exactly as before.

```json
"hetzner": {
  "token_file": "/home/me/.config/horizon/cloud/credentials/hetzner",
  "server_types": ["cx43", "cpx42"],
  "locations": ["hel1", "nbg1"]
}
```

- `token_file` is an absolute path to a private (0600) file holding a Hetzner
  Cloud API token with read and write access. The token covers the whole
  project, so it stays on this machine and never reaches a worker. Use a
  project dedicated to Horizon.
- `server_types` and `locations` list what Horizon may request, in order of
  preference. Only x86 types fit the worker image.
- A cloud's chosen data centers narrow `locations` to the ones it names. They
  cannot add a location the settings do not allow; a cloud placed only in
  locations the settings do not allow is refused rather than moved.

Horizon targets the current Hetzner Cloud API as described by
<https://docs.hetzner.cloud/cloud.spec.json>. `scripts/check-hetzner-api.py`
checks every operation and field Horizon uses against that spec.
