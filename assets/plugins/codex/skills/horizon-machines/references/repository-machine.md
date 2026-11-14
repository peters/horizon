# A machine for a repository

A repository says in its own `AGENTS.md` what a machine for its work needs. The
person asks for one such machine, or two, and gets machines that are ready for
that repository: the system packages are there, the tools are there, the
repository is there, its caches are warm, and the check of the repository passed
on the machine.

This page holds the recipe, the cards of one run, and the way that the
repository arrives. The provider commands, the rent, the wait for SSH and the
delete are the steps of the skill.

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

```json
{"components": [
  {"type": "heading", "text": "Two machines for example-repo", "subtitle": "The recipe of AGENTS.md",
   "badge": {"text": "Rent?", "tone": "info"}},
  {"type": "kv", "rows": [
    {"label": "Resources", "value": "4 vCPU, 8 GB"},
    {"label": "Type", "value": "the cheapest type of this place that holds them"},
    {"label": "Machines", "value": "2"},
    {"label": "Price", "value": "2 x 0.0107 EUR / hour, 0.0214 EUR / hour together", "mono": true}]},
  {"type": "text", "text": "One press rents both machines. The setup of each machine follows on its own card."},
  {"type": "buttons", "items": [
    {"id": "rent", "label": "Rent 2 machines", "tone": "primary"},
    {"id": "cancel", "label": "Cancel", "tone": "danger"}]}]}
```

### 2. One steps card for each machine

After the press, rent each machine and give each one a card of its own, titled
with its name. The card shows the five steps of the machine, one after the
other:

1. `Prepare the system`: the packages of the recipe, with the package manager of
   the image, and the update of the package list first.
2. `Install the tools`: each tool with the version of the recipe, installed in
   the way that the recipe names.
3. `Bring the repository`: the bundle or the clone of the section below.
4. `Warm up`: the `warmup` commands of the recipe, in the folder of the
   repository.
5. `Run the check`: the `check` command of the recipe, in the folder of the
   repository.

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "Ready for example-repo",
   "badge": {"text": "Working", "tone": "info"}},
  {"type": "progress", "label": "Prepare the machine", "percent": 40, "detail": "2 of 5"},
  {"type": "steps", "steps": [
    {"label": "Prepare the system", "state": "done", "at": "18 s"},
    {"label": "Install the tools", "state": "done", "at": "2 m 10 s"},
    {"label": "Bring the repository", "state": "now", "at": "now"},
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
    {"label": "Bring the repository", "state": "done", "at": "8 s"},
    {"label": "Warm up", "state": "done", "at": "1 m 40 s"},
    {"label": "Run the check", "state": "done", "at": "6 m 02 s"}]},
  {"type": "text", "text": "cargo test --workspace: 412 tests pass, 0 failed"},
  {"type": "buttons", "items": [
    {"id": "connect", "label": "Connect", "tone": "primary"},
    {"id": "delete", "label": "Delete the machine", "tone": "danger"}]}]}
```

### 4. A step that fails keeps its card

A machine that failed a step keeps its card: the heading keeps the name of the
machine, the step that failed is `failed` in the list, one `text` component holds
the last lines of the output of that step, and the card holds the buttons
`Try again` and `Delete the machine`. Never rent a second machine for the same
card, and never hide a failed step under a `Working` badge.

```json
{"components": [
  {"type": "heading", "text": "build-1", "subtitle": "example-repo",
   "badge": {"text": "Stopped", "tone": "warn"}},
  {"type": "steps", "steps": [
    {"label": "Prepare the system", "state": "done", "at": "18 s"},
    {"label": "Install the tools", "state": "done", "at": "2 m 10 s"},
    {"label": "Bring the repository", "state": "done", "at": "8 s"},
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

The bundle holds every branch and every tag of the clone that made it, and no
credential reaches the machine. A clone from a remote is the other way, and it
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
