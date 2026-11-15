# A machine for a repository

A repository says in its own `AGENTS.md` what a machine for its work needs. The
person asks for one such machine, or two, and gets machines that are ready for
that repository: the system packages are there, the tools are there, the
repository is there, its caches are warm, and the check of the repository passed
on the machine.

This page holds the recipe, the cards of one run, the way that the repository
arrives, and the trust of the folder that it made. The provider commands, the
rent, the wait for SSH and the delete are the steps of the skill.

## The recipe

The recipe is the section `## Horizon machine` of `AGENTS.md` of the repository.
It sits beside `## Horizon plan` and it is read before any machine is rented.

| Entry | What it holds | Example |
|---|---|---|
| `resources` | vCPU and memory that the work needs. Never a type name of a provider | `4 vCPU, 8 GB` |
| `count` | How many machines one run prepares when the person asks for the default | `1` |
| `place` | `any`, or the place when the work needs one, such as the region of the data | `any` |
| `packages` | System packages, as the package manager of the image takes them | `build-essential, pkg-config, cmake` |
| `tools` | Each tool with the version that the work needs and how it is installed | `rust 1.95, rustup` |
| `warmup` | Commands that fill the caches before the check runs | `cargo fetch` |
| `check` | The one command that proves the machine | `cargo test --workspace` |
| `repository` | How the repository arrives: `bundle`, or `clone` with the name of a credential | `bundle` |
| `branch` | The branch that the machine holds, when `AGENTS.md` names the branch of the work. Absent when the work uses the branch that the clone has | `work/login-redirect` |
| `network` | The stored name of the Tailscale network that the machine joins. Absent when the machine joins no network | `home` |

A section of this shape is a recipe:

```markdown
## Horizon machine

- resources: 4 vCPU, 8 GB
- count: 1
- place: any
- packages: build-essential, pkg-config, libssl-dev
- tools: rust 1.95 (rustup), python 3.13 (system)
- warmup: cargo fetch
- check: cargo test --workspace
- repository: bundle
```

Give this block to the person to paste when the repository holds no section.

The entry `network` holds the stored name of a network and never an address or a
secret. A recipe names it only when the work of the repository reaches that
network. A recipe without the entry prepares a machine in no network.

## A repository without a section

When `AGENTS.md` holds no `## Horizon machine` section, ask one question on the
card and offer to write the section. Do not guess in silence and do not rent a
machine from a guess.

1. Read what the repository says about its own machine: the workflow files under
   `.github/workflows/`, a toolchain file such as `rust-toolchain.toml` or
   `.python-version`, a lock file, and the build steps of `README.md`.
2. Draw one card with the heading `No machine recipe in this repository`, the
   rows that you read with their source, the buttons `Write the section` with
   tone `primary` and `Not now`, and one `caption` that names the file that you
   would change.
3. On `Write the section`, append the section to `AGENTS.md` in the worktree of
   the task, draw it on the card, and rent nothing until the person presses the
   rent of the run. On `Not now`, stop the run and keep the card.

A written section is the person's file: it goes to the repository on its own
branch, as every other change of that repository does.

## The run

Read the recipe, then do these steps in this order.

### 1. The rent card

One card states the whole rent, and one press rents every machine of the run:

- `resources` of the recipe, and the type that holds them: the cheapest type of
  the provider at that place that holds the vCPU and the memory of the recipe,
  as the provider answers it in this run.
- `count`, which is the number of the run, the `count` of the recipe, or the
  number that the person asked for.
- The price for each hour of every machine of the run together, for example
  `2 machines, 0.0214 EUR / hour together`. Read both prices in this run and add
  them. When the provider answers no price, write `price not read`.
- The network of the machine, when the panel of the agent holds one or more
  stored networks: the caption `Network` and the pills of the cards of Set up,
  with `network.none` first. The pill of the `network` entry of the recipe is
  `selected: true`.

