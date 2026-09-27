# Hetzner Cloud workers

Hetzner is being added as a second provider for CPU clouds (#972). This page
covers what is configurable today. Deployment on Hetzner is not wired yet: a
profile that names `hetzner` is accepted configuration, the New cloud dialog
does not offer it, and preparing or deploying it fails with "Hetzner clouds
cannot be deployed yet" before any cloud state is created.

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

Turn on **Hetzner Cloud** in Cloud settings, or add a `hetzner` section to the
cloud `settings.json`. It is optional; settings without it are read and written
exactly as before. The form stores the token as a private file under
`credentials/` and keeps a saved token when the field is left blank; turning
Hetzner off removes the binding.

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

Horizon targets the Hetzner Cloud API `v1` as described by its OpenAPI spec,
<https://docs.hetzner.cloud/cloud.spec.json>, and the changelog feed,
<https://docs.hetzner.cloud/changelog/feed.json>. Last checked on 2026-09-26
against the spec published on 2026-09-23 (info.version 1.0.0; newest changelog
entry 2026-09-23). `scripts/check-hetzner-api.py` checks every operation and
field Horizon uses against the live spec; run it before changing the Hetzner
integration.

## Offers

With a `hetzner` binding, Hetzner offers appear beside RunPod's wherever offers are
ranked: `cloud_deploy offers SETTINGS [REQUIREMENTS_JSON]` from this computer, and the
`cloud_offers` tool for agents on its workers once the host sends them the catalog.
Hetzner comes in `other_providers`, ranked on its own and never mixed with RunPod:

- amounts are euros, net of VAT, never converted, and `max_hourly` is read in euros for them;
- each estimate is for a run that starts now, billed per started hour and capped per
  calendar month (UTC) for compute, the workspace volume and the IPv4 address;
- `stopped_monthly` is the kept volume only, since a stopped Hetzner cloud releases its
  server and address;
- only the locations in `locations` are listed, with every server type, and
  `availability` is Hetzner's advisory flag (`listed` or `unlisted`), never a filter;
- offers stay informational (`rentable: false`) until Horizon can create clouds on Hetzner.

Workers receive the catalog through `horizon-cloud-worker cloud-offers publish-hetzner`,
beside the price list, so older worker images keep taking RunPod prices unchanged.
