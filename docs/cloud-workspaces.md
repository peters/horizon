# Cloud workspaces

A cloud is one remote development container. New shell and agent panels share its
Git checkout; branches and additional worktrees are created manually. Browser and
Device panels share that cloud's runtime.
RunPod and Hetzner provision workers. An existing Docker host can also run them;
see [Docker host clouds](cloud-docker-host.md). Daytona and Fly.io appear in
labelled design fixtures.

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
Use a preauthorized, non-ephemeral auth key with only `tag:horizon-worker`.
Use a reusable key to enroll more than one cloud. The tailnet policy must define
`tag:horizon-worker` in `tagOwners`. The auth key must permit this tag.
The tailnet administrator sets which services this tag can reach.
Horizon does not change the tailnet policy.

The worker requests `--advertise-tags=tag:horizon-worker` at enrollment.
After `tailscale up`, it reads `Self.Tags` from `tailscale status --json`.
It checks the saved node once at each resume too. The node must have exactly
`tag:horizon-worker`. An untagged node or a node with extra tags is refused and
logged out. An unreadable status also fails the check. The error names the tag
and the `tagOwners` prerequisite.

These checks apply at enrollment and resume only. Horizon does not monitor tags
continuously. The tailnet policy controls reachability, also after a policy or
tag change. This contract does not provide a separate tag for each workspace.
See [the enrollment trust boundary](architecture/tailnet-enrollment.md).

**None** is the default in **New cloud**. Choose a saved network before provisioning. A cloud
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
refused. A selected tailnet requires `horizon-tailnet-contract=4` from the worker
image before allocation. Reconnect and new sessions require this marker from the
actual worker too. Markers 1, 2 and 3 do not qualify. **None** remains compatible
with older images.

### Release compatibility note: tagged enrollment

Existing keys that cannot assign `tag:horizon-worker` fail enrollment. An already-joined,
untagged cloud logs out when it resumes with the new helper. Before the upgrade,
ask the tailnet administrator to define `tag:horizon-worker` in `tagOwners` and
supply a key with only that tag. Rebuild custom images with contract marker 4.
The quick-start pin contains marker 4, so a new cloud can select a tailnet on
compatible storage. Existing clouds retain their recorded image until a rebuild.
The worker requires root-owned private state and a mount root that agents cannot
write. The tested RunPod persistent storage reports directories as `0777` and
files as `0666` even after successful mode changes; this runtime is unsupported
for isolated worker startup. Choosing **None** does not bypass these checks.
See the [rollout procedure](testing/procedures/tailnet-marker4-rollout.md).

Each cloud has one device name in its tailnet for the life of the cloud. The
worker makes the name from the cloud ID. The name stays the same after a stop
and a resume. The name has one of two forms:

- A cloud ID of lowercase letters, digits and inner hyphens usually gives the
  direct form, `horizon-cloud-<cloud ID>`. A UUID cloud ID, which Horizon makes
  for each new cloud, always has this form.
- Other cloud IDs give a digest form. The rules for this form are below.

Other clouds and agents use the full name `<device name>.<tailnet>.ts.net`. To
find the device name of a worker, read the first entry in its device inventory.
The provider name of the server or pod can be different from the device name.

Tailscale names a device after its host name, and each new container has a
random host name. Thus, the worker gives the device name to Tailscale:

- When the container starts, the worker reads the cloud ID from
  `HORIZON_CLOUD_OPERATION`. It writes the name to a root-only file in
  `/run/horizon-tailnet`.
- The first enrollment gives the name to `tailscale up` with `--hostname`.
- The node state on the cloud volume keeps the name. A resume does not need
  the auth key again.
- If the device has a different host name, the next deploy or resume changes it
  with `tailscale set --hostname`. This applies to a cloud that enrolled with an
  older image, after a rebuild of its image. If this change fails, the cloud
  stays in the tailnet with its old name.

The digest form starts with `horizon-cloud-` and a short form of the cloud ID.
The worker writes the ID in lowercase and changes each underscore to a hyphen.
Then it adds a hyphen and 20 hexadecimal characters (80 bits) of the SHA-256 of
the exact ID. The full name has a maximum of 63 characters. These cloud IDs get
the digest form:

- An ID with capital letters or underscores.
- An ID with a last hyphen.
- An ID that makes a name of more than 63 characters.
- An ID that ends with a hyphen and 20 hexadecimal characters.

Thus, each name is a valid DNS label and is unique to its cloud. Tailscale does
not change a name that a tailnet administrator set. If a different device has
the name, Tailscale adds a suffix such as `-1`. An image with this behavior
reports `horizon-tailnet-contract=4`. Marker 2 also declares stable names on
older images, but those images do not qualify for selected-tailnet operations.
An image without stable-name support keeps the container host name, which changes
at each resume.

All members of the tailnet can see the device name. A name in the direct form
shows the full cloud ID. A name in the digest form shows a maximum of 28
characters of the ID, in lowercase and with hyphens for underscores. Then it
shows the digest. Thus, it does not always show the exact cloud ID.

Agents discover ACL-visible devices in
`/run/horizon-tailnet-devices/devices.json` (names, addresses and online state
only). HTTP/HTTPS and SOCKS proxy environment variables provide outbound access;
TCP tools without proxy support need an explicit SOCKS proxy. This is not a
kernel VPN interface and does not add inbound services or route access beyond
Tailscale policy. The daemon continues independently of the controller, with
state on the cloud's persistent volume. The newer signed project-session runtime
currently refuses a tailnet until its own unprivileged runtime is qualified;
the ordinary cloud panel's shell/agent runtime uses the isolated launcher.

## Tailnet API credential status

Settings > Tailnets displays **Not stored**, **Personal API token stored**,
**OAuth client stored**, **Reading…** or **Status unavailable** for each saved
network. All reads run on background workers. API entry and replacement use the
masked native controls below; the enrollment auth-key editor remains separate.

Set API credential and Replace API credential start an empty masked draft.
Choose Personal API token or OAuth client, enter the values and select Save API
credential. Changing the source clears all fields. Discard credential draft and
Settings close discard unsaved values. Current stored values are never loaded.

An agent requests set_personal_token or set_oauth with `tailnet_api_credential`,
the exact saved ID and its request UUID. It sends no values. The person chooses
Enter requested API credential on that row and explicitly saves the requested
kind. Requests do not open Settings or grant write authority. CLI plans and MCP
use the same native-owner route; no secret getter/import or node cleanup is added.

Stored presence reports kind only, not expiry, scopes or remote API access.
API credentials stay in this computer's OS-store service `horizon-cloud-tailnet-api`;
enrollment auth keys use `horizon-cloud-tailnets`. An existing staged entry at
its exact saved local tailnet ID is recognized without another paste.
The same saved ID across profiles intentionally shares the API binding.
A profile name does not isolate credentials.

Read `cloud_companions.tailnet_api_credentials` for cached observations, even
with Settings closed. While Settings is open, its pending and completed status
also reaches this cache before MCP requests; close is not required for an update.
Ready contains `credential.state: missing` or `stored`;
stored entries include `credential.kind: personal_token` or `oauth`.
Pending means a read is unfinished; unavailable means it could not be read.
Neither means missing. Before a successful catalog read, `tailnets` can be null
and the map empty; this is not proof that credentials are missing.

The CLI is `cloud_deploy tailnets STATE_ROOT [TAILNET_ID]`:

```sh
cargo run -p horizon-core --example cloud_deploy -- tailnets STATE_ROOT
```

It returns saved ID/name items and the same map, with ready/unavailable status.
Neither status interface imports a secret or requests node cleanup. Status can
finish a prior recorded local binding recovery and retire exact obsolete slots;
it submits no new credential mutation. Saving credentials through the shared
host store does not validate remote access or remove nodes.
Completed saved-tailnet deletion prunes only that removed ID's disabled API record.
A failed cleanup keeps its recorded intent for retry; it does not authorize another ID.
Worker image qualification still requires marker 4; API status does not update the pin.
See [the boundary](architecture/tailnet-api-credentials.md),
[the storage procedure](testing/procedures/tailnet-api-credential-store.md)
and [the status procedure](testing/procedures/tailnet-api-status.md).

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

The toolbar has two buttons that make a workspace:

- **+ New cloud** makes a workspace and opens **New cloud** for it. Its tooltip
  shows the machine of a quick start and its hourly price, for example
  `from €0.0088/h · 2 vCPU · 4 GB · Hetzner`: the cheapest offer of the providers
  that this computer can deploy to. A repository with its own profile can ask for a
  larger machine. **New cloud** then shows that machine and its price. In a narrow
  window, the button shows **+ Cloud**.
- **+ Workspace** makes an empty workspace on this computer. No dialog opens.
  **New** in the sidebar does the same.

The empty canvas shows **New Cloud** and **New Workspace**, which do the same.
**Menu › Add cloud here** starts a cloud in the current workspace.

The preset picker that opens when you double-click an empty part of the canvas
also offers **Cloud GPU**. It does what **Cloud** does and selects the first
profile with `gpu: true`. When the repository has no GPU profile, the dialog says
so and keeps the default profile on a CPU worker. The presets under **This PC** in
that picker make a workspace on this computer with a panel.

