# ADR-1774: A cloud provider is done when a person can pick it, rent it and delete it

Status: Accepted. This record gives the completion rule for a new cloud provider.
Date: 2026-10-25
Deciders: Repository maintainer through reviewed implementation PRs.
Tracking: [#1774](https://github.com/peters/horizon/issues/1774).

## Context

Epic #1774 adds Azure as the fourth provider beside RunPod, Hetzner and Docker
host. The eight planned PRs each passed its own tests. The provider compiled
with an exhaustive `Kind`, the ARM client polled its operations, the retail
catalog priced every VM size, and the lifecycle provisioned, stopped, resumed
and deleted a real cloud. The CLI and the MCP tools could create an Azure cloud
for a profile with `provider: azure`.

A person could not. **New cloud** listed Azure prices as informational rows with
`rentable: false`, so the top-right provider chooser fell back to a row filter
that appears only when a provider has a rentable offer. Azure never appeared as
a provider choice, and no procedure recorded a rent or a delete of an Azure
cloud in the UI on a final head. The gap came from a rule that looked correct
per PR: a dialog lists the providers it can rent from, and an informational
offer is not a rental.

The cost was not a failed test. Every test passed, and the feature was still
unusable through the one surface that most people use. This record prevents a
repeat, for any provider.

Record placement: this repository keeps its decision records directly under
`docs/architecture/` and titles them `ADR-<issue>` (`shared-cloud-workers.md`,
`remote-workspace-storage.md`). There is no `docs/architecture/adr/` directory,
and this record does not start one, so that one convention stays in force.

## Decision

### A provider is done at four observed outcomes

A provider is complete only when all four of these are true, in this order, for
one cloud that a person made:

1. **Pick.** A person picks the provider in the **New cloud** heading. The entry
   is present whether or not this machine can start a cloud there yet.
1. **Rent.** A person rents a cloud on it through the UI, without a CLI, a
   settings edit, or a hand-written profile.
1. **Run.** The worker image starts end to end: the cloud reaches its ready
   state, SSH answers, the repository arrives, and an agent session or a
   terminal panel works on it.
1. **Delete.** A person deletes the cloud in the UI, and the delete is proven:
   for Azure the resource group is absent and a tag sweep is empty; for a
   provider with a VM, the VM is absent. A stop or a release is not a delete.

A provider that meets only some of these is in progress, and its epic stays
open. A merge of the provider's lifecycle code is not evidence that a person can
use it.

### The chooser and the lifecycle read one policy

The provider list in **New cloud** derives its entries from the same policy the
lifecycle admits a cloud with. `horizon_cloud::provider::Description` holds the
one description per provider, and `Kind` makes an unhandled provider fail to
compile. The dialog reads that description for the label, the mark and
`supports(profile)`. It reads the machine's own bindings for the section that
makes the provider usable (`prices::State::bound` in
`crates/horizon-ui/src/app/cloud_panel/production/prices`), which the same
settings file gives the lifecycle through `Settings::load` and
`Settings::for_cloud` (`crates/horizon-core/src/cloud_runtime/settings.rs`).
Each provider's `preflight` in `crates/horizon-core/src/cloud_runtime/providers`
then refuses a cloud whose profile its section cannot place.

Consequences of the rule:

- A provider whose entries come from a second, dialog-local rule is a defect,
  even when the two rules agree today. The AWS-shaped mistake is a list of
  provider names in the dialog beside a provider list in the lifecycle.
- The chooser never branches on a provider name. It shows what
  `Description::ALL` and the machine's bindings give, and a new provider needs
  one description entry, one `Kind` arm, and its own settings section.
- A provider that a profile cannot run on is absent from the heading for that
  profile, because `supports(profile)` is the same test that the lifecycle
  applies to the profile.

### An informational offer must not hide a provider

`rentable: false` marks an offer as comparison only: its price is real, and
Horizon refuses to create a cloud from that row, because something else decides
the placement. Azure is the current case. The machine's `azure` section decides
the VM size and the region, so `crates/horizon-cloud/src/offers/azure.rs` sets
`rentable: false` for every row and the cloud's placement comes from
`Settings::for_cloud`.

Informational offers must not remove the provider from the chooser:

- The chooser entry is decided by whether the machine has the provider's
  section, not by whether any offer is rentable.
- A provider whose section is missing is still listed, dimmed, and it says what
  to add: the same reason the checks and the start refusal use.
- Choosing an entry moves the cloud to that provider and keeps the placement
  that the section allows, so it reaches the placement that a repository profile
  written for that provider reaches.
- A start on a provider whose section is missing is refused with that reason,
  and **Start cloud** stays disabled. The entry never rents an unexpected row.

This rule keeps `rentable` a statement about one offer, and never a statement
about a provider.

### The native VNC smoke lane is mandatory

A provider PR completes on a smoke run, not on unit tests. The run follows the
procedure for that provider under `docs/testing/procedures/` and this lane:

- The candidate is a frozen binary from the final head. Record its SHA-256.
- The desktop is task-owned: a separate X display with its own window manager,
  private application state and a loopback VNC server, as
  `scripts/device-smoke/README.md` describes. Do not use the developer's
  desktop.
- The run is watched live through a native Device panel in the person's
  workspace. A screenshot, a recording or a written report alone does not show
  it. If the panel cannot be presented, report that lane as blocked.
- The recording covers pick, rent, the image end to end, and delete with the
  proven absence of the provider's resources. Keep the motion video, and add
  the PR GIF from it under the repository GIF rule.
- Delete the cloud that the run made, and report the proof of absence for it.

## Consequences

- A provider epic lists the four outcomes as acceptance items, and its last PR
  is the one that records the smoke run.
- A reviewer refuses a provider UI PR that has no native VNC smoke run and no
  GIF, and a provider PR that a person cannot pick in **New cloud**.
- A new provider costs one real subscription run. That run is the price of the
  outcomes above, and it is not replaceable with a recorded provider response.
- `rentable` keeps one meaning. Code that compares prices keeps using
  informational rows, and code that decides a chooser entry uses the settings
  policy instead.

## Precedent

Epic #1774 is the precedent for both halves of this record. Its PR 6
(`feat(azure): cloud settings and creation UI`) shipped Azure in the dialog from
the `azure` settings. Azure could be picked from a repository profile and from
the CLI, but not from the **New cloud** heading, and no UI procedure recorded a
rent or a delete. The epic stayed open after the provider became deployable,
which is what showed the difference between "the lifecycle works" and "a person
can use it". This record makes that difference the completion rule.
