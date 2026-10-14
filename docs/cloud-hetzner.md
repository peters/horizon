# Hetzner Cloud workers

Hetzner is a second provider for CPU clouds (#972). A cloud whose profile names
`hetzner` deploys on a Hetzner Cloud server: the server runs the unchanged
worker image under Docker, and the workspace lives on a Hetzner volume in the
same location. Create one with **New cloud** (a `provider: hetzner` profile, or
Hetzner chosen for a CPU profile) or with the deployment coordinator
(`cloud_deploy`); both stop, resume, check and delete it. Rebuilding a Hetzner
cloud's image is refused for now.
Horizon checks the cloud ID, the token, the locations and the registry pull
credential before it records or builds anything.

## How a deployment runs

An explicit worker choice fixes the server type and location. Without an explicit
choice, Horizon uses the configured fallback types and locations in this sequence.

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
    min_cpu: 8
    min_memory_gb: 16
    storage:
      volume_gb: 100
```

- CPU only. Hetzner has no hourly GPUs, so `gpu: true` is refused.
- The workspace volume must be 10 to 10,240 GB.
- Hosted devices (`capabilities.browserstack`) are refused for now.
- `egress` limits where the worker can connect. See
  [Outbound allowlist](#outbound-allowlist).

## Machine settings

Turn on **Hetzner Cloud** in Cloud settings, or add a `hetzner` section to the
cloud `settings.json`. It is optional; settings without it are read and written
exactly as before. The form stores the token as a private file under
`credentials/` and keeps a saved token when the field is left blank; turning
Hetzner off removes the binding.

A machine can use Hetzner alone: with Hetzner on, the RunPod API key may stay
empty. New cloud then offers only Hetzner, and only profiles Hetzner can run (CPU
profiles without hosted devices) are ready. The shared picker can select the cheapest
matching Hetzner worker even when the repository profile names RunPod. It preserves
the profile's minimum requirements and displays the chosen provider, server type and
location before Start cloud. Explicit choices stay selected during refresh. Turning
Hetzner off requires a RunPod key again.

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
- `server_types` lists fallback types in preference order. Horizon uses this list
  when a cloud has no explicit worker choice. It does not restrict the offer catalog.
- `locations` lists permitted locations in preference order. The offer catalog
  contains all current x86 types in these locations that fit the worker image.
- An explicit worker choice can select an x86 type outside `server_types`.
  Horizon checks its resources in the current catalog before deployment.
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

  Before each server is created, including on resume, Horizon asks the registry
  whether the token can read the image, as `docker pull` would. An expired or
  revoked token, or an image the registry does not have, is refused then with a
  clear error, before the server is paid for, instead of as a host that never
  becomes ready. A registry that cannot be reached or answers otherwise leaves
  the decision to the host's own pull.

## Stop, resume, check and delete

- **Stop** releases the server: the worker shuts down gracefully (power is cut
  after a minute), then the server is deleted. A powered-off Hetzner server is
  still billed, so only the volume keeps costing money while a cloud is stopped.
- **Resume** clears the released server, so the next reconnect creates a new
  server in the volume's location that attaches the same volume. The new server
  has a new host key, pinned after its worker contract passes. Processes from
  before the stop are gone; `/workspace` is kept.
- **Check** reports a released server as stopped once it is gone, and a missing
  one as lost. A server powered off any other way is still billed, so check
  reports that and leaves the cloud as it is; stop it to release the server.
  Check never creates, starts or deletes anything. If a stop was
  interrupted before its server was gone, the cloud stays stopping and cannot be
  resumed; stopping again finishes it.
- **Delete** removes the server, the workspace volume and the SSH key, and
  confirms each is gone. Only then can the cloud be removed from Horizon. A
  create request whose response was lost counts as having created nothing only
  if a second look 30 seconds later still finds nothing. A cloud whose
  delete has not finished cannot be deployed until it does; redeploying a
  deleted cloud creates a new volume. Stop and delete work even after the settings stop
  allowing the cloud's location.
- Before stop or delete removes a server, Horizon waits up to five minutes for
  the actions Hetzner still runs on it, such as the attach of the workspace
  volume to a server that is still being created. A server deleted during that
  attach left the volume attached to a server that no longer existed, so the
  volume could not be deleted and kept billing. An action that still runs after
  five minutes does not stop the delete.

## Idle stop

A Hetzner worker cannot stop its own billing: a powered-off server is still
billed, and deleting it needs the project-wide token, which never leaves this
computer. So with `idle_stop_minutes` (10 to 1440) set, Horizon stops the cloud
instead. The worker's idle watcher counts activity as it does on RunPod (agent
terminal output, or the container averaging half a CPU core) and records how
long the worker has been idle; while the cloud is ready, Horizon reads that
record over SSH every two minutes and, once the worker has been idle for the
whole period, stops the cloud exactly as **Stop** does: the server is released
and the volume kept. The card then shows **Stopped after 30 idle minutes** (for
a 30-minute period), and **Resume worker** creates a new server on the same
volume. Nothing resumes it automatically.
Choosing **Reconnect cloud** during the few seconds the stop takes waits for it
to finish and then shows the cloud stopped, or offers **Reconcile stop** when the
stop did not finish, rather than reporting that another operation holds the cloud.

This works only while Horizon is running and the cloud's card is connected. A
cloud left running while this computer is off or asleep keeps its server and is
billed until Horizon runs again or someone stops it. `cloud_deploy idle-check
SETTINGS STATE_ROOT` runs one check from a script. Agents on a Hetzner worker
cannot stop it with `horizon-worker-stop`. The worker image must report
`horizon-idle-report-contract=1`; rebuild older images from the current worker
bootstrap.

## Outbound allowlist

A profile with `egress` lets its worker connect only to the names on an allowlist.
Only Hetzner workers support it, because the host of the server enforces it, outside
the container. Upkeep clouds always use it (see [Upkeep](upkeep.md)).

```yaml
profiles:
  upkeep:
    provider: hetzner
    image: registry.example.com/team/worker
    egress:
      allow:
        - nuget.example.com
        - "*.preview.example.com"
```

The allowlist has these names, and the names in `allow`:

- GitHub: `github.com`, `api.github.com`, `codeload.github.com`, the content hosts
  of GitHub (`objects`, `raw`, `release-assets`, `pkg-containers`, and for Git LFS
  `github-cloud` and `media`, each `.githubusercontent.com`),
  the hosts that serve the logs of Actions (`results-receiver.actions.githubusercontent.com`
  and the storage accounts `productionresultssa0` to `productionresultssa19` of
  `blob.core.windows.net`), `ghcr.io`, and the npm and NuGet registries of GitHub
  Packages.
- Rust: `index.crates.io`, `static.crates.io` and `static.rust-lang.org`.
- npm: `registry.npmjs.org`, `registry.yarnpkg.com` and `nodejs.org`.
- NuGet and .NET: `api.nuget.org`, `builds.dotnet.microsoft.com` and `ci.dot.net`.
- PyPI: `pypi.org` and `files.pythonhosted.org`.
- The model APIs: `api.openai.com`, `auth.openai.com`, `chatgpt.com`,
  `api.anthropic.com`, `console.anthropic.com` and `platform.claude.com`.

Each name is exact. `allow` takes at most 16 names: a lowercase host name, or
`*.example.com` for every name under `example.com`. Do not allow a domain where
other people can publish, such as the storage or the sites of a cloud provider: an
agent could send data there.

The host enforces the allowlist like this:

- The container runs on its own Docker network, `horizon-egress`, without address
  translation. A packet from the container that leaves the host has a private
  source address, so it gets no answer.
- The firewall of the host sends each TCP connection of the container to port 443
  to the egress proxy on the host. The proxy reads the name in the TLS ClientHello.
  When the name is on the allowlist, the proxy resolves the name itself and connects
  to a public address of it. It does not use the address that the container chose,
  and it never connects to a private, loopback or link-local address. Else it closes
  the connection.
- The container resolves names through the resolver of the host. The resolver
  answers only for names on the allowlist and answers NXDOMAIN for all other names.
- The firewall drops everything else that the container sends: other ports, UDP
  (also QUIC), DNS to another resolver (for example `1.1.1.1`), DNS over TLS and
  ICMP. Replies to SSH connections from outside pass.
- The guard script installs the network and the rules before each start of the
  container. When a step fails, the container does not start.
- The proxy and the resolver run as services of the host. When one of them stops,
  the container cannot connect through it. The container never gets another way
  out. systemd starts a stopped service again after one second.
- The proxy and the resolver write each refused name to
  `/workspace/.horizon-egress/refused.jsonl` in the container: one JSON line with
  the time, the name and `tls` or `dns`. Each of the two services writes at most
  one line a minute for a name, and at most 30 lines a minute. The folder belongs to
  root, so an agent cannot change the log; root in the container can. The journal of
  the host keeps a copy (`journalctl -u horizon-worker-egress-proxy`). The log starts
  again after 256 KiB.
- The allowlist is in `/etc/horizon-worker/egress-allow` on the host. No process in
  the container can change it. A rebuild keeps the allowlist of the worker; a new
  cloud gets the allowlist of its profile.

The allowlist does not stop these:

- Data sent to a name on the allowlist. An agent that has the credential of
  another account can, for example, push to a repository of that account on
  GitHub, publish a package to npm or send data to a model API. The host cannot
  tell the services of the worker from the agents, because all of them use the same
  network.
- A content delivery network that sends a request to another site by the HTTP Host
  name inside the TLS connection. The proxy checks only the name in the
  ClientHello.
- A tailnet. Tailscale is not on the allowlist, so a tailnet does not connect from
  such a worker. Connect over SSH.

## Rebuilding the image

**Rebuild image & restart** works as on RunPod (see [Cloud workspaces](cloud-workspaces.md#rebuilding-a-clouds-image)),
except for the switch itself. Hetzner cannot report which image a server runs,
so Horizon does not switch the server in place:

1. It builds and verifies the new image as usual. When the image is private, it
   also checks that the host's pull login (`registry_pull`) can read the new
   digest, before anything happens to the server.
2. It releases the server as **Stop** does. The workspace volume is kept.
3. It records the new image and clears the server's fence in one save, then
   reconnects. The reconnect creates a new server on the rebuilt image, attaches
   the same volume and relaunches the sessions. The server gets a new address.
   When the rebuild changes the cloud's settings, the same save records the new
   profile, and the new server gets the new capabilities and idle stop.

Every step is recorded first, so after an interruption **Continue rebuild**
finishes the release and starts the new server. **Cancel rebuild** leaves the
cloud as it was when the release had not begun. Once the release has begun, it
finishes the release and starts a new server on the previous image instead, with
the previous settings.

## Not available on Hetzner yet

- Shared workers and hosted devices.
- Cloud IDs must be lowercase letters, digits and hyphens, at most 48
  characters, and cannot start or end with a hyphen. Clouds created in the app
  already are.

Horizon targets the Hetzner Cloud API `v1` as described by its OpenAPI spec,
<https://docs.hetzner.cloud/cloud.spec.json>, and the changelog feed,
<https://docs.hetzner.cloud/changelog/feed.json>. Last checked on 2026-10-11
against the spec published on 2026-10-05 (info.version 1.0.0; newest changelog
entry 2026-10-05). `scripts/check-hetzner-api.py` checks every operation and
field Horizon uses against the live spec; run it before changing the Hetzner
integration.

## Offers

With a `hetzner` binding, Hetzner offers appear beside RunPod's wherever offers are
ranked: `cloud_deploy offers SETTINGS [REQUIREMENTS_JSON]` from this computer, and the
`cloud_offers` tool for agents on its workers once the host sends them the catalog.
Native Hetzner offers remain in `other_providers`. The additional `comparison`
orders offers from every configured provider by `estimated_total_usd`, using dated
ECB reference rates. If two totals are equal, each provider keeps its own order.
The **New cloud** worker list uses the same order. Check `comparison.complete`
before calling its first offer the cheapest match; a failed catalog or
unavailable conversion makes it false.
Billing stays in each provider's currency.

- native amounts are euros, net of VAT, and `max_hourly` is read in euros for them;
- each estimate is for a run that starts now, billed per started hour and capped per
  calendar month (UTC) for compute, the workspace volume and the IPv4 address;
- `stopped_monthly` is the kept volume only, since a stopped Hetzner cloud releases its
  server and address;
- all current x86 server types in allowed `locations` are listed, independent of
  the fallback `server_types` list, and
  `availability` is Hetzner's advisory flag (`listed` or `unlisted`), never a filter;
- offers are `rentable`: Horizon creates Hetzner clouds.

Workers receive the catalog through `horizon-cloud-worker cloud-offers publish-hetzner`,
beside the price list, so older worker images keep taking RunPod prices unchanged.
On a machine set up for Hetzner alone, Horizon also runs `cloud-offers clear-runpod`
once on each ready worker, so RunPod prices sent before the key was removed are not
offered for the rest of their 20 minutes.

## New cloud

With a Hetzner binding, **New cloud** lists RunPod and Hetzner workers together.
Repository `min_cpu` and `min_memory_gb` define minimum requirements. The picker
shows cheapest, balanced and most powerful matches for the requested duration,
with storage and IPv4 included. All providers is the default browsing scope;
provider buttons narrow it. In stock only starts checked; below-minimum workers
start hidden and can be inspected but cannot be selected. Hetzner's availability
flag is advisory. **In stock only** does not hide an unlisted type. Its row shows
**Unlisted · advisory**, and the three picks can use it. When the settings exclude
locations from the Hetzner catalog, the dialog shows a note with their count.
The list does not show those locations. The fallback server types do not
restrict the catalog or add to the exclusion count.

The estimate uses USD for comparisons and retains euro prices for billing.
Reference rates come from the [ECB](https://www.ecb.europa.eu/stats/policy_and_exchange_rates/euro_reference_exchange_rates/html/index.en.html),
with their date shown. Quotes over seven days old or dated in the future are
refused. The UI fetches rates in the background, refreshing every six hours;
CLI and worker offer queries need read-only HTTPS access to
`data-api.ecb.europa.eu`. Missing rates leave native prices visible and the
comparison incomplete. These estimates exclude taxes and invoice conversion
fees; reference rates are informational and are not an invoice exchange rate.

A Hetzner row selects that exact server type and location. Start cloud records
its provider and explicit placement; a capacity refusal never rents a different
type or location. Refreshing prices preserves the chosen identity. A removed
or incompatible choice blocks Start until another worker is selected. Saved
worker specifications preserve explicit placement through stop, resume,
reconnect and image rebuild, including CLI lifecycle calls with plain settings.
Legacy records without an explicit choice retain their configured fallback policy.
Changes to the fallback list do not replace an explicit worker choice. The chosen
location must remain permitted. The selected type must remain compatible with the profile.

To deploy a listed offer through the CLI, save the selected offer object as JSON
and pass `--worker-choice OFFER_JSON_FILE` to `cloud_deploy deploy`. The CLI
resolves its provider/type/location against a fresh catalog and machine policy,
then verifies repository minimums; caller-supplied resource or price fields do
not override those checks. `cloud_offers` remains read-only on host and worker
MCP interfaces; its offer identity can be used with this explicit CLI deployment
path. No worker is rented by inspecting offers.