When you cancel **New cloud** for a workspace that **+ New cloud** made, the
empty workspace goes too, unless it is the only workspace on the board. When cloud
workspaces are not ready on this computer, **+ New cloud** is dimmed.

A repository can ask for This PC with `placement: local` at the top level of
`.horizon/cloud.yml` (the default is `placement: cloud`). **New cloud** then shows
**This repository runs on This PC** with **Open on This PC**, which opens a
terminal in the repository in that workspace and closes the dialog. You can still
start a cloud for it.

In **New cloud** from **+ New cloud**, select **Keep my choice for this
repository** to keep where the repository runs. **Start cloud** keeps **Cloud**,
and **Open on This PC** (or **Open on This PC instead**) keeps **This PC**. The
next time, **New cloud** says **You keep This PC for this repository.** or
**You keep Cloud for this repository.** A kept choice wins over the `placement` of
the repository: a kept **This PC** offers **Open on This PC**, and a kept **Cloud**
does not. Click **Forget** to remove the choice. Horizon keeps the choices on this
computer only, in `~/.horizon/cloud/repository-choices.json`.

In an existing workspace, choose **Cloud** from the panel-creation menu (or
**Menu > Add cloud here** in the toolbar), enter a title, and press Enter. Horizon discovers the Git
root from the workspace directory, loads `.horizon/cloud.yml`, and uses its named
default profile. Preparation runs while you type. A configured launch starts
provisioning immediately after submission, without a separate Deploy action.
**More options** contains the container disk, repository and committed revision.
Only committed source is transferred; local changes stay on this computer.

A pasted repository link is cloned on this computer first. Unless you choose
another folder, the clone goes to `<folder>/<owner>/<repository>`, for example
`~/github/acme/web`. `<folder>` is the first of `~/github`, `~/code`, `~/src`,
`~/projects` and `~/dev` that exists, else `~/Horizon`. A GitLab link keeps all
its groups, as in `~/github/group/subgroup/app`. When a folder on that way is
already a checkout, the clone goes to `<folder>/<repository>` instead, so it never
lands inside another repository. A checkout of the same link that
is already there is used as it is, also one from an earlier Horizon at
`<folder>/<repository>`.

While it clones, the dialog shows the step and Git's phase with its progress. For
receiving and resolving it also shows the phase's time left, counted down live;
the phases that run on the host jump too much for one. A second line shows what
the whole clone has received, out of about the repository's size for a GitHub
link, and how long it has run. No time left is shown for the whole clone: on
`git/git`, receiving took 13 of 64 seconds, and the rest follows object counts
that are known only at the end.