```json
{"components": [
  {"type": "heading", "text": "Two machines for example-repo", "subtitle": "The recipe of AGENTS.md",
   "badge": {"text": "Rent?", "tone": "info"}},
  {"type": "kv", "rows": [
    {"label": "Resources", "value": "4 vCPU, 8 GB"},
    {"label": "Type", "value": "the cheapest type of this place that holds them"},
    {"label": "Machines", "value": "2"},
    {"label": "Price", "value": "2 x 0.0107 EUR / hour, 0.0214 EUR / hour together", "mono": true}]},
  {"type": "caption", "text": "Network"},
  {"type": "list", "selection": "single", "layout": "pills", "rows": [
    {"id": "network.none", "title": "None"},
    {"id": "network.home", "title": "home", "selected": true}]},
  {"type": "text", "text": "One press rents both machines. The setup of each machine follows on its own card."},
  {"type": "buttons", "items": [
    {"id": "rent", "label": "Rent 2 machines", "tone": "primary"},
    {"id": "cancel", "label": "Cancel", "tone": "danger"}]}]}
```

Draw the caption and the pills only when the panel holds a stored network. A
recipe that names a network that the panel does not hold gets no pill for it:
say in one `text` line which three names are missing, and join nothing from a
guess. A press on a pill changes the network of this run, and `None` keeps the
machine out of every network.

### 2. One steps card for each machine

After the press, rent each machine and give each one a card of its own, titled
with its name. The card shows the six steps of the machine, and one step more
when the machine joins a network:

1. `Prepare the system`: the packages of the recipe, with the package manager of
   the image, and the update of the package list first.
2. `Install the tools`: each tool with the version of the recipe, installed in
   the way that the recipe names.
3. `Join the network <NET>`: the network of the run, when it is not `None`. The
   page [tailscale](tailscale.md) gives the commands and the proof. The machine
   joins after `Prepare the system` and before the repository arrives.
4. `Bring the repository`: the bundle or the clone of the section below.
5. `Trust the repository folder`: the trust of the folder of the clone for each
   agent program on the machine, as
   [the trust of the folder](#the-trust-of-the-folder-of-the-repository) says.
6. `Warm up`: the `warmup` commands of the recipe, in the folder of the
   repository.
7. `Run the check`: the `check` command of the recipe, in the folder of the
   repository.

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Ready for example-repo",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Prepare the machine", "percent": 50, "detail": "3 of 7"},
  {"type": "steps", "steps": [
    {"label": "Prepare the system", "state": "done", "at": "18 s"},
    {"label": "Install the tools", "state": "done", "at": "2 m 10 s"},
    {"label": "Join the network home", "state": "now", "at": "now"},
    {"label": "Bring the repository"},
    {"label": "Trust the repository folder"},
    {"label": "Warm up"},
    {"label": "Run the check"}]},
  {"type": "buttons", "items": [
    {"id": "stop", "label": "Delete the machine", "tone": "danger"}]}]}
```

Two machines are prepared at the same time when the `at_once` of `## Horizon
plan` allows two commands. Each machine has its own card, its own steps and its
own session, and the cards change independently.

### 3. The machine card says what the machine is ready for

When the check passed, the heading says `Ready for <repository>`, the badge is
`Ready` with tone `ok`, and the evidence of the check is one `text` line with
the command and its result, for example
`cargo test --workspace: 412 tests pass, 0 failed`. Then write the record of the
machine with `machine_set`, as step 5b of the skill says, and add `Connect`.

A machine that is ready for a repository names it in the field `repository` of
its record:

```
operation:  machine_set
name:       "build-1"
ssh:        {"host": "<address>", "user": "<user from the reference>"}
key:        "build-1"
session:    "build-1"
provider:   "<Provider>, <region>"
repository: "example-org/example-repo"
```

The field holds the name of the repository, such as `example-org/example-repo`,
and not the entry `repository` of the recipe, which says how the repository
arrives. Horizon draws it on the second line of the row of the machine in the
sidebar, beside the provider words, and it reads no folder from it. Set the
field only when the check of the recipe passed on that machine; a machine that
is ready for no repository has no field. A record that holds the field needs a
Horizon that knows it on every computer that follows one server, because an
older Horizon refuses the record of a newer one.

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Ready for example-repo",
   "badge": {"text": "Ready", "tone": "ok"}},
  {"type": "steps", "steps": [
    {"label": "Prepare the system", "state": "done", "at": "18 s"},
    {"label": "Install the tools", "state": "done", "at": "2 m 10 s"},
    {"label": "Join the network home", "state": "done", "at": "31 s"},
    {"label": "Bring the repository", "state": "done", "at": "8 s"},
    {"label": "Trust the repository folder", "state": "done", "at": "2 s"},
    {"label": "Warm up", "state": "done", "at": "1 m 40 s"},
    {"label": "Run the check", "state": "done", "at": "6 m 02 s"}]},
  {"type": "kv", "rows": [
    {"label": "Folder trusted for", "value": "Claude Code, Codex"},
    {"label": "Network", "value": "home", "mono": true}]},
  {"type": "text", "text": "cargo test --workspace: 412 tests pass, 0 failed"},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
    {"id": "server_here", "label": "Put the Horizon server here"},
    {"id": "browser_here", "label": "Add a browser"},
    {"id": "join_network", "label": "Join a network"},
    {"id": "stop", "label": "Stop"},
    {"id": "delete", "label": "Delete", "tone": "danger"}]}]}
```

