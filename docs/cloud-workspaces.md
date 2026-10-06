# Cloud workspaces

A cloud is one remote development container. New shell and agent panels share its
Git checkout; branches and additional worktrees are created manually. Browser and
Device panels share that cloud's runtime.
RunPod and Hetzner provision workers. Daytona and Fly.io appear in labelled design
fixtures.

Cloud deployment and lifecycle control currently require a Unix host with supported
file and directory synchronization. Windows cloud operations fail before state or
provider mutation until durable directory updates are implemented; ordinary local
sessions and preserved cloud metadata remain available. The standalone provider
crate remains portable. Windows cloud durability is tracked in #823; native Device
platform qualification remains separately tracked in #741.

## Tailnets (auth-key MVP)

**Settings → Tailnets** stores named Tailscale auth keys in the machine's OS
credential store. Names and opaque IDs are the only saved catalog metadata; keys
are never returned by CLI/MCP, loaded back into the form, or included in project
configuration. Add multiple networks, replace a key, or remove a saved binding.
Use a preauthorized, non-ephemeral key; reuse requires a reusable key.

Choose **None** or a saved network in **New cloud** before provisioning. A cloud
retains that choice after allocation, including stop/resume. Removing a binding
does not move its existing clouds to another network. Remote Hosts are outside
this MVP. OAuth enrollment, API-based administration and Remote Hosts are
follow-ups to #1166.

`cloud_companions` lists saved network names/IDs. `cloud_companion_ensure_ready` accepts an
optional `tailnet`: a listed ID or `"none"`. This is a
provisioning choice, subject to the existing owner/grant and first-allocation
confirmation gates. Identical retries retain their original choice; a changed
retry or changing a provisioned cloud is refused. Never pass an auth key to a
tool. Omitting `tailnet` retains the saved choice.

The stock worker includes pinned Tailscale binaries and persistent userspace
networking. Its root control lane receives the key through private pinned SSH
stdin and passes it to enrollment through an anonymous memory file. It never
saves the auth key. Node state and the administrative socket are root-only;
workspace code, agents, browsers and desktop services run as UID 10001 without
capabilities or privilege escalation. The cloud volume retains node identity
across worker restarts. Provider volumes that cannot enforce ownership/modes are
refused. Custom images must advertise `horizon-tailnet-contract=1` before a
selected-network cloud can be allocated; **None** remains compatible with older
images.

Agents discover ACL-visible devices in
`/run/horizon-tailnet-devices/devices.json` (names, addresses and online state
only). HTTP/HTTPS and SOCKS proxy environment variables provide outbound access;
TCP tools without proxy support need an explicit SOCKS proxy. This is not a
kernel VPN interface and does not add inbound services or route access beyond
Tailscale policy. The daemon continues independently of the controller, with
state on the cloud's persistent volume. The newer signed project-session runtime
currently refuses a tailnet until its own unprivileged runtime is qualified;
the ordinary cloud panel's shell/agent runtime uses the isolated launcher.

## Companion declarations

Version 1 configuration accepts optional companion repository metadata:

```yaml
companions:
  service:
    repository: example/service
    profile: cpu
  consumer:
    repository: example/consumer
    profile: gpu
    placement: same_worker
```

Repository identities currently use GitHub `owner/repository` notation. Aliases
start with a lowercase letter and contain lowercase letters, digits, `_` or `-`.
Profiles refer to the companion's configuration, not the declaring repository's
profiles. Local checkout paths and target cloud IDs belong in machine-local state.