Missing account settings open a repair form without losing the title or target
workspace. Enter the compute API key and choose how the profile's agents sign in:
an API key, **ChatGPT sign-in** for Codex or subscription login for Claude.
**Save and start** continues the submitted launch. Opening registry setup from
New cloud uses **Save and return** instead: the form remains open even when compute
and SSH are ready. Saving or cancelling returns to New cloud for a fresh publishing
review without submitting a launch. A dedicated
SSH identity is created when needed and keys stay in private machine-local files.
Blank replacement fields preserve saved keys. Subscription login for Claude happens
through the actual agent on the worker; worker readiness does not prove authentication.
See [Sign in with ChatGPT for Codex](#sign-in-with-chatgpt-for-codex).
Open **Menu > Cloud settings** in the toolbar to change machine defaults without
launching. **Menu > Fit all clouds** shows every cloud on the canvas.

Cloud allocation requires a saved session so worker identity survives reconnect.
An isolated test instance can use its own disposable saved session and private home.

[Quick start](#quick-start) needs only Git and OpenSSH on this computer. For other
clouds, install Docker with BuildKit/buildx too.

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
Agent login state stays on the worker, except the ChatGPT login that Horizon gives
to Codex. Workspaces and profile names do not create separate credential sets.
Reconnect removes unbound API-key files, including interrupted-upload staging
files, while retaining ChatGPT and subscription login state. Existing agent
processes can retain authentication already loaded into memory or their
environment; start a new session to apply the changed authentication choice.

Optional repository Git authentication uses explicit `git_credentials` bindings.
With [Connect GitHub](#connect-github), a binding is a fallback for the
repositories that the app does not reach. See the [worker credential setup](../examples/cloud-worker/README.md#optional-git-credentials)
for private file permissions, repository matching, removal and token-scope limits.

### Sign in with ChatGPT for Codex

**ChatGPT sign-in** replaces the subscription login of Codex. In **Cloud settings**,
choose **ChatGPT sign-in** for Codex and click **Sign in with ChatGPT** once. Horizon
runs `codex login` from the Codex CLI on this computer, which opens the ChatGPT sign-in
page in the browser. The login goes to a private Codex home,
`~/.horizon/cloud/credentials/codex`, with file storage. Your own Codex login in
`~/.codex` is not used or changed. The Codex CLI must be installed on this computer.

Every deployment, reconnect, resume and rebuild of a cloud that runs Codex, without a
bound OpenAI API key, then gives this login to the worker as
`/workspace/home/.codex/auth.json`, mode 0600. The agent user owns it on a worker that
isolates agents, and `root` owns it on a worker that does not. Codex on the worker
starts signed in. The login goes over SSH standard input, never in a command line or a
log.

ChatGPT refresh tokens are single use, so one owner renews the login: this computer.
While Horizon runs, it checks the login every five minutes. It renews the login when
less than two days, or less than half the life of the access token, is left, with the
same request that Codex uses, and gives the renewal to every cloud with a record under
`~/.horizon/cloud`, also clouds of other sessions. Codex on a worker renews its own login
only five minutes before the access token ends, and it first reads its login file again
and takes a newer login that it finds there, so the workers use the login that this
computer renewed. Each ready cloud is also asked every half hour, and when the login here
changes. A sync waits at most 15 seconds for one worker and asks no more clouds after two
minutes per pass; the rest, with the clouds that failed last time asked last, wait for the
next sync.

This needs Codex 0.162 or newer on the worker, as the worker images ship it: such a Codex
reads its login file again before it renews. An older Codex keeps the login it loaded
until it restarts, and its first renewal after Horizon renewed fails. Rebuild the image of
such a cloud.

A worker that renewed its login while Horizon was closed keeps that renewal, and the
renewal comes back to this computer and replaces the spent copy, so the next cloud gets
it. Only a later renewal of the same account wins; a new sign-in here, also of another
account, replaces the worker's login. When the login is due, Horizon first asks every
cloud for a renewal it made on its own, also a cloud it asked recently. If OpenAI still
refuses the refresh token, for example because two clouds renewed the same login at the
same moment while Horizon was closed, Horizon stops sending it and **Cloud settings** asks
you to sign in again. The login stays on this computer until then, so a later renewal
that a cloud returns makes it usable again. Clouds get a new login when they next
connect or within five minutes. Do not run `codex logout` or `codex login` on a worker: both revoke the
login that every cloud shares. **Sign out** in **Cloud settings** removes the login from
this computer and revokes it when OpenAI can be reached; clouds keep working until their
current access ends.

Without a sign-in, Codex on a new cloud asks you to sign in in its own terminal, as
before. A bound OpenAI API key takes precedence over the ChatGPT login.

### Claude with a setup token

A setup token signs Claude in on your Claude subscription for one year. Claude on the
worker then needs no sign-in of its own. To make one, run `claude setup-token` on a
computer where you use Claude Code.

Horizon keeps the token like the agent API keys: in a private file (0600), for all
workspaces on this computer. `settings.json` keeps only the absolute path of the file
and the time that the token was saved, in seconds since 1970:

```json
{
  "claude_token": { "file": "/private/claude-token", "saved_at": 1792065600 }
}
```

Horizon counts the expiry as one year after `saved_at`. Horizon cannot read the real
expiry from the token. A token that was made before it was saved runs out earlier.

Every deployment, reconnect, resume and rebuild of a cloud that runs Claude gives the
token to the worker as `/workspace/credentials/claude-oauth-token`. The token goes over
SSH standard input, never in a command line, a log or the cloud board. Each Claude panel
and each shell on the worker gets the token as `CLAUDE_CODE_OAUTH_TOKEN`. This needs a
worker image with this change; an older image ignores the file.

Before **Resume worker** starts a stopped worker, Horizon checks the setup-token file and
the API-key files of the agents of the cloud, as a deployment does. A missing or unusable
file stops the resume before the provider starts paid capacity, and the refusal names
the file.

Settings never keep a setup token and a Claude API key together: Claude prefers the API
key. When you choose another sign-in, the next reconnect removes the token file from the
worker. Horizon does not revoke the token.

Trust boundary: the token gives access to your Claude subscription. Each process on the
worker that runs as the agent user can read it, as it can read an API key. Horizon does
not check that the token is valid before it sends it.

### Connect GitHub

**Cloud settings › GitHub › Connect GitHub** gives clouds GitHub access as you,
with no token to create or copy.

1. Horizon opens GitHub in your browser with a GitHub App already described.
   You click **Create GitHub App**. Horizon keeps the app's client
   secret in `credentials/github-app-<app>` (0600) and never keeps its private key.
   The app is named **Horizon for <login>** when Horizon already knows your GitHub
   login from an earlier sign-in on this computer and no public app has that name;
   otherwise it is **Horizon** and a short random suffix. Horizon cannot see a
   private app, so a private app with the same name is not detected. Then GitHub's
   form says that the name is taken: change the name there and click **Create GitHub
   App** again. GitHub's form also lets you change the name in other cases.
2. GitHub shows the installation page. You choose the repositories that clouds
   may reach. **Choose repositories** on the card opens this page again. To give
   clouds the repositories of an organization, install the app on that
   organization too. The app is public so that an organization can install it;
   it reaches only the repositories where it is installed. An app that an older
   Horizon created is private: select **Make public** in its **Advanced**
   settings first.

   The app asks for these repository permissions, and a token acts with them only
   in the repositories where the app is installed:

   | Permission | Access | Why |
   |---|---|---|
   | Contents | Read and write | Clone, commit and push |
   | Pull requests | Read and write | Open, review and merge pull requests |
   | Checks, Commit statuses | Read | Read why CI failed on a pull request |
   | Actions | Read and write | Read job logs and rerun a failed job; this also allows cancelling a run |
   | Workflows | Write | Push a commit that changes `.github/workflows`, as Dependabot updates to GitHub Actions do |
   | Metadata | Read | Required by GitHub for every app |

   With Actions and Workflows, an agent that holds the cloud's token can change
   what CI runs on a branch and rerun it. Branch protection and required reviews
   still apply to merges. An app that an older Horizon created has only Contents,
   Pull requests and Metadata: add the others under **Permissions & events** in its
   settings, then accept the new permissions on each installation.
3. GitHub does not let a new app turn on its device sign-in. When the card says
   so, open the app's settings, select **Enable Device Flow** and save. This is
   needed once, for the default mode.

The card then chooses how a new cloud gets its access:

| Mode | New cloud | The worker renews with |
|---|---|---|
| **Ask me for each new cloud** (default) | One **Authorize** click on GitHub, with a code that the card shows | The app's client ID only |
| **Automatic** | No click after the first approval: the first sign-in for the app shows **Authorize** in your browser once. Later, your browser, already signed in to GitHub, returns to Horizon by itself | The client ID and a copy of the client secret, readable only by the worker's root service |

When a deployment reaches the worker, Horizon asks the worker's GitHub service
whether it already holds access. A worker that does keeps it, so a reconnect
never asks you again. Otherwise the card shows the sign-in for that cloud. The
cloud gets access to its repository and its same-worker siblings, where the app
is installed. **Skip** continues without GitHub access, and so does a declined or
expired sign-in, or a worker image without the service. When cloud settings also
have a `git_credentials` binding for the repository, Git keeps using that binding
in those cases, and the card says so. The binding is a fallback: while the app's
access reaches every repository of the cloud, Horizon removes that binding from
the worker; otherwise the binding stays for the repositories the app does not
reach. The app's access comes first for a repository that both reach. A cloud
whose checkout no longer has a GitHub origin loses the access its worker held.

The worker's root service renews the access every 8 hours for about 6 months,
also while this computer is off. Git reaches GitHub through the worker's Git
proxy, which adds the access only for the repositories of the cloud, so Git never
gets a token. The proxy also adds the token of a `git_credentials` binding for its
repository. `gh` gets a short-lived access token for a repository of the cloud.
Agents never see the refresh token. Commits use your name and your
GitHub private commit address. The steps card shows **GitHub: signed in as
<login>**.

An agent that needs another repository asks with the worker's `github_access`
tool. The request shows at the top right of the cloud with **Allow for this
cloud** and **Deny**. The worker checks that the app reaches the repository
before it allows anything.

Access is per cloud. Every agent session of a cloud can use each repository the
cloud has access to. The Git proxy limits Git only. `gh` still gets the full
access token, and an agent can use that token outside `gh` for every repository
that the app reaches. Put work that must not reach a repository in a cloud without
access to it, and install the app only on the repositories that agents may use.

**Disconnect** stops new clouds from getting access. It does not end the access
of running clouds: delete the app on GitHub for that. Deleting the app ends every
token at once.

### Your repositories in New cloud

**Where is your code?** in **New cloud** is one search field:

- Type words to search the Git checkouts on this computer and, with GitHub
  connected, the repositories that your GitHub App is installed on. The letters
  must only come in order: `acwb` finds `acme/web-board`. With nothing typed,
  the checkouts that changed last come first.
- A repository that is on GitHub and on this computer shows one time, with both
  badges. If you choose it, Horizon uses the checkout.
- Type a path that starts with `/`, `~`, `./` or `../` to see its folders. **Tab** opens
  the highlighted folder, and **Enter** uses a repository.
- Paste a link to clone a repository. Choosing a GitHub repository that is not
  on this computer puts its link in the field. Continue clones it on this
  computer as before.

The arrow keys move through the results. Horizon looks for checkouts up to four
folders below your home folder, on a background thread. It does not look in
hidden folders, or inside build folders such as `vendor` or `node_modules`; a
build folder that is itself a checkout is listed. On macOS it does not look in
the folders that macOS guards with a privacy prompt, such as Documents. Before
this computer signs in to your GitHub App, **Show your GitHub repositories**
signs it in and lists them. What the search reads and its limits are in
[the checkout search boundary](architecture/checkout-search.md).

For a private repository on github.com, the token card becomes **Clone
with GitHub**, which clones with your connected account; **Use a token instead**
keeps the old way.

While this computer is signed in, Horizon keeps the last list privately beside
the sign-in in its credentials folder. The next **New cloud** shows it
immediately, without a click, and Horizon gets a new list in the background to
replace it. If that refresh fails, the list stays with a note that it may be out
of date and a **Refresh from GitHub** button, and Horizon tries again every 30
seconds. Each try first checks which account this computer is signed in as, so a
list of another account goes also without the network. While a list shows, Horizon
also checks that account every 30 seconds, without the network: a sign-in as
another account or a Disconnect in another window removes the list. **Disconnect** forgets the list
together with the sign-in.

For this, this computer signs in once to your GitHub App, the way the app's
setting says: one Authorize click, or none in Automatic. Horizon keeps that
sign-in private in its credentials folder, renews it before each use for about
six months, and never writes it into a checkout. **Disconnect** forgets it. A
repository the app does not reach shows **Add it on GitHub**, which opens the
app's repository choice.

### Choose where to publish images

A profile with `build` can omit `image`. New cloud shows **Publish images** with
the selected destination and publishing account before you start. **Automatic**
uses your image-publishing account, independently of the source repository's
author or Git host. If no authorized destination is available, choose an account
or set up the exact registry before building.

Use **Change** to select an existing authorized registry, use the profile default
or explicitly sign in to another publishing account. Horizon remembers the choice
on this computer for the canonical checkout and named profile. It keeps the exact
saved grant reference; changing accounts cannot substitute another grant. The
repository YAML stays unchanged. Opening worker pull setup returns to New cloud
for a fresh review after **Save and return** or Cancel; it does not start a worker.

For example, a build profile can contain:

```yaml
version: 1
default: dev
profiles:
  dev:
    provider: runpod
    min_cpu: 2
    min_memory_gb: 4
    build:
      context: .horizon
      dockerfile: .horizon/Dockerfile
```

A prebuilt profile without `build` still requires `image`. An explicit image is
supported too. Reconnect, retry and rebuild keep an existing cloud's recorded
destination and publishing identity, even if the current account or local
preference changes. An older cloud without publisher metadata keeps its explicit
image; Horizon does not invent a former account binding.

Local agent panels use `cloud_image_destination` through normal CLI plans or
stdio MCP without a Settings window. List the named profile, retain its exact
`shown_override`, and use the listed choice for select/reset. The host supplies
the checkout and settings scope; callers cannot supply credentials or another
source path. See [public destination operations](architecture/image-publishing.md#public-destination-operations).
Git hosting and image hosting are independent. This adds no GitLab account
connection or project discovery.

### Publishing images to ghcr.io

A cloud whose image Horizon builds pushes it to its reviewed destination, from
the profile or the machine-local choice. GitHub's package registry does not accept tokens of the GitHub App behind
Connect GitHub, so the first time an image goes to `ghcr.io`, the card shows
**Allow Horizon to publish images for you** with a code. Open GitHub, paste the
code and click **Authorize** once. The permission covers images only
(`write:packages`), not your repositories, and goes to Horizon's own GitHub
OAuth app.

Horizon keeps that sign-in on this computer, readable only by you, beside its own
Docker configuration, and logs that configuration in to `ghcr.io` before each
push. The access renews itself for about six months. If you revoke it on GitHub,
the next push asks again. **Skip: do not publish** stops the deployment at
**Push image**. An image repository bound in **Cloud settings › Container
registry** publishes with its own credential instead.

### A refused image push

If the registry refuses the push or the pull of the image of the cloud, a retry
gets the same refusal. Thus the failure on the card offers **Open Container
registry** instead of a retry. The header of the card keeps the retry of the
operation that failed for later: **Retry deploy** after a deployment,
**Reconnect** after a reconnect, or **Resume worker** after a resume. A refusal
during **Build locally** is about a base image that the recipe pulls, so that
failure keeps its retry. An image without a registry host, such as
`owner/worker`, uses the login in the private Docker configuration directory,
because **Container registry** cannot bind it. That failure also keeps its retry.

**Open Container registry** opens **Cloud settings** for the image repository that
the profile of the cloud names. If a binding covers that repository, the card
scrolls to it: replace the credential that the registry refused. After a refused
pull, that binding shows **Needs validation**, also if an earlier validation
passed. If **Container registry** has no binding for that repository, the card
shows the repository with
**Not set up**, who publishes to it, and that workers have no pull credential yet.
Click **Add credentials**: the entry for the repository opens with its name filled
in and **Expiry and publishing** open. Add a publishing credential and a read-only
pull credential, save the settings, then click the retry in the header of the
cloud card.
To add a different repository, click **Add another**. **Cloud settings** in the
toolbar Menu does not fill in a repository.

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

The worker's stop service, which stops a worker at an agent's request, keeps its
separate GraphQL stop call. It uses the provider-injected Pod-scoped credential,
which the live qualification for #966 found was refused by REST and accepted by
GraphQL. This migration changes the shared account-credential adapter; it does not
put account credentials inside workers or remove that independently qualified stop
path.

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

If the selected commit has no `.horizon/cloud.yml`, and the working copy has no
`.horizon/cloud.yml` either, **New cloud** shows **No cloud setup
yet**. A working copy that is not valid shows its error instead. This is not an error. The dialog then shows two choices:

- **Quick start** starts the repository on the base image. See
  [Quick start](#quick-start).
- **Set up from CI** starts a setup agent, Claude or Codex, that prepares the
  settings of the repository. It runs on Linux with a systemd user session:
  Horizon starts the agent in a systemd user scope, which holds every process the
  agent starts as its descendant, so it can end them all before it commits. The
  scope is not a sandbox: the agent runs as you, and a program it hands to another
  service of yours, such as Docker or your systemd user manager, runs outside it,
  as any program you start does. What **Use this setup** commits does not rest on
  the scope: it is the setup Horizon proved, read after the agent ended, with every
  other change named first. On macOS and Windows the dialog does not offer it, and
  on Linux without a systemd user session the agent does not start; use
  **Quick start** or write `.horizon` yourself.

The setup agent is a normal local agent session with its existing local login.
Horizon does not export the API-key bindings of remote workers to it. Its panel is
hidden: the dialog becomes a preparation view, **Set up REPOSITORY**, because
nothing is rented yet. The left column shows the steps (Read CI, Find tools,
Write Dockerfile, Build image, Run CI in image, Fast rerun, Desktop preview), each
with a short detail and its elapsed time. The right column is the agent's own
terminal without panel chrome. Click it to type to the agent, for example to
answer a question or to change its plan. A question also shows above the steps.
**Stop** closes the agent. **Close** keeps it working, and **New cloud** shows it
again for the same repository.

Several repositories can be set up at the same time, each with its own agent. Click
**Close** while a setup works, open **New cloud** again, choose another repository
and start its setup. **New cloud** shows the setup of the repository that it opens
for, so open it for the first repository to see that setup again.

The **Setup board** panel shows every setup in one place. It opens with the first setup,
beside the setup agents in a local workspace (the active one, else a workspace named
**Cloud setup**), and **Close** in the setup view shows it.
**Menu** > **Setup board** opens it too. Each row shows the repository, a segment for
each step, and what the setup does now: the running step, **Waiting for a build
slot** with the setups that build, a failed step, or **Ready** with the agent's
summary. **Open agent** shows the agent's panel. **Open in New cloud** (**Use in New
cloud** once the setup is ready) opens **New cloud** for that repository, with its
steps, the agent's terminal and **Use this setup**. **Stop** stops the setup.
**Set up a repository** opens **New cloud** to start another one. The rows are made
again from the setups that run, so after a restart the board is empty until a new
setup starts. One
repository has one setup at a time: a second setup of the same folder, also through
a link to it, is refused until the first one stops. A message about a setup, for
example why **Use this setup** did not commit, shows under the steps of that setup.

The image build, the CI in the image and the fast rerun are the heavy part of a setup.
Horizon runs them for a limited number of setups at the same time on one Docker: one
for each 16 processors of the computer, at least one and at most four. A setup that
waits for a free build slot shows **Waiting for a build slot** and the repositories
that build at that time on its **Build image** step. Slots go in the order that the
setups asked for them. To change the number, start Horizon with
`HORIZON_SETUP_BUILD_SLOTS` set to a number from 1 to 16.

**Build on** chooses where the proof of the next setup runs. It shows under the agent
chips when Cloud settings has a Docker host on another machine and Docker hosts are
on. **This computer** uses this computer's Docker. A Docker host uses the Docker of that
machine: Horizon opens an SSH session with your own SSH configuration and keys, which
forwards the host's `/var/run/docker.sock` to a socket in a private folder of this
computer, and every docker command of the proof and of its cleanup goes through it.
The agent still runs on this computer and reads the repository here. The proof copies
the bundle, the restored packages and the Rust git clones to the host, as it copies
them into a container on this computer. A live preview is brought back to this
computer through a second SSH forward of its port. A Docker host has build slots of its
own. A setup with a GPU profile, and a desktop preview that needs `/dev/kvm`, run on
this computer only. The setup board names the host of each setup that builds on one.

The agent reads the CI of the repository first, the GitHub Actions workflows in
`.github/workflows`: the Linux jobs that restore, build and test, the tool versions
they use and where those versions come from. Horizon proves GitHub Actions jobs
only, so a repository without a GitHub Actions workflow that builds or tests on
Linux cannot finish the setup yet; the agent says so. It uses
[localci](https://github.com/peters/localci) when it is installed: `localci plan`
resolves the jobs, and each `uses:` step that localci skips is a tool that the
image must supply. Then it reads `AGENTS.md`, the README and existing Dockerfiles.
It writes `.horizon/Dockerfile` on the public worker base of
[Quick start](#quick-start) and `.horizon/cloud.yml`. The image also gets a pinned
localci release, so agents on the worker run the CI jobs with
`localci run --fast`. The agent does not build the image or run the CI itself:
it names the CI jobs in its status file, and Horizon proves the image on this
computer with Docker. Horizon builds it, runs the worker check, runs the jobs
in a container of the image as the worker account with the localci that a worker
session finds first on its `PATH`, as the agents on the worker do, then once more
with `--fast` to show the warm rerun, and tells the agent the result. A wrapper
there that starts what the CI needs, such as a database, and then runs localci
is allowed. A job that runs no command here, because its `if:` is false on this
computer, such as a publish job, or because the proof skips each step of it that
runs one, is left out of the proof and named in its output. The proof shows that
the setup works, not that the image is honest: every program the CI runs comes
from the image, its shell included. The
dialog shows these steps as proved by Horizon, with their times. **Use this
setup** is available only after the proof that the agent asked for passed and
only while the `.horizon` files and the commit are the ones it tested. A build
context outside `.horizon` is built from the committed tree with the proposed
`.horizon` files over it, which is the image that deployment builds, and a build
path that leaves the repository is refused. Each profile's distinct build is
built and checked, and the CI runs in the default profile's image; a default
profile that runs a published image is pulled and checked instead of built. Each
image the setup builds must have localci on a worker session's `PATH`, with mode
755; a published image runs as it is published, so its workers have localci only
when the image does, and its proof runs Horizon's own. The CI runs
with the session environment a worker gives, not the image's `ENV`. A
setup with a GPU profile is proved only on a computer with an NVIDIA GPU, and the
CI uses that GPU when the default profile asks for one. The proof
gets the tested commit and its history as a bundle, never this computer's git
folder, checked out on its branch with the tags in its history, as
`actions/checkout` leaves a push build, and each setup run has its own image and volume, removed when it ends. The proof
mounts no folder of this computer into its containers. It copies what a container
needs into it before the container starts: the bundle, the committed tree with its
submodules and Git LFS files, Horizon's localci, the restored .NET packages and the
cloned Rust git dependencies. The copies are owned by `root`, so the CI's user can
read but not change Horizon's localci and the clones. Links in the copied folders
stay links and are never followed. For a .NET repository
with private feeds, check **Restore private NuGet packages on this computer for
the proof**: Horizon then runs `dotnet restore` here, with this computer's feed
logins, copies the packages that restore used into the proof's volume for the
worker's account, and the container restores again from them, without any
credential; the CI reads only those packages. A restore here or in the container
that fails, fails the proof. It is off by default, because the restore runs the repository's
build files on this computer. For a Rust repository whose committed `Cargo.lock` pins git
dependencies on GitHub, the dialog names those repositories under **Fetch private
Rust git dependencies on this computer for the proof**. When you check it, Horizon
clones each named repository with plain `git clone --mirror` and this computer's
Git login into a private folder in its own home, and the container reads a copy of
each clone, owned by `root`, in place of its GitHub address. Horizon runs no Cargo, rustup or
repository configuration on this computer for this, and clones nothing that the box
does not name. The clones of one proof may hold at most 4 GiB on this computer; past
that the proof stops. It is off by default, because the proof's CI can read every
repository that the box names. When you clear the box after a proof that cloned,
the next proof starts on a new volume, so no copy stays in its caches. A
repository without GitHub Actions is proved with the commands its README,
`AGENTS.md` or build files give, run in order in the same container on the same
checkout and restore; its images need no localci. For a
desktop app the agent names the command that starts it, and after the proof
passes Horizon starts the app in a container of the image on the proof's volume,
with a virtual desktop on `:99` like the worker's. When a window appears, Horizon
takes a screenshot (when the image has ImageMagick's `import`) and opens the
desktop live in a Device panel beside the agent,
on this computer's loopback, before anything is rented. A new proof or a new
preview request replaces the live desktop, and **Stop** or **Use this setup**
removes it. The preview of a GPU profile gets this computer's GPU, and it gets
`/dev/kvm` when the app asks for it, such as an x86_64 Android emulator. With
rootless Docker, only the container's root account can open `/dev/kvm`, so that
preview runs as root, which is your own account. When this computer has no
`/dev/kvm`, or a container cannot open it (with rootless Docker the person's
account needs access to it), Horizon skips that preview, says why, and accepts the
setup without it. The agent asks before it uses a credential of yours, such as your
GitHub token for a private package feed. It does not allocate compute, push
images, commit or open a pull request.

The agent pins each toolchain version in `.horizon/Dockerfile` as an `ARG`
and writes `.horizon/pins.json`, which names the repository file that each
version comes from, for example `global.json` for the .NET SDK. Horizon writes
`.horizon/verify-pins.py` beside it, the same file in every repository, and
checks the pins at the start of each proof with its own copy of that file. The
agent does not change the CI: its summary gives the one step to add to the main
CI job after the checkout, `python3 -I .horizon/verify-pins.py`. With that step, a
later pull request that moves a toolchain, such as a new SDK in `global.json`,
fails in its own CI until the setup moves with it.

The agent also declares the repository's [companions](#companion-declarations).
It finds them in `AGENTS.md` and in the product map that `AGENTS.md` points to,
such as `../company/docs/products.json`, whose edges point from the consumer to
the provider and say whether the two sides must be checked together. The agent
declares no `same_worker` sibling, because the proof does not build sibling layers
yet: where `AGENTS.md` requires another repository's tests for this repository's
changes, such as a native library and the app that consumes its packages, the
summary names that repository instead. A runtime contract that must be checked
together, such as a REST API, a message queue or a shared key, gets a
separate-cloud companion in the repository that consumes it, and in the provider
only when its `AGENTS.md` says its changes are checked against that consumer. A package that a repository only uses is
not a companion: a library that the app uses does not declare the app. Each
repository stays usable on its own, because a declaration only offers the
companion in **New cloud**.

Some build failures are account problems, not Dockerfile problems. Horizon
pulls private `ghcr.io` bases with its own GitHub login. If an organization
limits OAuth apps, GitHub refuses that pull. Horizon then shows the cause,
**ORGANIZATION blocks Horizon's GitHub app**, and tells the agent not to change
files for it. **Let the agent do it** opens a **GitHub access** browser panel on
the page of Horizon's app, beside the agent, and the agent clicks **Grant** for
that organization. When GitHub asks you to sign in, the agent hands the browser
to you: sign in, then press **Done**. The agent never types a password or a
code. **Open GitHub myself** opens the same panel for you; **Prove again**
then runs the proof again. Only an owner of the organization can grant access.
For other members, GitHub shows **Request**, and an owner must approve it.

When the agent is done, **Use this setup** commits the files in `.horizon` and
reads them as the repository profile, so **Start cloud** can use them. A change
of the repository or YAML that is not valid clears the profiles that the dialog
read before. Thus you cannot deploy an old profile
by accident.

If the read fails for a different cause, for example a committed
`.horizon/cloud.yml` that is not valid, the dialog shows the error. The setup agent
is then in the section **No cloud configuration yet?**.

### Quick start

Quick start runs a repository without `.horizon/cloud.yml` on the base image
`ghcr.io/peters/horizon-worker-base`. Horizon uses the built-in image-only profile
`quick-start`:

| Setting | Value |
|---|---|
| Provider | RunPod |
| Minimum size | 2 vCPU and 4 GB memory |
| Container disk | 20 GB |
| Workspace volume | 20 GB |
| Agents | Claude and Codex |
| Browsers | Chromium |
| Desktop | Yes |

The base image is public. Thus the provider pulls it without a registry login,
and Horizon does not build or push an image. Horizon never attaches a saved
registry login to the base image, also if this computer has logins for the same
registry. The cloud card shows **Build locally**
and **Push image** as skipped. Horizon pins the base image by digest, thus each
Horizon version starts the image that was tested with it. The
[release flow](release-flow.md#update-the-quick-start-image) tells how a release
updates the pin.

Quick start needs a RunPod API key and the SSH identity of this computer. It does
not need Docker on this computer. Before Horizon allocates compute, it does not get
the digest from the registry. It also does not download the image or do the worker
contract check in local Docker. A local check of the pinned image cannot find a
different result, because:

- The pin is a digest. A digest identifies one image, and the contents of that image
  do not change.
- Before the **Worker images** workflow publishes the image, it does the worker
  contract check with the capabilities of the profile `quick-start`. It also does
  the marker check and the host key check. If a check fails, the workflow publishes
  nothing.
- Horizon keeps the result of the contract check beside the pin, in `CONTRACT` in
  `quick_start.rs`. Horizon examines this result against the cloud as it examines
  the result of a local check. Thus the same refusals apply, for example for a Git
  credential or a tailnet.

Horizon uses the kept result only for the pinned image with the capabilities of the
profile `quick-start`. For each other image or set of capabilities, Horizon does the
local check in Docker as before. This includes a committed `.horizon/cloud.yml` that
names the base image at another tag or digest, or with other capabilities. It also
includes the pin of an earlier Horizon version. Thus a cloud from an earlier version
needs Docker when it gets a new worker, for example after **Delete** and a new
deploy. To prevent this, rebuild the ready cloud on the current pin first. If Docker
is not on this computer, the check stops with an error that tells you so.

The kept result does not prove that the registry still has the image. If the
provider cannot pull the image, the readiness check times out.

The worker also does its own contract check when it starts. If that check fails, the
worker stops before it starts its SSH service. Then the cloud card shows **Worker
readiness timed out** after the readiness time of the profile, 15 minutes for quick
start. The log of the worker container in the RunPod console shows the cause:
`Worker validation failed`. This is the same for each image.

Quick start has these limits:

- Quick start is only for a commit without `.horizon/cloud.yml`. If the commit has
  this file, **New cloud** reads the committed settings instead and says so under
  **Profile**: `Quick start does not apply: this commit has its own
  .horizon/cloud.yml, so its profiles are used.` `cloud_deploy --quick-start` stops
  with an error. An uncommitted file does not count.
- **Rebuild image & restart** builds nothing for a quick start cloud. It restarts the
  worker on the base image that this Horizon version pins. See
  [Rebuilding a cloud's image](#rebuilding-a-clouds-image).
- A quick start cloud has no companions and no siblings.
- The profile names RunPod. If this computer also has a Hetzner token, **Provider**
  also offers Hetzner. Quick start is tested on RunPod only.

To go back to the settings of the repository, click **Use .horizon/cloud.yml**
below the quick start note. A different repository also ends quick start. The
`cloud_deploy` harness has the same choice, `--quick-start`. Cloud creation has no
MCP operation, thus quick start has no MCP operation either.

For the default launch path, commit `.horizon/cloud.yml` using the
[example](../crates/horizon-cloud/examples/cloud.yml).

### Settings from the working copy

**New cloud** reads `.horizon/cloud.yml` when it opens. While **Committed base
revision** is empty, it reads the file again when you save it, and when the
checked-out commit changes. You do not have to commit an edit of the settings or
the image recipe before you start a cloud. If a read fails for a cause outside the file,
for example a Git lock, click **Try again**.

When **Committed base revision** is the checked-out commit (`HEAD` is the
default), **New cloud** uses the working copy of the file, also when the file is
not tracked. The image of a profile with `build` builds from the working copy too,
as a rebuild does: see [Uncommitted files in the image](#uncommitted-files-in-the-image).
Only two fields come from the commit, because the worker gets a committed archive
of the source and a companion names its own commit:

| Field | Comes from |
|---|---|
| Each field of each profile, also `build`, `image` and `capabilities`; `default`, the set of profiles, `placement` | The working copy |
| `source`, `companions` | The commit |

These rules apply:

- A profile that is only in the working copy is in the list, also when it has
  `build`.
- For a profile with `build`, the dialog lists below **Profile** each uncommitted
  file that goes into the image, or the reason that Horizon refuses a file. A
  refused file stops **Start cloud**. After you remove or rename the refused
  file, click **Check again** below the reason. The dialog reads the list with the
  settings. **Start cloud** reads the files again; if they changed since,
  Horizon refuses the start and shows the new list.
- When the working copy changes `source` or `companions`, a note below
  **Profile** names the fields, for example `Uncommitted changes to companions are
  not used until you commit them.`
- When the working copy has no `.horizon/cloud.yml`, or the file is not valid,
  **New cloud** uses the committed file and says so. The image still builds from
  the working copy.
- When the committed file is not valid, **New cloud** shows the error, also if
  the working copy is valid. The worker gets its source settings from the
  commit, thus commit a valid file first.
- When **Committed base revision** is a different commit, **New cloud** uses the
  file of that commit, and the image builds from that commit only. It does not
  use the working copy.
- A new read keeps the profile, size, provider, place and storage that you chose
  while they are still valid. The dialog shows the previous values until the read
  is complete.
- **New cloud** does not change the settings while it starts a cloud, while it
  waits for stock, or while a setup agent works on the repository. It reads the
  file again when this is complete.

An image-only profile in the working copy lets you test an existing worker image
before you commit the settings. Set `image` to a tag or a digest that the
registry can supply. The image passes the worker contract check before Horizon
allocates compute. Horizon then pins the image by its registry digest.

The new cloud keeps the profile that it was started with in the machine-local
state of the cloud. A later edit of the file does not change that cloud or its
reconnect settings. **Rebuild image & restart** compares the bound profile with
the profile in the working copy of the file. The cloud takes the rebuild settings of
the working copy, and Horizon refuses a difference in another field. Thus a rebuild
keeps the uncommitted settings that you started the cloud with, while they stay in
the working copy. See [Rebuilding a cloud's image](#rebuilding-a-clouds-image).
Credentials always stay in machine-local bindings.

Image-only profiles omit `build`. Repository Dockerfiles build from the selected
committed tree, with the confirmed uncommitted files of the build context on the
checked-out commit, honor `.dockerignore`, and reuse local BuildKit layers. The
source that the worker gets excludes dirty and untracked files. Selected Git LFS objects and recursively pinned
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
If no data center in a region can hold the volume, the region chip also shows
**Storage unavailable** and cannot be chosen. It does not show **none in stock**,
because the cause is storage and not stock.
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
cloud has no resize corner. New clouds are placed relative to their workspace. When
you create a cloud, the view fits its workspace below the overview controls and above
the minimap. The view shows as little of the clouds of other workspaces as possible,
so their parked terminals stay parked. A new workspace is at the right of the others,
so the view puts it at the left edge of the canvas. A window that is wider than the
space between two workspaces can show a part of the next workspace. **Menu ›
Fit all clouds** shows the clouds of all workspaces.

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

### Close a cloud

The **×** on the header of a cloud opens a dialog. If the cloud has a worker or
storage at its provider, the primary action is **Delete cloud resources**.
Horizon deletes them in the background, and the card shows the steps. When the
deletion is complete, Horizon removes the cloud and its panels. A cloud that
has no worker or storage shows **Remove cloud** instead. If Horizon then finds
storage in the record of the cloud, the dialog shows the reason and
**Delete cloud resources**.

If the deletion fails or cannot start, or if Horizon cannot read the resource
state of the cloud, the dialog shows the reason and **Remove from Horizon
anyway**. This action removes the cloud and its panels from Horizon, but it
does not delete the resources at the provider. The dialog names the resources
that can remain. They can cost money until you delete them at the provider.
Horizon keeps the state directory of the cloud, including its operation lock.
This keeps all controllers on the same lock file. **Cancel** keeps the cloud.

While a different operation of the cloud runs, for example a deployment or a
provider check, the dialog tells you to wait and shows only **Cancel**. This is
also true for an operation in a different Horizon instance. When the operation
ends, the open dialog shows the actions again. If another instance takes the
record lock between a dialog refresh and a removal click, Horizon refuses the
removal and keeps the cloud. The dialog shows only **Cancel** until that
operation ends.

### Parked terminals

When no terminal of a ready cloud is on the screen for 2 minutes, Horizon parks
the terminals of that cloud. A parked terminal has no SSH client, no terminal and
no process on this computer. It shows the text of its last screen in a dim color,
without a cursor, and its session continues in tmux on the worker. The focused panel and a panel that fills the
window do not park.

The cloud attaches again after it stays in view for 1 second. It attaches at
once when one of its panels gets focus. A cloud that becomes ready when it is
not in view parks its terminals immediately, so a restart opens no connection
for clouds that you do not look at. Browser panels and Device panels do not park.

A parked terminal takes no input. Keys or a middle-click paste aimed at it are not
sent; they make it attach at once, and you type or paste again when it is back.
Horizon does not keep that input for later, because the program in the session
may not have set its terminal modes yet, and a multi-line paste could then run
as separate commands.

While a cloud has parked terminals, Horizon reads the state of their sessions
every 10 seconds through one SSH command. A parked panel shows this state in a
strip at its bottom on the main canvas: working or idle, ended, or not found, with
the last line of the session. In a detached window or a fullscreen cloud, a parked
panel shows only its last screen until it attaches. The worker needs only Python 3
and tmux for this.

A new cloud opens its first panel when it becomes ready, and the canvas moves to
it. When you went to another workspace while the cloud deployed, your view and
focus stay where they are: the cloud list shows the cloud as ready, and the first
panel opens when you come to the cloud's workspace, for example with a click on
its row.

### Cloud list

The sidebar groups the workspaces by what they need from you:

| Group | Workspaces |
|---|---|
| **Needs you** | A cloud that failed, waits for a decision, asks for GitHub access, or has a parked session that ended or is not found. |
| **Cloud** | A cloud that is attached, disconnected, not deployed yet, or busy with an operation. A disconnected cloud shows **Disconnected**: its sessions continue on the worker. |
| **Parked** | A cloud with parked terminals, or with a stopped worker. |
| **This PC** | A workspace without a cloud. |

Each group header shows how many workspaces it has. At the right of the header
line, a summary in secondary text shows what the running workers of the group
cost a day, **no local cost** for **Parked**, and **live** for **This PC**. The
cost has one sum for each currency, for example `€0.58/day + $16.56/day`: Hetzner
bills in euros net of VAT and RunPod in US dollars. The
summary never wraps. When the sidebar is narrow, it shows fewer parts, and then a
shortened first part. Hover over it to see all of it. If no part fits on the
narrowest sidebar, the header does not show the summary. A screen reader still
reads it after the group name and count.

Each row shows a tile, the workspace name and one status line. The tile has the
color of the workspace on the canvas and the first letters of the first two words
of its name. The status line is the last line with words on the screen of the
primary terminal (the first agent, else the focused terminal, else the first
terminal). Frames, prompts and footer hints are not status lines, and terminal
padding becomes single spaces. For a parked cloud, the line comes from the session
status that the worker sends. A parked row is compact: it shows only a smaller
tile and the name, and its tile shows the status line on hover.

A badge on the lower-right corner of the tile shows the status. A filled badge is
green when an agent works, yellow when the workspace waits for you, red when an
operation failed, and the accent color while Horizon works on the cloud. A ring is
a parked cloud (green when an agent still works on the worker), or a stopped
worker. A workspace where nothing runs has no badge. Hover over the tile to see
what the badge means.

Click a row to go to its workspace. A parked cloud then attaches, as described in
[Parked terminals](#parked-terminals).

#### The cloud list for agents

A local agent panel in a workspace reads the clouds of that workspace with the
`cloud_list` MCP tool, or with `horizon-browser cloud list` in its shell. The
answer has the ID, the name, the group, the status line and the hourly rate of
each cloud, and whether an agent works on it. The agent can act on one of these
clouds as the sidebar does:

- **attach** moves your view to the cloud, as a click on its row does.
- **park** parks the terminals of a cloud that is out of view now. Horizon
  refuses a cloud in view, because it attaches again.

Each entry also has `worker_image` and `image_publication`. They describe the
image and publishing identity retained by that cloud, independently of current
account settings. Either can be `null` while its matching runtime is unavailable;
an unresolved image is also `null`, and legacy records have no publication. A
registry publisher includes its opaque nonsecret binding ID, never a credential
value or path. This reads metadata only, without opening
credentials or checking current registry access. It never changes a cloud's
destination.

The cloud list does not stop workers. Only you stop a worker, with **Stop** on its
card.

An agent sees only the clouds of its own workspace. Clouds of other workspaces
and requests from outside Horizon are not available.

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

Stop ends the processes on the worker. The storage can stay billable. No cloud
stops when idle. `idle_stop_minutes` (10 to 1440) still parses, so an older
`.horizon/cloud.yml` keeps working, but it has no effect: Horizon gives no worker
an idle period, and **New cloud** and **Rebuild** show a note for a profile that
sets one. Instead, an agent stops its worker when its task is done (see below). A
RunPod cloud created before this change still carries its period in
`HORIZON_IDLE_STOP_MINUTES`; it reconnects as before, and its image stops it when
idle until a rebuild moves it to a current image, which has no idle stop. Such a
worker is active while an agent terminal prints output, or while its container uses
half a CPU core or more. It stops itself with a provider credential that applies
only to that worker, also when this computer is offline, and never deletes
anything.

For each RunPod cloud, Horizon asks the provider for the status of the worker in
these conditions:

- The connection to a ready worker fails.
- A reconnect does not find the worker, or finds a record that says stopped.

Horizon does not do this check for a Hetzner cloud. Only Horizon stops a Hetzner
cloud, so a failure there stays a failure.

During this check, the card shows **Checking provider**. If the provider reports
that the worker stopped, the card shows **Stopped** · **Storage kept · billable** ·
**Stopped outside Horizon** with **Resume worker**, and logs `The provider reports
that this worker is stopped. Horizon did not stop it; …`. It does not show
**Operation failed**. Horizon cannot tell the cause: an agent, the idle stop of a
worker created before this change, or the provider account can stop a worker.

If the provider reports that the worker runs, the card shows the original
failure. Horizon records a stop that the provider confirms as an ordinary stopped
cloud. Nothing resumes it automatically. If the record already showed the stop,
the card shows **Stopped**. You can also choose **Check provider** at any time.
A worker created now stops only with **Stop** or at an agent's request.

An agent can stop its worker when its task is done, such as when its pull request
is merged. A worker image built from the current worker bootstrap offers this on
every dedicated RunPod worker, with no idle period, and has no idle stop of its
own; it reports `horizon-agent-stop-contract=1`. Older images offer it only to a
worker with an idle period. On RunPod the worker stops itself, without this computer.
A worker on Hetzner or a Docker host has no credential that can stop it, so
Horizon starts it with `HORIZON_WORKER_STOP=horizon`: the worker records the
request, and while the cloud is ready and no other operation runs, Horizon reads it
every 30 seconds (`horizon-worker-stopd --take`) and then stops the cloud as **Stop**
does, after it asks the worker once more that the request still holds. The card shows `<agent> asked to stop this cloud: <reason>`, then **Stopped
by <agent>** with the reason. While another operation of the cloud runs, or one
of its dialogs is open, Horizon gives the request back to the worker, which drops
it; the agent may ask again. This works only while Horizon runs: a request that
Horizon does not take within ten minutes lapses, and the worker keeps running. A
worker created before this change has no such setting and offers no agent stop: a
Hetzner cloud gets it with its next server, after Stop and Resume or a rebuild, and
a Docker host cloud only when it is created again. Shared workers and profiles
with hosted devices offer no agent stop. Agents get a
`stop_this_worker` MCP tool (or run `horizon-worker-stop --reason "..."`) with a
one-line reason. The worker identifies the requesting agent session itself,
refuses callers outside a Claude, Codex or Grok session (including shell sessions), and
refuses while another agent session printed output in the last two minutes, while
the container is busy, or when it cannot check its sessions or CPU use; the calling agent's own
output does not count. When Horizon takes a recorded request, it checks again and
then counts the output of every agent session, the caller's too. The reason and the requesting agent are kept on the
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
The worker desktop is view-only by design: its VNC server ignores mouse and keyboard
input. Thus this Device panel does not offer **Interact** and shows the reason.
Agents on the worker use the device tools for input. Local and SSH Device panels keep **Interact**.
All agent tools run on the worker, independently of the laptop connection.
The worker supports the public browser tools and standalone device input tools.
`device_panel` manages viewers in a running Horizon window, so it is unavailable
inside the headless worker and returns `viewer_requires_horizon` immediately.
It does not substitute a browser viewer or depend on the laptop for device input.

The development example `cargo run -p horizon-core --example cloud_deploy -- ...`
uses the same coordinator. Run it without arguments for its command synopsis.
With `--quick-start`, `deploy` and `prepare-image` use the built-in quick start
profile, as **New cloud** does. Then the profile argument must be `quick-start`.
These commands do not use Docker. A `rebuild` of a quick start cloud does not use
Docker either, if the cloud has the capabilities of the current `quick-start` profile.
It supports image preparation without allocation, deployment, stop, resume,
reconnect, endpoint, deletion, `rebuild SETTINGS STATE_ROOT PROFILE [PLAN_TOKEN]` with `continue-rebuild` and
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
settings without the placement the card records for a cloud. For an exact
Hetzner choice, resume keeps the recorded server type and location. The recorded
location must still be allowed by the settings. For an older record without an
exact choice, resume refreshes the fallback types and locations from the settings
file. The workspace volume still fixes the location.

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
example to pick up newer agent CLIs or a change to its `.horizon` recipe. You do
not have to commit the change first. Horizon resolves the repository's latest commit
(`HEAD`) and reads the working copy of `.horizon/cloud.yml`. The `source` and
`companions` fields come from the commit. If the working copy has no
`.horizon/cloud.yml`, or the file is not valid, Horizon uses the file of the commit,
and the confirmation says so. A cloud on the public base image without a `build`
section reads only the commit.

Horizon refuses the rebuild when the cloud's profile is missing in that file.

A rebuild can change these settings of the cloud. The cloud gets the values of the
file:

- `capabilities`: the agents, the browsers, the desktop and `agent_approvals`. For
  example, you can add the agent `codex` to a cloud that has only `claude`.
- `bootstrap.readiness_seconds`, the readiness timeout.
- The `build` section: `context`, `dockerfile` and `platform`.

Horizon refuses the rebuild when the profile is different from the profile of the
cloud's worker in another field. The refusal names the field. A running worker cannot
change these fields:

- `gpu`, the `cpu` and `memory_gb` of a GPU cloud, `min_cuda_version` and
  `min_gpu_memory_gb`. The worker keeps its GPU host.
- `image`. The rebuild pushes to the image repository of the worker, and the worker
  pulls with its own pull binding.
- `capabilities.browserstack`. The hosted devices are set when the worker is created.
- `bootstrap.contract_version`. The worker keeps its bootstrap contract. Version 1 is
  the only version that a profile can name now.
- A missing `build` section. Then there is no recipe to rebuild.

Horizon also refuses a profile with values that no profile can have, for example
`idle_stop_minutes` outside 10 to 1440. A changed `idle_stop_minutes` is no change:
it has no effect, and Rebuild says so. For each change, the rebuild output
shows a line that starts with `Profile change:`, for example `Profile change: Adds
agent codex`. Before Horizon builds, it checks the API key file of each agent in the
new profile, not only of an added agent, as each deployment does. A reconnect of the
cloud needs each of these files too. Thus an unusable key file stops the rebuild
before the long build and before the worker changes. The refusal names the agent, the file
and the problem, and says that a reconnect needs the file too, for example `Agent
claude cannot use its API-key file /home/me/keys/anthropic: the file is missing`.
Fix the file, or remove the agent from the profile, and rebuild again. **Continue
rebuild** does the same check until Horizon requests the image switch.

Some differences are not a change. When you create the cloud, you choose these
values in **New cloud**:

- the provider;
- the vCPU and memory of a CPU cloud;
- the storage: the container disk (`storage.container_gb`, **System disk
  requirement** on Hetzner), the workspace size (`storage.volume_gb`) and the
  workspace type (`storage.volume_tier`, on a RunPod CPU cloud only).

Their values in the file are only defaults for new clouds. Thus the rebuild keeps the
cloud on its provider, its size and its storage. A workspace that grew after
creation is not a change either.

For a profile with a build section, Horizon builds the recipe with the newest agent CLIs
under a new tag. The build context is the working copy, as
[Uncommitted files in the image](#uncommitted-files-in-the-image) tells. The build uses the new capabilities, and the worker contract check
uses them too, so the image must contain each added agent. The image gets the label
`io.horizon.worker.settings` with the rebuild settings. Thus a change of the settings
always gives a new image. Horizon then pushes the image and verifies the worker's pull
binding. An unchanged image digest is reported, and nothing restarts.
A profile without a build section has no recipe to rebuild. The next paragraph tells
the only exception.

A cloud on the public base image without a `build` section can also rebuild. A quick
start cloud is an example. Its latest commit must have no `.horizon/cloud.yml`. If it
has one, Horizon refuses the rebuild, because it never ignores committed settings.
The rebuild builds nothing. Horizon switches the worker to the base image that this
Horizon version pins. Thus after an upgrade that changes the pin, **Rebuild image &
restart** moves the cloud to the new image. If the cloud has the capabilities of the
current `quick-start` profile, the contract check uses the kept result, as
[Quick start](#quick-start) tells. Then this rebuild does not need Docker. Otherwise
Horizon checks the image in local Docker. The workspace volume, the profile and the
sessions stay, as in each other rebuild. If the cloud already runs the pin, Horizon
reports an unchanged image and restarts nothing.

Before Horizon changes the worker, it compares the unpacked size of a built image, as
local Docker shows it, with the container disk of the cloud's worker. On Hetzner, the
container disk is the system disk requirement, the smallest system disk that the new
server can have. The deployment output shows the two sizes. If the image is larger,
Horizon refuses the rebuild with the field `storage.container_gb`. The worker keeps its
image, and nothing stays pending. The pushed image stays in the registry. Horizon does
not compare the pinned base image, because it builds nothing. **Continue rebuild**
checks a built image again: its local tag must still name the pushed image, and its
size must fit. If the image is larger, the rebuild stays pending, so that **Cancel
rebuild** can end it.

Horizon then releases any hosted devices and switches the existing worker to the
new image through the provider's pod update, which keeps the worker ID. A Hetzner
cloud instead releases its server and starts a new one, with a new server ID and
address, on the same workspace volume; see
[Hetzner](cloud-hetzner.md#rebuilding-the-image). On either provider `/workspace`
(worktrees, agent logins, SSH host keys) survives; the container disk is reset and
running processes end. The worker gets the new capabilities in its
environment: the pod update sends them on RunPod, and the new server gets them on
Hetzner. The cloud's profile changes only when the worker runs the new image. Until
then, and after a cancelled rebuild, the cloud keeps its previous profile. After
readiness, Horizon gives the credentials of each added agent to the worker, as for a
new cloud, and relaunches each session's process in its existing worktree without
resetting it. Images without the relaunch contract report those sessions lost, as
Stop and Resume do. A session of an agent that the rebuild removed does not relaunch.
The output reports it once, for example `Session panel-1 (codex): this cloud's profile
no longer has agent codex, so its process was not relaunched`. The cloud keeps the
record of the session, marked as reported, so later reconnects, also after a restart
of Horizon, do not report it again. If a later rebuild adds the agent again, the
session relaunches in its worktree and the mark is cleared. A session that ends for
another reason is always reported.

The replacement is journaled in the deployment record before each provider
mutation, and older Horizon versions cannot read the record while the update may
be in flight. After an interruption, reconnect and **Check provider** only read
the provider: a worker already on the new image completes the switch, and anything
else stays blocked as a pending replacement. Continuing it sends the update again
only while the worker still reports its previous image. Cancelling it drops an
unsent update or switches the worker back and relaunches its sessions. Deletion
stays available throughout.

On the runtime card of a Ready cloud, **Rebuild image & restart…** asks for
confirmation and names these consequences first. The card shows it if the profile
has a build section or runs the base image. The confirmation reads the checkout
first. Then it lists each change of the cloud's settings, for example `Adds agent
codex`, `Removes browser firefox` or `Readiness timeout 1800 s → 3600 s`, or it tells
that the settings stay. It also lists each uncommitted file that goes into the image.
If Horizon refuses the settings or a file, the confirmation shows the reason and does
not start the rebuild. If the settings or the uncommitted files change after you
confirm, Horizon refuses the rebuild, and you must confirm again. The
`cloud_deploy plan-rebuild SETTINGS STATE_ROOT PROFILE` harness command prints the
same lists. For a cloud on the base image, **Build
locally** and **Push image** show as skipped. While it runs, the card lists the rebuild's steps with their durations and
offers **Cancel rebuild** only until the image switch is requested. A rebuild
cancelled while its recipe is read leaves nothing pending, and the card
offers **Reconnect cloud**; cancelled later, it stays pending. A pending replacement shows a notice with **Continue
rebuild** and **Cancel rebuild**, and Stop waits until it is resolved. Cancelling
a switch that may have been sent asks for confirmation, because switching back
restarts the worker again, and Horizon does not reconnect such a cloud on its own
at startup. The card keeps the outcome, such as an unchanged image or a session
that could not be relaunched, until the next deployment.

### Uncommitted files in the image

**New cloud** on the checked-out commit and a rebuild build the image from the
working copy, so that you can change the recipe and build without a commit. Horizon
extracts the commit, as for each build, and then copies these files over it:

- each tracked file that you changed, with its content in the working copy;
- each untracked file that Git does not ignore;
- the removal of each tracked file that you deleted.

Only the files in the `build.context` folder, the `build.dockerfile` file and its own
`<Dockerfile>.dockerignore` count, because the build reads only them. Names compare
without case. When a build path has `..` or passes a symbolic link in the working copy,
all of the checkout counts, because the build follows the link. Horizon reads the
working copy with libgit2. Thus no program that the repository's Git configuration
names runs, for example a content filter or a file system monitor. Horizon refuses a
configuration that names another working folder (`core.worktree`).

> **WARNING:** THE IMAGE CAN GO TO A PUBLIC REGISTRY. Review the selected
> destination and its visibility. Each uncommitted file in the build
> context can go into the image. Examine the list before you click **Start cloud**
> in **New cloud** or **Rebuild and restart** in the rebuild confirmation.

These rules keep files out of the image:

- Horizon never copies a file that Git ignores: `.gitignore`, `.git/info/exclude`
  and the global exclude file apply. This includes a file that you removed from the
  index with `git rm --cached` and then ignored: the image does not get it. A tracked
  file stays tracked, also when it matches `.gitignore`, so its changes are listed.
  Put local files, logs and secrets in `.gitignore`.
- `.dockerignore` applies when Docker reads the context, also a changed or new
  `.dockerignore`. The confirmation can list a file that `.dockerignore` excludes.
  Docker does not put that file in the image.
- Horizon refuses the build when an uncommitted file in the build context has the
  name of a common secret file: `.env`, `.env.<name>`, `<name>.env` (but not an
  `.example`, `.sample`, `.template` or `.dist` file) and `.envrc`; a file that ends
  in `.pem`, `.key`, `.p8`, `.p12`, `.pfx`, `.jks`, `.keystore`, `.kdbx`, `.tfvars` or
  `.tfstate`; an SSH private key (`id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519` and
  copies such as `id_rsa.bak`, but not a `.pub` file); `.netrc`, `.npmrc`, `.pypirc`,
  `.pgpass`, `.htpasswd`, `.git-credentials`, `.aws/credentials` and
  `.docker/config.json`. The refusal names the file. This also
  applies to a tracked file that you changed. Add the file to `.gitignore`, or
  commit or remove the change.
- Horizon copies a symbolic link as a link. It never reads the target of the link.
  It refuses a file that it can only reach through a folder that is a link. On Linux
  and macOS it refuses a hard link, because a hard link can share its content with a
  file outside the repository. It also refuses a file name with a control character
  or a text-direction character, because the list could show such a name incorrectly.
- On Windows, Horizon refuses an uncommitted symbolic link or junction in the build
  context, because it cannot copy it there. It does not detect a hard link there.
- Submodules and same-worker siblings come from their commits. An untracked nested
  Git repository is not part of the build.
- The source that the worker gets is the committed archive. Only the image recipe
  uses the working copy.

Horizon does not examine the content of a file. A secret in a file with an ordinary
name goes into the image. You must examine the list.

The list shows each file with `modified`, `new` or `deleted`. Horizon calculates a
SHA-256 hash of the paths and the contents of the files. It records this hash with
the rebuild, or with the new cloud when you click **Start cloud**. Before the build, Horizon
reads the files again. If a file is different from the confirmed file, or a file was
added, Horizon refuses the build. **Continue rebuild** and a retry of a new cloud do
the same check. **Cancel rebuild** and a new rebuild let you confirm the files again.
For a new cloud, put the files back as they were, or delete the cloud and create a
new one. Without uncommitted files, the list says that the image builds from the
latest commit.

An image built from uncommitted files, by **New cloud** or by a rebuild, gets the
label `io.horizon.worker.uncommitted` with the commit and the hash. Thus a build of the
same files can use the build cache and give the same image digest, and a rebuild from
other files or from the commit alone gives a different one. Until a rebuild from a
commit only, the card shows `Image: built from <commit> plus uncommitted changes`.
Commit the changes to make the image reproducible.

`cloud_deploy plan-rebuild SETTINGS STATE_ROOT PROFILE` prints each file as
`Uncommitted file: modified <path>` and then a plan token. The token covers the
settings and the files. To confirm the plan, give the token to
`cloud_deploy rebuild SETTINGS STATE_ROOT PROFILE <token>`. If the settings or the
files changed after `plan-rebuild`, the rebuild is refused. Without uncommitted files
you can give no token; then the rebuild takes the settings as it reads them.

`cloud_deploy plan-deploy SETTINGS REPOSITORY PROFILE STATE_ROOT CLOUD_ID` reads the
settings as **New cloud** does, prints each uncommitted file of the image in the same
form and then a plan token. Give it to `cloud_deploy deploy` or `prepare-image` with
the same arguments and `--plan-token <token>`. The token also covers the worker that
`--worker-choice` selects, so a plan for one offer does not start another. Without
uncommitted files you can give no token. A typed `REVISION` that is not the checked-out commit builds that commit
only.

### What a cloud costs

While a worker bills, the header of its card shows the cost of the current run
and the cost a day, for example `€0.06 run · €0.58/day`. When the header has room,
a second line shows the cost a week and a month, and what the worker adds by the
end of the month if it keeps running:
`€4.03/wk · €17.52/mo · +€9.89 by Oct 31 if left running`. A narrow card keeps
only the first line. Hover over the cost to see the hourly rate, the cost a year
and what the worker adds by the end of the year.

A day is 24 hours, a week 168, a month 730 and a year 8,760, as Hetzner counts
them. The end of a month and of a year is midnight UTC. The amounts are compute
only: storage, a primary IPv4 address, traffic and VAT are not in them. Hetzner
caps a month at the monthly price of the server type, so a full month can cost
less than 730 hours. A stopped or deleted worker shows no daily cost.

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
On a worker with agent isolation, agent and shell sessions run as the agent user
`horizon-agent` (UID 10001). Ready then also means that the alias resolves for
the agent user. The agent
user gets a read-only copy of the alias, the key, the host-key pin and the
catalog. The root files stay private. When you uncheck the companion, the source
also removes the agent copy. A key that an agent copied works until the target
revokes the grant. See
[agent access](companion-worker-protocol.md#agent-access).
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
- `cloud_companion_uncheck` unchecks a companion as the box on the card does,
  when the task no longer needs it: the source's access goes, and the target's
  grant once both workers confirm. It never starts, stops or deletes a cloud.
  Agents can uncheck but never check, because checking is the owner's
  authorization.
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