The ready card of a machine that was prepared for a repository holds the row
`Folder trusted for` with the programs that trust the folder of the repository,
for example `Folder trusted for: Claude Code, Codex`. The step that writes the
trust names the programs, and a machine whose agent programs were installed
before the repository arrived holds that row too.

The ready card of a machine that joined a network holds the row `Network` with
the stored name of that network, and a machine in no network has no such row.
The page [tailscale](tailscale.md) gives the rows and the proof that the machine
is in the network.

The ready card of a machine that was prepared for a repository keeps the buttons
of every machine card of the skill `horizon-machines`: `Connect`, `Put the
Horizon server here`, `Add a browser`, `Join a network`, `Stop`, and `Delete`.
Draw `Start` instead
of `Stop` while the machine is stopped. A card that holds only `Connect` and
`Delete` is a card that cannot continue the set-up, and the person must open
another card to put the server or a browser there.

### 4. A step that fails keeps its card

A machine that failed a step keeps its card: the heading keeps the name of the
machine, the step that failed is `failed` in the list, the badge is `Stopped`
with tone `warn`, one `text` component holds the last lines of the output of that
step, and the card holds the buttons `Try again` and `Delete the machine`. The
state `failed` draws the red cross of a failed board item, so the person reads
the step that stopped. Never rent a second machine for the same card, and never
hide a failed step under a `Working` badge.

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "example-repo",
   "badge": {"text": "Stopped", "tone": "warn"}},
  {"type": "steps", "steps": [
    {"label": "Prepare the system", "state": "done", "at": "18 s"},
    {"label": "Install the tools", "state": "done", "at": "2 m 10 s"},
    {"label": "Bring the repository", "state": "done", "at": "8 s"},
    {"label": "Trust the repository folder", "state": "done", "at": "2 s"},
    {"label": "Warm up", "state": "failed", "at": "40 s"},
    {"label": "Run the check"}]},
  {"type": "text", "text": "cargo fetch: error: failed to load source for dependency `ring`"},
  {"type": "buttons", "items": [
    {"id": "retry", "label": "Try again", "tone": "primary"},
    {"id": "delete", "label": "Delete the machine", "tone": "danger"}]}]}
