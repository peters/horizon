# Hetzner Cloud workers

Hetzner is a second provider for CPU clouds (#972). A cloud whose profile names
`hetzner` deploys on a Hetzner Cloud server: the server runs the unchanged
worker image under Docker, and the workspace lives on a Hetzner volume in the
same location. The New cloud dialog does not offer Hetzner profiles yet; deploy
them with the deployment coordinator (`cloud_deploy`). Stop, resume, check,
delete and rebuild are not wired yet and are refused before any provider
request; delete a test cloud's server, volume and SSH key in the Hetzner console.
Horizon also checks the cloud ID, `idle_stop_minutes`, the token and the
locations before it records or builds anything, and the registry pull
credential before it creates anything.

## How a deployment runs

1. Horizon registers a throwaway SSH key for the cloud. Its private half is
   discarded; it only stops Hetzner from generating and emailing a root password.
2. It picks the first allowed location, in order, where an allowed server type
   has the profile's CPU, memory and container disk, skipping locations where
   none does, and creates an ext4 workspace volume there. The volume fixes the
   location for the cloud's whole life.
3. It tries the allowed server types that have the profile's CPU, memory and
   container disk in that location, in order. A capacity refusal (HTTP 412 `resource_unavailable` or HTTP 422
   `placement_error`) moves on to the next type; any other refusal stops.
   Hetzner's availability flag is advisory, so every allowed type is tried.
4. The server boots Hetzner's `docker-ce` image with user data that mounts the
   volume, pulls the worker image by digest and runs it with sshd on port 22.
5. Readiness waits for the worker contract over SSH. The server first answers
   with its own sshd, so the host key is pinned only after the worker passes.

A lost response never creates a second server or volume: each request is
recorded first, and a retry looks the resource up by the cloud's label. The
volume, location and key are recorded in the cloud's `hetzner.json`.

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
- `registry_pull` is optional and only needed for a private worker image:

  ```json
  "registry_pull": {
    "server": "example.azurecr.io",
    "username": "horizon-pull",
    "password_file": "/home/me/.config/horizon/cloud/credentials/hetzner-pull"
  }
  ```

  The credential reaches the server's user data, which the host can read for
  the server's whole life (the container cannot). Use a read-only token scoped
  to the image repository, with a short expiry. Horizon reads it only while a
  server can still be created; a retry that reconnects to a server already
  requested does not need it, so an expired token never blocks recovering one.

## Not available on Hetzner yet

- `idle_stop_minutes`: a Hetzner worker cannot stop its own billing, because a
  powered-off server is still billed and deleting it needs a project-wide token.
- Shared workers, hosted devices and image rebuilds.
- Cloud IDs must be lowercase letters, digits and hyphens, at most 48
  characters, and cannot start or end with a hyphen. Clouds created in the app
  already are.

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
- offers stay informational (`rentable: false`) until the New cloud dialog can price and
  create Hetzner clouds; the deployment coordinator already deploys them.

Workers receive the catalog through `horizon-cloud-worker cloud-offers publish-hetzner`,
beside the price list, so older worker images keep taking RunPod prices unchanged.