`placement` defaults to `cloud`: the companion runs on its own cloud and is
selected in the cloud panel as described under
[companion access](#companion-access-in-cloud-panels). A declaration never starts
a cloud or authorizes access. The selection pins the source session/workspace,
source cloud, alias, declaration, and target cloud. A changed declaration,
including a changed placement, or a missing target requires a new explicit
selection rather than rebinding to another matching repository. Multiple matches
stay distinct.

`placement: same_worker` declares a sibling for repositories coupled at build
time, such as a native library and the application that consumes its binaries.
A declaration only offers the sibling: it is built into a cloud's image only when
chosen in **New cloud**, as described below. A same-worker sibling never
provisions, starts or stops a cloud, and it is not listed among the
separate-cloud companions, their grants or the worker's companion catalog.

When the loaded configuration declares same-worker siblings, **New cloud** shows
**Siblings on this worker**: one row per sibling with a checkbox, its repository,
alias and profile, and the path of its local checkout. Every row starts
unchecked, because checking a row is what authorizes the sibling. When the
directory beside the primary checkout named after the sibling's repository has
the declared GitHub origin, its path is filled in; type another path or choose
**Browse…**. A checked sibling is checked in the background against the committed
revision being launched, and its row shows either the commit that will be built
in or what to change, such as a checkout of another repository, a checkout
without a commit, a missing profile or build section, or a directory name that
collides with the primary's. **Start cloud** stays disabled while any checked
sibling is being checked or was refused. The chosen checkouts are saved with the
cloud in this machine's session state, never in committed configuration. The
first deployment pins each sibling's committed `HEAD` before the image is built,
and the cloud card lists the pinned siblings with their commits. Pinned siblings
cannot change later; create a new cloud to choose others. **Rebuild image &
restart** layers each sibling's latest committed recipe while its checkout on the
worker stays at the pinned commit; the card then also names the commit the image
layer was built from. Cloud creation has no CLI or MCP operation, so siblings are
chosen only in New cloud.

On the worker, each sibling is checked out beside the declaring repository in a
directory named after the sibling's repository name without the owner, so
relative paths such as `../consumer` in repository scripts keep working.
Validation enforces what that layout needs: same-worker
siblings in one configuration have distinct repository names, compared
case-insensitively, that do not start with a dot. A clash with the declaring
repository's own name can only be detected when a cloud is created.

Horizon versions that predate `placement` reject a configuration that uses it,
including for launching the declaring repository's own cloud.

## One-time machine setup

In an existing workspace, choose **Cloud** from the panel-creation menu (or
**Cloud > New cloud**), enter a title, and press Enter. Horizon discovers the Git
root from the workspace directory, loads `.horizon/cloud.yml`, and uses its named
default profile. Preparation runs while you type. A configured launch starts
provisioning immediately after submission, without a separate Deploy action.
**More options** contains the container disk, repository and committed revision.
Only committed source is transferred; local changes stay on this computer.

Missing account settings open a repair form without losing the title or target
workspace. Enter the compute API key and choose API-key or subscription login for
the profile's agents. **Save and start** continues the submitted launch. A dedicated
SSH identity is created when needed and keys stay in private machine-local files.
Blank replacement fields preserve saved keys. Subscription login happens through
the actual agent on the worker; worker readiness does not prove authentication.
Open **Cloud > Cloud settings** to change machine defaults without launching.
On narrow windows, Cloud appears in the toolbar overflow.

Cloud allocation requires a saved session so worker identity survives reconnect.
An isolated test instance can use its own disposable saved session and private home.

Install Git (and Git LFS for repositories that use it), OpenSSH, and Docker with BuildKit/buildx. Configure Docker registry
authentication in a private configuration directory using `docker login` with
`--password-stdin`. Use separate repository-scoped push and pull credentials;
give the provider only the pull binding. Build the generic image using
[`examples/cloud-worker`](../examples/cloud-worker/README.md).

For guided private-image setup, rotation, revocation and shared CLI/MCP controls,
see [private registry bindings](private-registry-bindings.md).

For manual setup, create an Ed25519 SSH identity and store the provider API key in a
private file (0600 on Unix). Put bindings in `~/.horizon/cloud/settings.json`; all file paths
are absolute. This file contains paths and references, never literal API keys:

```json
{
  "runpod_key_file": "/private/runpod-api-key",
  "ssh_identity_file": "/private/cloud_ed25519",
  "docker_config": "/private/docker-config",
  "registry_pull_auth_id": "provider-pull-credential-reference",
  "cpu_flavors": ["cpu3c"],
  "gpu_types": ["NVIDIA RTX A6000"],
  "data_centers": []
}
```

`cpu_flavors` lists preferred CPU flavors. RunPod CPU pods take 2, 4, 8, 16 or
32 vCPU. The flavor fixes memory per vCPU (2 GB for `cpu3c`/`cpu5c`, 4 GB for
`cpu3g`/`cpu5g`, 8 GB for `cpu3m`/`cpu5m`) and limits container disk per vCPU
(10 GB for `cpu3*`, 15 GB for `cpu5*`). Horizon sends the preferred flavors that
offer the profile's vCPU count, memory and container disk; when none do, it uses
the flavor with the least memory per vCPU and price that does. For example, an
8 vCPU, 32 GB profile uses `cpu3g` when only `cpu3c` is preferred. A size no
flavor offers fails validation before the image is built.

For rootless Docker, set `docker_host` to its Unix socket URI. Public images can
use a null `registry_pull_auth_id`. Optional `anthropic_api_key_file`,
`anthropic_workspace_id` and `openai_api_key_file` bind explicit API authentication;
otherwise agents use their own interactive login. OpenAI authentication uses the
agent CLI's supported `login --with-api-key` command with SSH stdin; credentials
and login output are never included in command arguments or progress logs.
Agent login state stays on the worker. Workspaces
and profile names do not create separate credential sets.
Reconnect removes unbound API-key files, including interrupted-upload staging
files, while retaining supported subscription login state. Existing agent
processes can retain authentication already loaded into memory or their
environment; start a new session to apply the changed authentication choice.

Optional repository Git authentication uses explicit `git_credentials` bindings.
See the [worker credential setup](../examples/cloud-worker/README.md#optional-git-credentials)
for private file permissions, repository matching, removal and token-scope limits.

## Provider API and storage requirements

Direct root SSH endpoints accept numeric IPs and validated ASCII DNS hostnames.
Provider inspection and cleanup never resolve hostnames; the bounded OpenSSH
transport performs resolution when connecting. Proxy command strings and
non-root endpoints are not executed. Older worker records retain their numeric
address format; hostname destinations require the updated worker for companion
connections.

The shared UI, CLI and MCP adapter uses the [RunPod REST v2 API](https://docs.runpod.io/api-reference-v2/overview)
for workers, capacity, registry credentials, storage and billing. Existing saved
worker identities and volume journals remain readable. Upgrading does not replay
an unresolved creation request or turn an observed resource into a fresh allocation.
CPU availability is queried for the requested vCPU count and Pod product; a positive
catalog answer is not a reservation. Configured compute preferences are tried in
order only after a definite rejection. Ambiguous creation responses remain fenced.

The optional worker-local idle-stop watcher retains its separate GraphQL stop call.
It uses the provider-injected Pod-scoped credential, which the live qualification
for #966 found was refused by REST and accepted by GraphQL. This migration changes
the shared account-credential adapter; it does not put account credentials inside
workers or remove that independently qualified idle-stop path.

CPU workspaces use network storage and require an account with no Serverless
endpoints. Profiles select `storage.volume_tier: STANDARD` (the default) or
`HIGH_PERFORMANCE`. Placement and allocation require that exact tier; Horizon
never silently substitutes another tier. The v2 API does not expose complete mounts for stale or scaled-down
Serverless workers. Horizon therefore refuses new CPU storage allocations, initial
attachment and storage deletion while any Serverless endpoint exists. This includes
unrelated endpoints with no currently configured volumes. Use a separate account
for these CPU workspaces; Horizon never removes unrelated endpoints to pass this
check. The API credential must permit reading Serverless endpoints as well as Pod,
volume and registry operations. Failed or incomplete listings block mutation.

GPU workspaces use their Pod's persistent mount and retain the requested CPU/RAM
minimums. `PROVISIONING` and `STARTING` keep the same bound identity while readiness
is checked; they do not mean a worker is missing or authorize a replacement.

## Repository setup and deployment

If the repository has no `.horizon/cloud.yml`, enter its directory in
**Cloud > New cloud**, expand **No cloud configuration yet?**, and open a setup
agent. This is a normal local agent panel using its existing local login; remote
worker API-key bindings are not automatically exported to it. The agent inspects
repository requirements and proposes the selected capabilities before preparing
files. Review its changes and prerequisites, run the repository's checks, and
commit the setup. Return to New cloud and choose **Read .horizon/cloud.yml**.
Changing the repository or encountering invalid YAML clears previously loaded
profiles, so a stale profile cannot be deployed accidentally.

For the default launch path, commit `.horizon/cloud.yml` using the
[example](../crates/horizon-cloud/examples/cloud.yml).

To test an existing worker image without committing the test settings, open
**More options** and select **Use local image-only settings**. Horizon reads the
working copy of `.horizon/cloud.yml`, including untracked files, and offers only
profiles with no `build` section. Set the local default to an image-only profile;
a build default is refused instead of selecting another CPU or GPU profile.
Set `image` to a registry-accessible tag or
digest, resource minimums, GPU requirement and explicit capabilities. The image
still passes the worker contract check before allocation and is pinned by its
resolved registry digest. Use **Read .horizon/cloud.yml** after editing settings.

Local mode captures the selected profile in this cloud's machine-local state;
later file edits do not change that cloud or its reconnect settings. The selected
**Committed base revision** independently controls the application source.
Local mode cannot change source packaging or declare companion repositories;
those remain committed-source features. Credentials always stay in machine-local
bindings. Closing the dialog resets the next launch to committed configuration.

Image-only profiles omit `build`. Repository Dockerfiles build from the selected
committed tree, honor `.dockerignore`, and reuse local BuildKit layers. Dirty and
untracked files are excluded. Selected Git LFS objects and recursively pinned
submodule commits must be available locally. They are verified before allocation,
transferred without local Git configuration, and checked out independently for
each agent. LFS content goes into the transfer archive straight from the local LFS
store and is verified as it is written, so local staging holds it only once. Only attributes from the selected commit determine LFS hydration.
Extended LFS pointer formats are rejected explicitly. Source repositories must use
SHA-1 object IDs and UTF-8 paths; unsupported formats fail validation before
compute allocation.

Submodules carry their full history by default. A repository whose submodules are
large third-party trees can send only each pinned commit and its tree instead:

```yaml
source:
  submodule_history: pinned   # default: full
```

The worker records such a submodule as shallow, so `git log`, `git describe` and
`git blame` inside it see a single commit. Build scripts that derive a version from
submodule history break under `pinned`, which is why it is opt-in. For a native
library with six third-party submodules, `pinned` cut their transfer from 1.19 GB
to 0.19 GB. The primary repository and each same-worker sibling keep their full
history either way. Each repository reads the setting from its own committed
`.horizon/cloud.yml`; a sibling without one gets full history. Worker images older
than this option report no `horizon-source-shallow-contract=1`. Horizon reads that
from the image contract it checks before allocating a worker and packs full history
for such an image up front. Older Horizon builds refuse a `.horizon/cloud.yml` with
a `source` block.

A repository whose tests need only some of its LFS content, such as a large set of
video fixtures, can leave the rest out of the transfer:

```yaml
source:
  lfs:
    include: [src/**, fixtures/reversal/**]   # default: every path
    exclude: [fixtures/reversal/*.raw]        # applied after include
```

Patterns are git-lfs fetch patterns (`lfs.fetchinclude`/`lfs.fetchexclude`): at most
64 in all, each non-empty, at most 256 characters, without commas and without Unicode
control, format, surrogate or private-use characters. The worker applies the same
per-pattern rules. Horizon also caps the patterns at 8,192 characters in all, so its
git-lfs call fits a Windows command line. Horizon asks the local git-lfs which of the repository's
own LFS paths the patterns exclude and sends every other object; the worker checks with
its git-lfs that each path left out is excluded, then sets the same patterns in the
repository's configuration, so worktrees keep those paths as pointer files and
`git status` stays clean. Submodule LFS content is always sent, and so are empty
objects, paths containing a newline and paths ending in a carriage return, which
git-lfs's line-based listing cannot identify.
Objects the selection leaves out need not be fetched locally; every other object,
including those exceptions, is verified before allocation. An image that cannot honor the selection receives every object, which
then must all be local. For an application whose LFS content was 3.23 GB, 3.14 GB of it video
fixtures, excluding the fixtures its tests do not read is the largest transfer
saving. Images without `horizon-source-lfs-selection-contract=1` receive every LFS
object; Horizon decides that from the image contract before allocation, as for pinned
submodule history.

A GPU profile whose image needs a recent CUDA can set `min_cuda_version` as
`major.minor`, for example `min_cuda_version: "12.8"`. A host's driver limits the
newest CUDA it runs (CUDA 13 needs driver 580 or newer), so without the field a
worker can land on a host too old for the image. Versions compare as numbers, so
12.11 is above 12.2, and CPU profiles reject the field. Horizon sends the floor
as `gpu.minCudaVersion` in each v2 worker creation request, alongside the chosen
GPU type and allowed data centers. RunPod enforces it during placement; a definite
capacity refusal may try the next configured GPU type with the same floor. An
uncertain response never permits another create. The GPU stock New cloud shows
does not yet take the floor into account.

A GPU profile can also set `min_gpu_memory_gb`, such as `min_gpu_memory_gb: 24`,
so New cloud offers only GPU types with at least that much GPU memory. It takes 1
to 1024, and CPU profiles reject it.

Before each image build, Horizon looks up the release npm currently tags `latest`
for every supported agent CLI. It passes them as the `HORIZON_CODEX_VERSION`,
`HORIZON_CLAUDE_VERSION` and `HORIZON_GROK_VERSION` build arguments, next to
`HORIZON_AGENTS`, `HORIZON_BROWSERS`, `HORIZON_DESKTOP` and `HORIZON_BROWSERSTACK`,
and names the versions in verbose output. All three are passed because an image
can install an agent its profile does not enable. A failed lookup stops the
deployment before the build. In a custom Dockerfile, declare each version argument
directly before that agent's install step and install the exact release, for
example `ARG HORIZON_CLAUDE_VERSION=` followed by
`RUN npm install -g "@anthropic-ai/claude-code@${HORIZON_CLAUDE_VERSION:-latest}"`.
A newer release then rebuilds only that step and the steps after it, so place agent
installs after heavier toolchain steps. An install step without a version stays
cached at the first release it installed. Record the installed versions in
`/etc/horizon-worker/agent-versions.json`, for example `{"claude": "2.1.281"}`.
`horizon-worker-check` requires each enabled agent that has an entry to report
that version from `--version`, and probes an enabled agent without one with
`--version` alone; Horizon runs the check before pushing the image. A layer, such
as a same-worker sibling's recipe, that installs one agent should merge its entry
into the existing file rather than replace it, so the base image's agents stay
pinned; a layer that writes only its own entry still passes the check. The [example worker Dockerfile](../examples/cloud-worker/Dockerfile)
follows this pattern. Existing clouds keep the image they were built with; new
clouds receive the latest agents.

A custom Dockerfile on its own base, such as a project CUDA image, gets the
current worker helpers and scripts by copying the published helper artifact,
pinned by digest, instead of compiling them:
`COPY --from=ghcr.io/peters/horizon-worker-helpers@sha256:<digest> / /`. The
[worker README](../examples/cloud-worker/README.md#helpers-from-the-published-artifact)
describes what the artifact contains, where to find the newest digest and what
the base must still supply. `python3 examples/cloud-worker/check-markers.py IMAGE`
names the current contract markers an image lacks, such as siblings, session
environment or GPU lock.

Choose **New cloud**, enter its title, repository and base revision, load profiles,
then create and deploy. Horizon validates and uploads the image before allocating
compute. It resolves an immutable digest and checks the worker contract. Keep the
computer online until image/source upload and readiness complete. Expand verbose
output for build and push progress. Failures retain a retryable card and the
persisted operation identity.

### Choosing a worker

New cloud is a wide dialog: the worker catalog on the left and a summary on the
right, with **Start cloud** and **Cancel** in the action bar under both. Pick a
**Profile** from `.horizon/cloud.yml`; its kind decides between CPU and GPU workers.
The profile's `min_cpu` and `min_memory_gb` are CPU resource minimums, and
`min_gpu_memory_gb` is the GPU memory minimum. The picker names the limits and
counts workers hidden by them. **Show workers below requirements** is unchecked
by default and reveals those
workers with a reason; they cannot be chosen. There are no separate default-size
settings. Legacy `cpu` and `memory_gb` keys remain accepted as aliases with the
same meaning. To migrate a profile, rename those keys without changing the values;
do not specify both names for a resource. Saved worker records keep their existing
CPU and memory keys and their allocated sizes.

**Machine** combines the current catalogs of supported, configured providers:
RunPod for CPU and GPU, and Hetzner for compatible CPU profiles. **Cheapest**,
**Most powerful** and **Balanced** use estimated total cost for the chosen run length,
including storage and Hetzner IPv4, in USD using dated ECB reference rates. Billing
stays in each provider's currency. An incomplete catalog or missing exchange rate
keeps the offers visible without claiming a global cheapest choice. The initial
choice is the cheapest matching worker when the comparison is complete; explicit
choices stay selected during refresh. Provider buttons narrow both the cards and
full list. A single-provider scope ranks in its billing currency without exchange
rates; cross-provider ranking requires a current dated quote. With **In stock only**
checked, starting points include only workers reported in stock, using the exact
capacity check for the selected CPU size. If none match, all three starting points
are empty. Uncheck it to include unavailable workers.
Hetzner availability is advisory. Horizon examines the exact type and location
again when it creates the cloud. Thus the filter does not hide an unlisted Hetzner
type. Its row shows **Unlisted · advisory**, and the starting points can use it. CPU workers are more powerful with more vCPUs and then more memory; GPU types
rank by price, which follows their performance more closely than their memory does.
Search and the **In stock only** filter are always visible above the full worker
list, with a count of the results and workers hidden by requirements.
The full worker list uses the order of the `cloud_offers` comparison. The workers
that meet the requirements come first, with the cheapest estimated total first.
The workers below the requirements follow in the same order. Each row shows the
hourly price in the billing currency. If the dialog can convert the estimate, the
row also shows the estimated total in the comparison currency. Without a current
exchange rate, a row in another currency shows only its hourly price, and the
dialog shows that the comparison is incomplete. If two totals are equal, each
provider keeps its own order. **In stock
only** is checked by default; uncheck it to show sold-out workers. Opening the
dialog or changing profiles restores these filter defaults. A CPU size is offered only when a
flavor can hold the profile's container disk. A GPU profile always requests one
explicit GPU type, the cheapest in stock to start with, which replaces the
`gpu_types` setting for every attempt, retry and redeploy of that cloud and is
named on its card. Choosing a card or row changes only the worker; nothing is
rented until the cloud starts.

The summary names the chosen worker, its stock where the cloud may go, and its
storage: RunPod CPU workers keep a **Standard** or **High-performance** network
volume, and GPU workers a pod volume, whose size is edited here for this launch
without changing the repository profile. The estimated cost lists compute per hour,
each kind of storage per month, and the month running and stopped:

- a standard network volume of a CPU cloud: $0.07 per GB for the first TB and
  $0.05 beyond it, billed whether the cloud runs or not;
- a high-performance CPU network volume: RunPod prices it per data center and does
  not publish the price, so it shows as not published and the monthly totals leave
  it out. Only data centers that hold high-performance volumes are offered;
- the pod volume of a GPU cloud: $0.10 per GB while running and $0.20 while
  stopped;
- the container disk: $0.10 per GB, billed only while running and cleared when
  the cloud stops.

RunPod does not publish storage prices in its catalog, so these are list prices
with the date they were checked. CPU prices are the dearest flavor a size may land
on, marked "up to" when there is more than one.

Catalog stock is RunPod's own: each data center reports its GPU types and CPU
flavor families with high, medium or low availability, and a GPU type it does not
list is out of stock there. The chosen CPU size gets an exact stock check of its
own, which the summary and the data center choices show. Prices and stock refresh
every 15 seconds while the dialog is open, and **Refresh** fetches them at once.
The dialog says how old they are. When a refresh fails, the last good catalog stays
on show with its age and the reason, but it is never treated as current; once it is
more than an hour old, Start cloud waits for a successful refresh.

Hetzner, when configured beside RunPod, is chosen under **Provider** and keeps its
own size and location fields and price card. Adding it to the catalog is a
follow-up.

### Starting once a sold-out worker returns

When the chosen worker is out of stock where the cloud may go, the action bar
offers **Start new cloud once available**. Checking it turns Start into **Start
when available**, which needs one exact data center, so the watch never broadens
the chosen worker or place. While it waits, the summary names the data center and
the price shown when the watch started. Every 15 seconds it checks current stock
for that worker there and starts this cloud once when stock returns, at no more
than that price: while the price is higher the watch waits, and it needs a known
price to start at all. A failed start does not retry. The watch lasts while the dialog stays open;
closing it, Cancel or **Stop watching** ends it, and the fields stay locked until
then.

Agents in Horizon panels can ask for the same prices through the `cloud_offers`
tool of Horizon's MCP server, for example "the cheapest GPU with at least 24 GB in
Europe for 10 hours". It returns up to 50 offers, cheapest estimated total first:
each with its hourly price, an estimate for the expected hours including 20 GB (or
the requested size) of workspace storage and, for RunPod, the default 20 GB container
disk of a profile, for a CPU size the flavors a cloud of that
size requests (from Cloud settings) priced at the dearest, since RunPod picks one, availability (CPU sizes are confirmed
when a cloud is created), the regions with that GPU in stock, and that it runs on
provider-operated hosts. The running Horizon answers, fetching prices when they are
missing or older than 15 minutes, and says how old they are. Without a running
Horizon or cloud settings the tool fails rather than returning old prices. It only
reads prices; renting stays with the person.

Agents on a cloud worker get the same answer, through `cloud_offers` on the
companions server or the browser server, from the prices its Horizon last sent.
While any cloud is ready, Horizon refreshes prices every 15 minutes and sends each
fresh list to every ready worker over the SSH connection companions use; a worker that misses one is asked again after five minutes. Only
prices travel: the RunPod key stays on this computer. A worker without prices, or
with prices older than 20 minutes, answers with an error instead of old prices.
When this machine no longer has a RunPod key, Horizon tells each ready worker once
to drop the RunPod prices it holds, so its agents stop being offered them at once;
worker images built before this refuse it and keep the old prices until they are
20 minutes old.

**Data center** is part of the catalog, not an advanced setting: **Any data
center** (the default, where Horizon picks one with stock) or a region, each with
how many of its data centers have the chosen worker in stock. Every allowed data
center is shown immediately, grouped by region, for choosing exactly one. Region
choices select all compatible data centers in that region. Sold-out regions and
data centers stay visible and selectable. Data centers that cannot hold the chosen
workspace volume stay visible with **Storage unavailable**, and cannot be chosen.
The machine's `data_centers` setting still limits
what is offered, and the dialog says how many other data centers it excludes.

The choice is saved with the cloud: every attempt, retry and redeploy asks the
provider only for the chosen data centers. A cloud's workspace stays in the data
center it first starts in, and a stopped cloud resumes there, so the dialog says
so under the data center choices. Once a worker exists, the cloud card names its data
center and region, also for a cloud placed in any region. Horizon looks the region
up in RunPod's data center list, which covers every data center even after the
`data_centers` setting changes, and fetches that list once if New cloud has not
loaded it yet.

Until a worker is requested, including after a failed attempt or a definite
provider rejection, the cloud card offers vCPU and memory drop-downs for CPU
profiles, listing only sizes RunPod offers with the profile's container disk.
The next attempt reuses the built image and applies the new size and the current
`cpu_flavors`, `gpu_types` and `data_centers` settings. Once a worker is
requested these drop-downs are fixed. A ready CPU worker instead uses the
confirmed resize operation described below, which replaces its compute while
retaining its network workspace. After confirmed deletion, the pre-allocation
drop-downs return until the replacement worker is requested.

Add normal panels inside the cloud using the existing panel picker. Choose
Default, Rows, Cols or Grid independently for each cloud. Cloud and workspace
membership is permanent. Removing the last cloud from a workspace also removes the
workspace when no panel remains in it; a new cloud being created there keeps it
until that creation ends. **Close All Panels** keeps its workspace. Full screen
opens one cloud; F11 opens its focused panel, and Escape returns through the
previous views. The shared canvas supports zooming
out to 5%; Fit reserves space for the overview controls and minimap on smaller
windows, and saved views use the same zoom limits as manual zoom.

Cloud workspaces stay in the main window so their frames, runtime controls and
child panels remain together. Use cloud Full screen for a focused view. Move an
ordinary detached workspace back to the main window before creating a cloud in
it. Older saved detached-cloud entries restore in the main window, retaining
their cloud and panel identities.
Workspace layout presets are disabled for workspaces containing clouds. Each
cloud keeps its own layout; ordinary panel resize collision handling does not
move cloud members. Drag the bottom-right corner of a cloud frame to resize it.
With Rows, Cols, or Grid, every visible member grows or shrinks together to fill
the frame, down to the ordinary panel minimum. Default placement keeps each
panel where it is and will not shrink the frame through those panels. A collapsed
cloud has no resize corner. New clouds are placed relative to their workspace and are
reconciled before the overview is fitted.

## Sessions and lifecycle

Each agent has a stable tmux session. New shell and agent panels share the same
checkout, including branches, commits and uncommitted files. Coordinate concurrent
edits or create separate worktrees manually when isolation is needed. Existing
sessions retain their recorded paths; reconnecting does not migrate them. Older
worker images must be rebuilt before adding shared-checkout panels. Closing Horizon detaches presentation while
the worker and tools continue. Reconnect inspects the same worker, restores SSH
tunnels and attaches existing sessions. Reconnect also restores closed terminal
views from their saved remote references.

After a restart, each panel of a cloud waits until its cloud is ready. Until
then, the panel shows that Horizon reconnects the cloud. When the cloud is
stopped, the panel shows that the cloud is stopped and that **Resume worker** on
the card restores the panel.

A ready RunPod CPU cloud can **Resize compute** or **Grow workspace** from its
runtime card. Compute replacement retains the same network workspace but stops
processes, discards temporary container files and reconnects recorded sessions on
the new worker. The requested size must be available; charges change with it.
Workspace growth keeps the worker, increases storage charges and cannot shrink
again. Review and confirm each change before it starts. An interrupted transaction with a retained journal
offers its exact target through **Retry resize** after restart; competing lifecycle
operations remain blocked until recovery finishes. Once compute replacement commits,
the journal is removed and startup resumes readiness and reconnection automatically. GPU pod-local
storage and other providers do not offer compute replacement through these
controls. The host transaction requires Unix directory durability.

Before readiness, Horizon verifies the provider's assigned container disk and
persistent volume sizes and the `/workspace` mount path against the profile.
Missing, undersized or differently mounted storage blocks source and agent-credential
transfer. New CPU clouds allocate an owned network volume of the profile's selected
tier in a data center that has the cloud's exact CPU size and tier in stock, honoring configured location
preferences, and attach it at worker creation. The provider catalog rates only CPU
flavor families, so Horizon confirms stock for the requested vCPU and memory size
before allocating storage; when no configured data center has it, no volume is
created. New CPU profiles require `storage.volume_gb` between 10 and 4000 GB;
unsupported sizes are rejected before placement lookups or allocation. Saved
allocation journals remain reconcilable and deletable under their original sizes.
GPU clouds and existing deployments retain their Pod-local
storage contract. Unexpected volume identities, locations or capacities block
readiness. CPU mount verification uses the current provider API because the legacy
worker response omits CPU network attachments. Deletion checks current mounts on
all listed workers before removing storage. Network storage remains billable when the worker is stopped or deleted;
explicit cloud deletion removes the worker first, then confirms deletion of the
volume allocated and tracked by this cloud. Separately attached network volumes
are not adopted or deleted: their files and credentials remain, and their storage
charges continue. Cleanup messages preserve that distinction even for older
records whose worker attachment details are no longer available.
Failed cleanup keeps the cloud available for retry and prevents local removal.
Allocation and deletion are journaled so uncertain responses never create a second
volume or adopt an unrelated one. The worker remains allocated and bound to its original operation for
inspection or explicit deletion; retry does not create a replacement. Old saved
worker records without storage fields remain readable. Deployment and reconnect
attempts that reach readiness inspect fresh provider data; this does not change
panel eligibility for a cached Ready record when an earlier preflight fails. This capacity check does not itself prove that files
survive a provider restart; persistence still needs a live recovery test.

Stop ends running processes; storage can remain billable. Idle stop is off unless
a profile opts in: set `idle_stop_minutes` (10 to 1440) so a dedicated worker stops after that
long without agent activity. The worker
counts as active while any agent terminal prints output or its container uses
at least half a CPU core, so a quiet build keeps it running. On RunPod the worker
stops itself, even while this computer is offline, using the provider's credential
scoped to that worker, and never deletes anything. Hetzner gives a worker no
credential that could stop it, so there Horizon makes the stop, only while it is
running, and the stop releases the server and keeps the volume as Stop does; see
[Hetzner idle stop](cloud-hetzner.md#idle-stop). The rest of this section describes RunPod. Choose **Check provider** on the card afterwards: a worker confirmed
stopped offers Resume like an explicitly stopped one, and nothing resumes it
automatically. Profiles without the field never stop on their own, and workers
shared across workspaces and profiles with hosted devices do not support it.

On RunPod, the same opt-in lets an agent stop its worker when its task is done, such as when
its pull request is merged, without this computer. Agents get a
`stop_this_worker` MCP tool (or run `horizon-worker-stop --reason "..."`) with a
one-line reason. The worker identifies the requesting agent session itself,
refuses callers outside a Claude, Codex or Grok session (including shell sessions), and
refuses while another agent session printed output in the last two minutes, while
the container is busy, or when it cannot check its sessions or CPU use; the calling agent's own
output does not count. The reason and the requesting agent are kept on the
workspace volume, and after a resume the cloud card shows them, for example
"Stopped by claude 2 h ago: PR 12 merged". A redeployed cloud starts without the
previous worker's reason. Resume
starts the same worker when provider capacity permits, but lost processes are
reported rather than silently recreated. Delete permanently destroys the worker
and its Pod-local files. Uncertain creation responses are reconciled before any
retry; Horizon never allocates a replacement for a missing worker automatically.
After managed workspace storage cleanup is confirmed, **Redeploy cloud** on the
same card allocates a new worker and managed storage for that cloud. It keeps
the repository, revision, profile, panels and saved sessions, uploads source
again, and reuses the recorded image. vCPU and memory can change before the new
worker is requested. Redeploy is refused while worker termination, hosted-device
release, or managed storage cleanup is unfinished. Separately attached network
volumes stay untouched.

When creation needs confirmation, choose **Check provider** on the cloud card.
This action queries the original operation without building images, preparing
source, transferring agent credentials or changing provider resources. If the
provider supplies a worker ID, enter it under **Provider-confirmed worker ID**;
Horizon verifies its cloud identity and immutable image before adopting it.
Conflicting matches remain blocked. A missing ID, an empty listing, a lookup
authorization failure and elapsed time do not prove that creation failed.
Keep the operation record and ask provider support for an authoritative outcome
when lookup cannot resolve it. There is no force-reset or replacement action.

The [provider create API](https://docs.runpod.io/api-reference/pods/POST/pods)
documents no creation idempotency key or request-status lookup. Its
[billing history](https://docs.runpod.io/api-reference/billing/GET/billing/pods)
identifies worker IDs, not Horizon operation IDs; an absent billing record is
not proof that no worker was created. A definitive rejection received from the
original create request permits another explicit deployment attempt. Lookup
errors cannot provide that permission. A previously bound worker that disappears
is reported as missing. A non-running match retains its verified worker ID but
is reported as inactive: a desired termination status is not proof that deletion
has finished. Check again or explicitly delete that same worker. Only the
explicit deletion flow confirms cleanup. The terminated worker identity remains
until that cleanup is confirmed; **Redeploy cloud** is the later explicit action
that starts a new allocation.

After a worker-service crash, a private journal that durably confirms a remote
device was released can be cleaned up without the old provider credentials.
Unreleased, malformed or mismatched identities remain blocked until their exact
release can be verified.
Recovery after worker-service loss is host-only: the journal records the original
requester, which may no longer own a transferred allocation. Agents cannot reclaim
that historical ownership. Use Horizon's explicit remote-device release action
to reconcile the exact retained allocation through the authenticated host.
If saving confirmed release fails, the allocation keeps its exact identity and
blocks cleanup. Reconcile after restoring writable storage to retry that save;
the live process retains the provider's release result and need not query it again.

Browser panels display their actual controller. Native VNC Device panels are
read-only viewers; worker-local device tools perform input and report its agent.
Choose **Add desktop viewer** on a ready cloud to view its worker desktop.
All agent tools run on the worker, independently of the laptop connection.
The worker supports the public browser tools and standalone device input tools.
`device_panel` manages viewers in a running Horizon window, so it is unavailable
inside the headless worker and returns `viewer_requires_horizon` immediately.
It does not substitute a browser viewer or depend on the laptop for device input.

The development example `cargo run -p horizon-core --example cloud_deploy -- ...`
uses the same coordinator. Run it without arguments for its command synopsis.
It supports image preparation without allocation, deployment, stop, resume,
reconnect, endpoint, deletion, `rebuild SETTINGS STATE_ROOT PROFILE` with `continue-rebuild` and
`cancel-rebuild`, and `reconcile SETTINGS STATE_ROOT [WORKER_ID]`. Reconciliation prints a
structured outcome without worker environment or credentials and an explanation;
the UI and this harness use the same locked coordinator and provider policy.
An unresolved outcome is a successful check, not permission to deploy again.
`deploy` on a cloud whose worker and managed storage are confirmed deleted
starts the same explicit redeploy as the card; it does not replace a missing or
unresolved worker.

`resume` does what **Resume worker** does on the card: it starts a stopped
RunPod worker, or clears the fence of the server a Hetzner stop deleted, so a
new one can be created. It then prints the next step: `reconnect SETTINGS
STATE_ROOT` reconnects the recorded cloud as **Reconnect cloud** does. It reads
the repository, revision, profile and same-worker siblings from the deployment
record, so no `--sibling` options are needed, then waits for readiness and
relaunches the recorded sessions. It never creates a cloud's first worker, never
starts a stopped one and never reopens a deleted cloud; those stay with `deploy`
and `resume`. A Hetzner stop deletes the server, so the reconnect after its
resume creates the new server on the same workspace volume, as the card's does;
it does so only on a volume a server has held. The harness loads the machine
settings without the placement the card records for a cloud, so a resumed
Hetzner cloud's recorded server types and locations are refreshed from the
settings file. The workspace volume still fixes the location.

The provider may assign a new public SSH port whenever the worker starts, for
example after a resume or an image switch. `endpoint SETTINGS STATE_ROOT` checks
the worker with the provider as `reconcile` does, creating, starting and deleting
nothing, and like `reconcile` records what it finds, such as a worker that stopped
itself. It then prints the running worker's current endpoint as JSON:
`host`, `port`, `user`, `host_key_alias`, `known_hosts` and `identity_file`. It
refuses a worker that is not running or whose host key Horizon has not pinned
yet. Scripts can then connect with Horizon's pinned host key instead of a cached
endpoint:

```bash
ssh -o StrictHostKeyChecking=yes -o HostKeyAlias="$alias" \
  -o UserKnownHostsFile="$known_hosts" -o GlobalKnownHostsFile=/dev/null \
  -o IdentitiesOnly=yes -i "$identity_file" -p "$port" "$user@$host"
```

Cloud creation has no public MCP operation yet, and the worker's browser/device
MCP tools do not allocate or reconcile compute. Agents in Horizon can start and
stop a checked companion cloud; see
[Starting and stopping companions from agents](#starting-and-stopping-companions-from-agents). This example is
an integration harness, not an installed user command.

### Rebuilding a cloud's image

A ready dedicated cloud can rebuild its worker image and restart on it, for
example to pick up newer agent CLIs or a committed change to its `.horizon`
recipe. Horizon resolves the repository's latest committed `HEAD` (uncommitted
changes are not used) and reads `.horizon/cloud.yml` there. It refuses when the
cloud's profile is missing there or differs from the one the cloud was created
with, naming the change: a running worker's size, capabilities, image repository
and build section are fixed. A CPU cloud's vCPU and memory are chosen when the
cloud is created, so different committed values for them are not a change. A
profile without a build section has no recipe to rebuild. Otherwise Horizon builds the recipe with the newest agent CLIs under a
new tag, validates the worker contract, pushes the image and verifies the
worker's pull binding. An unchanged image digest is reported, and nothing restarts.

Horizon then releases any hosted devices and switches the existing worker to the
new image through the provider's pod update, which keeps the worker ID. A Hetzner
cloud instead releases its server and starts a new one, with a new server ID and
address, on the same workspace volume; see
[Hetzner](cloud-hetzner.md#rebuilding-the-image). On either provider `/workspace`
(worktrees, agent logins, SSH host keys) survives; the container disk is reset and
running processes end. After readiness, Horizon relaunches each session's process
in its existing worktree without resetting it. Images without the relaunch
contract report those sessions lost, as Stop and Resume do.

The replacement is journaled in the deployment record before each provider
mutation, and older Horizon versions cannot read the record while the update may
be in flight. After an interruption, reconnect and **Check provider** only read
the provider: a worker already on the new image completes the switch, and anything
else stays blocked as a pending replacement. Continuing it sends the update again
only while the worker still reports its previous image. Cancelling it drops an
unsent update or switches the worker back and relaunches its sessions. Deletion
stays available throughout.

On the runtime card of a Ready cloud whose profile has a build section,
**Rebuild image & restart…** asks for confirmation and names these consequences
first. While it runs, the card lists the rebuild's steps with their durations and
offers **Cancel rebuild** only until the image switch is requested. A rebuild
cancelled while its committed recipe is read leaves nothing pending, and the card
offers **Reconnect cloud**; cancelled later, it stays pending. A pending replacement shows a notice with **Continue
rebuild** and **Cancel rebuild**, and Stop waits until it is resolved. Cancelling
a switch that may have been sent asks for confirmation, because switching back
restarts the worker again, and Horizon does not reconnect such a cloud on its own
at startup. The card keeps the outcome, such as an unchanged image or a session
that could not be relaunched, until the next deployment.

### Deployment progress

The cloud runtime card shows the active stage and elapsed time for completed
stages. Image push/download and source upload expose measured progress, bytes per
second and an approximate remaining time when a total and a stable rate exist.
BuildKit reports completed/discovered build steps. Provisioning and readiness show
current activity and elapsed time because the provider supplies no reliable ETA.
Ready shows the measured time to worker readiness for a successfully timed
deployment. It does not claim application-visible startup time or invent a time
for older records without measurements.
Cached image layers are excluded from transfer speed. Expand verbose output for
command details. Per-stage timing resets on retry. The recorded worker-readiness
duration survives reconnect.

After a successful deployment or reconnection the card shows how long it took,
a ribbon of its phases in order, and **Where the time went**, largest first:
repository checks, image build and push, the provider request, the image
download, worker boot, readiness checks, source upload and import, and sessions.
Workers whose image reports its container start separate the image download from
boot; older images count boot as part of the download. A resume includes the
provider's start request, and only a worker that was ready before counts as a
reconnection. The breakdown is kept
with the cloud and survives restarting Horizon. The `cloud_deploy` harness
prints the same phases and timestamps every line.

## Local network bridge

**Share local network** on a Ready cloud's card lets that cloud's agents reach
devices on the network this computer is on, over TCP, through this computer. It is
off by default and after every restart. See [Local Network Bridge](local-network-bridge.md).

## Planned shared workers

Explicit sharing of a compatible CPU worker across trusted projects is tracked in
[#805](https://github.com/peters/horizon/issues/805). The
[shared-worker contract](architecture/shared-cloud-workers.md) defines the proposed
identity, migration and lifecycle design. This is not yet a supported placement
choice; existing clouds continue to use dedicated workers.

### Companion access in cloud panels

In a saved session, each cloud panel lists the separate-cloud companions declared
in its committed `.horizon/cloud.yml`; same-worker siblings are not listed. Check a repository to allow access to an existing
cloud with that repository and profile in the same workspace. When more than
one cloud matches, choose its stable cloud ID first. A missing cloud cannot be
selected. The selection does not create, start, resume, or keep a worker running.

Running workers establish a dedicated SSH alias and a separate target worktree.
Ready means source-to-target SSH was verified. Stopped and unreachable states
remain visible; checking a stopped cloud leaves it stopped. Refresh retries
access through the existing SSH endpoints without contacting provider lifecycle
APIs. No overlay network is needed when those endpoints are reachable.

The source worker must be able to reach the target's published SSH endpoint.
Some providers refuse this connection when workers share a public address;
Horizon reports Unreachable and does not add a relay or start another worker.

Workers with enabled agents automatically advertise `horizon-cloud-companions`.
The read-only `cloud_companions_list` and `cloud_companion_inspect` tools expose
the same catalog as `horizon-cloud-worker companions list` and `inspect <alias>`.
Use the returned SSH alias and worktree with ordinary SSH, Git, and rsync. A
stale catalog loses Ready status; inspection can verify an unchanged connection
independently. These worker tools do not start or stop clouds.
The same server also offers `cloud_offers`, so agents on workers without browser
tools can rank cloud offers from the prices the owning Horizon last sent the
worker.

Uncheck to remove access. If either worker is offline, removal stays pending
until that original worker can confirm cleanup. The target then removes the
grant's worktree when it holds no changes and no untracked or ignored files, and
keeps it otherwise; the source drops the grant's key once the target has revoked
it. Existing shells and copied data cannot be recalled. Clear old selections before
retiring their source cloud. Changing the workspace, declaration, target worker,
or initial revision requires cleanup and explicit selection again. These clouds
share trusted shell access; this is not credential isolation. Both workers need
an image containing the updated companion helper and rsync.

### Starting and stopping companions from agents

An agent panel in Horizon can start or stop a companion cloud that the owner
checked on the source cloud's card, through the browser MCP server:

- `cloud_companions` lists the clouds in the agent's workspace with their declared
  companions, whether each is checked, its status and its target cloud ID.
- `cloud_companion_ensure_ready` takes the source `cloud` ID and the companion's
  `alias`. It reuses a running companion, resumes a stopped one and verifies
  source-to-target SSH access and the target's repository environment. On
  Hetzner, resuming creates a new server on the retained workspace volume.
- `cloud_companion_stop` stops the companion's worker and keeps its workspace
  storage and worktrees. It stays stopped until an explicit Ensure Ready:
  checking its box or restarting Horizon does not start it.
- `cloud_companion_operation` reads an operation's phase from the original
  request's `cloud` and `alias` and its `operation_id`, without changing
  anything: polling never starts or continues an operation.

Ensure Ready and Stop answer at once with an `operation_id` and a phase; poll
`cloud_companion_operation` with the same `cloud` and `alias` until `done` is
true. When `resend` is true, nothing is running the operation, for example after
its card was busy or Horizon restarted mid-operation: send the same Ensure Ready
or Stop again to continue it, which reconciles and never repeats a provider
change. A caller may pass its own UUID
as `operation_id`, so a lost answer is polled instead of sent again. Repeated or
concurrent requests for the same companion share one operation and never start
a second worker. The operation runs on the target cloud's card with its progress
and log, as the card's own Resume or Stop would; a card busy with another
operation leaves the request recorded, and the next request for it continues
it. Ready means the source reached the target over SSH, not only that the
provider reports the worker running.

A deleted, deleting, lost or changed companion is refused, and a companion
whose provider outcome is uncertain is reconciled on the next request, never
repeated. Agents cannot create a companion that has no cloud yet: the owner creates it
with **New cloud** and checks it on the source cloud's card first. A checked
cloud whose first worker was never started answers `confirmation_required`;
start it from its own card. Starting a companion never starts the companions it
declares itself. The first request for a checked companion binds it to that
cloud and its checkout; choosing another cloud for the same alias later needs
the binding cleared first.

These requests need the running Horizon that owns the source cloud, in a saved
session; without it they fail with `cloud_companion_timed_out` or
`cloud_companion_unavailable`, and nothing starts. An Ensure Ready or Stop that
Horizon reaches only after the caller stopped waiting is refused with
`cloud_companion_expired`, before anything is recorded. Existing SSH connections
between workers keep working without Horizon. The CLI reaches the same tools
through a plan, for example:

```bash
horizon-browser run - <<'PLAN'
{"version":1,"steps":[{"id":"start","tool":"cloud_companion_ensure_ready",
  "arguments":{"cloud":"<source cloud ID>","alias":"consumer"}}]}
PLAN
```

When closing, Horizon waits for earlier companion jobs and durably saves queued
unchecks locally. This save does not require provider settings or reachable
workers; remote cleanup resumes when the session is opened again. If the local
journal cannot be saved, shutdown displays the error and retries instead of
discarding the uncheck. The fallback exit path also waits for that save, so a
persistent disk or journal-lock failure can keep the process alive until repaired.