```

`Try again` repeats the failed step and the steps after it, on the same machine.
`Delete the machine` runs the delete of the skill and removes the card after the
delete, as the confirm card of the skill asks first.

## How the repository arrives

The default is a bundle, so no credential of the person is on the machine:

```sh
git bundle create <state>/example-repo.bundle --all
scp -p <state>/example-repo.bundle <user>@<address>:~/example-repo.bundle
ssh ... 'git clone ~/example-repo.bundle ~/example-repo && rm ~/example-repo.bundle'
```

A repository whose `AGENTS.md` names the branch of the work, as the entry
`branch` of the recipe gives it, gets the bundle of that branch and of the
branches that it needs:

```sh
git bundle create <state>/example-repo.bundle <branch>
```

The machine then holds the branch that the person works on, and the card says
which branch the machine holds and why: `The machine holds work/login-redirect,
the branch that AGENTS.md names for this work. Say so if you want another
branch.` A person reads the branch in one line of the card, and no run leaves the
branch to a guess.

A bundle of every branch holds every branch and every tag of the clone that made
it, and it takes longer to copy and more room on the machine. Use it when the
recipe names no `branch`. No credential reaches the machine either way. A clone from a remote is the other way, and it
needs a credential on the machine:

- Only when the `repository` entry of the recipe says `clone`.
- Only when the person agrees on a card that names the credential, for example
  `An SSH key of this computer is copied to build-1 for the clone of
  example-repo. Say no to use a bundle instead.`
- The card holds the buttons `Copy the credential` with tone `primary` and
  `Use a bundle`.
- A credential that the person confirms is copied for one clone and removed from
  the machine after the clone, and it never appears in a card, a log line or an
  argument.

A repository folder on the machine is the clone of one machine: the helper that
works there writes in that clone, and a second helper of the same repository gets
a worktree of it, as the skill `horizon-worktree` says.

## The trust of the folder of the repository

An agent program asks at its first start in a folder whether the person trusts
that folder, and it waits for the answer before it works. A machine that was
rented for one repository has nobody at that question, so the step `Trust the
repository folder` answers it: after `Bring the repository` it writes the trust
of the folder of the clone, for each agent program that the machine has.

A folder can hold settings that run code when the program starts, so the question
protects the person from a folder that they did not choose. The rent named the
repository of this machine, so the rent is the answer of the person for that
folder; a repository of another party that the person does not know is a reason
to rent with no repository and to clone it by hand.

The step writes the trust of one folder, the folder of the clone, and of no other
folder:

- Read `<path>` with `realpath` on the machine, so it is the absolute canonical
  path.
- Never a parent folder, never the home folder, and never the setting
  `defaultProjectTrust` of pi. Never a flag that turns the approval of a tool call
  off: the trust of a folder is not the permission mode of a program, and this
  step changes no permission mode.
- Only for a program that the machine has: `command -v claude`, `command -v
  codex` and `command -v pi` answer a path when it is there. A program that the
  person installs on the machine later is no part of this step: it gets its entry
  when the person asks, for example "trust the repository folder for pi on
  build-1".
- Merge into each file and never replace it. When the machine has no `python3` and
  no `jq`, the file of that program stays as it is, and the card names that
  program with the reason.
- A file that does not exist is made with the mode 600, in a folder with the mode
  700.

Run each block on the machine with the form of step 5 of the skill. A second run
of a block changes no byte of the file.

### The trust of Claude Code

The file is `~/.claude.json` of the SSH user, and the entry is
`projects["<path>"].hasTrustDialogAccepted = true`. The block keeps every other
key of the file and of that project.

```sh
ssh <options of the machine key> <user>@<address> sh <<'SH'
command -v claude >/dev/null || exit 0
path=$(realpath ~/example-repo)
file=$HOME/.claude.json
if [ ! -e "$file" ]; then
  printf '{"projects": {"%s": {"hasTrustDialogAccepted": true}}}\n' "$path" > "$file"
  chmod 600 "$file"
  echo "$file: written"
  exit 0
fi
if command -v python3 >/dev/null; then
  python3 - "$file" "$path" <<'PY'
import json, pathlib, sys

file, folder = pathlib.Path(sys.argv[1]), sys.argv[2]
data = json.loads(file.read_text())
project = data.setdefault("projects", {}).setdefault(folder, {})
if project.get("hasTrustDialogAccepted") is not True:
    project["hasTrustDialogAccepted"] = True
    file.write_text(json.dumps(data, indent=2) + "\n")
print(f"{file}: holds the trust of {folder}")
PY
elif command -v jq >/dev/null; then
  if ! jq -e --arg path "$path" '.projects[$path].hasTrustDialogAccepted == true' "$file" >/dev/null 2>&1; then
    tmp=$(mktemp)
    jq --arg path "$path" \
      '.projects = ((.projects // {}) | .[$path] = ((.[$path] // {}) + {hasTrustDialogAccepted: true}))' \
      "$file" > "$tmp" && mv "$tmp" "$file"
  fi
  jq -e --arg path "$path" '.projects[$path].hasTrustDialogAccepted == true' "$file" >/dev/null \
    && echo "$file: holds the trust of $path"
else
  echo "$file: no python3 and no jq here, so no change"
  exit 0
fi
chmod 600 "$file"
SH
```

### The trust of Codex

The file is `~/.codex/config.toml` of the SSH user, and the entry is the table
`[projects."<path>"]` with `trust_level = "trusted"`. The block appends the table
only when the file holds no table of that path.

```sh
ssh <options of the machine key> <user>@<address> sh <<'SH'
command -v codex >/dev/null || exit 0
path=$(realpath ~/example-repo)
file=$HOME/.codex/config.toml
table="[projects.\"$path\"]"
mkdir -p "$(dirname "$file")" && chmod 700 "$HOME/.codex"
[ -e "$file" ] || { : > "$file" && chmod 600 "$file"; }
if grep -qxF -- "$table" "$file"; then
  echo "$file: holds $table, so no change"
  exit 0
fi
printf '\n%s\ntrust_level = "trusted"\n' "$table" >> "$file"
chmod 600 "$file"
echo "$file: added $table"
SH
```

### The trust of pi

The file is `~/.pi/agent/trust.json` of the SSH user, and the entry is
`{"<path>": true}` merged into the object. pi asks only for a folder that holds
`.pi/settings.json`, `.pi/mcp.json`, `.pi/extensions`, `.pi/skills`, `.pi/prompts`,
`.pi/themes`, `.pi/SYSTEM.md`, `.pi/APPEND_SYSTEM.md` or `.agents/skills`, and an
entry for a folder with none of them is harmless.

```sh
ssh <options of the machine key> <user>@<address> sh <<'SH'
command -v pi >/dev/null || exit 0
path=$(realpath ~/example-repo)
file=$HOME/.pi/agent/trust.json
mkdir -p "$(dirname "$file")" && chmod 700 "$HOME/.pi" "$HOME/.pi/agent"
if [ ! -e "$file" ]; then
  printf '{"%s": true}\n' "$path" > "$file"
  chmod 600 "$file"
  echo "$file: written"
  exit 0
fi
if command -v python3 >/dev/null; then
  python3 - "$file" "$path" <<'PY'
import json, pathlib, sys

file, folder = pathlib.Path(sys.argv[1]), sys.argv[2]
data = json.loads(file.read_text())
if data.get(folder) is not True:
    data[folder] = True
    file.write_text(json.dumps(data, indent=2) + "\n")
print(f"{file}: holds the trust of {folder}")
PY
elif command -v jq >/dev/null; then
  if ! jq -e --arg path "$path" '.[$path] == true' "$file" >/dev/null 2>&1; then
    tmp=$(mktemp)
    jq --arg path "$path" '. + {($path): true}' "$file" > "$tmp" && mv "$tmp" "$file"
  fi
  jq -e --arg path "$path" '.[$path] == true' "$file" >/dev/null \
    && echo "$file: holds the trust of $path"
else
  echo "$file: no python3 and no jq here, so no change"
  exit 0
fi
chmod 600 "$file"
SH
```

The step is `done` when every block that ran named its file and the trust that it
holds, and the ready card then says in one line which programs those are, for
example `Folder trusted for: Claude Code, Codex`. A file that a block left as it
is says so on the card with the reason, and no `Folder trusted for` line names
that program. No question to the person stands before this step, because the rent
named the repository.

## The plan uses the recipe

A todo of a plan whose place is `rent a machine` uses this page: the recipe of
the repository of the plan gives the resources, the tools and the check, and the
helper of that todo works in the clone that the recipe brought. The rent card of
the plan states the price of the machines together, as the rent card of this page
does, and one press of the plan card rents them.

## Limits

- A recipe states what the work needs, and the run proves it with the check of
  the recipe. It does not prove that the check is the right check for the
  repository.
- The versions of the tools come from the recipe. A machine that installs a
  newer version of a tool than the recipe names is a machine that the check may
  refuse, and the card says which version it installed.
- A repository whose work needs a graphical session, a device or a licensed tool
  is outside this page: the recipe holds no such entry, and the run stops at the
  step that needs it.
