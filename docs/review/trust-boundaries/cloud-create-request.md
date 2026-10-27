---
applyTo: "crates/horizon-ui/src/app/cloud_panel/production/create_agent.rs,crates/horizon-ui/src/app/cloud_panel/production/create_agent/**,crates/horizon-ui/src/app/browser_provider_usage.rs,crates/horizon-browser-control/src/manifest/cloud_create.rs,crates/horizon-browser-mcp/src/controller/cloud_create.rs,crates/horizon-core/src/cloud_runtime/settings/agent_lifecycle.rs,crates/horizon-core/src/cloud_runtime/prices/agent.rs"
---

# Cloud create request boundary

Read the cloud create test procedure `docs/testing/procedures/cloud-create-tool.md`
and the machine settings section in
`crates/horizon-core/src/cloud_runtime/settings.rs`.

## Untrusted input

- An agent panel of a workspace. Its `cloud_create` request is untrusted: the
  operation, the repository path, the name and the caller-chosen operation ID.
- The repository directory and its Git content on this computer. The commit
  decides the profile that quick start uses.
- Every answer of a provider: its price list, its worker and its GitHub sign-in
  prompt.
- The saved record of a cloud, which a later operation reads again.

## What this code guarantees

- The host answers only the request of a live agent panel of this Horizon, in
  that panel's workspace. A request of another host instance or of an actor
  without an agent panel is refused before any file or provider call.
- The machine's `agent_lifecycle` section decides every create. Without the
  section, no agent rents a cloud. The listed repositories, the listed providers
  and the hourly limit are refusals, never warnings. A rate that Horizon cannot
  quote is a refusal, so an unknown price never passes the limit.
- The provider of an agent's cloud is chosen from the offers of the providers the
  section allows: the cheapest offer that meets the quick-start requirements
  wins, by the one order `cloud_offers` uses, and a tie keeps the first offer of
  that order. The provider is never the one the profile names unless its offer
  wins, and an offer of a provider the section does not list is never chosen. A
  provider whose prices this machine cannot read offers nothing, and a policy
  whose providers have no offer is a refusal that names the providers it lists.
  The rate the policy compares with its limit is the hourly price of the chosen
  offer, in the currency of its provider.
- The offer that the policy admits is the worker that the deployment requests.
  The chosen offer's server type or GPU type, its location and, on a provider
  that bills by flavor, its size go into the deployment request, and the card
  keeps that placement. The fallback server types, sizes and locations of the
  machine's own settings never replace them. An offer that Horizon cannot pin to
  one worker is a refusal, and the create rents nothing.
- The settings file is read on the operation's own thread, never on the frame
  thread, so the answer to a `submit` is `submitted` and every refusal of the
  policy, the repository and the price is the `status` answer that follows.
- Quick start alone is offered. A commit with its own `.horizon/cloud.yml` is
  refused, and no caller chooses a worker, a plan token, siblings, a publishing
  destination or a provider credential.
- The cloud is registered on a card in the caller's workspace, and the record
  that names it is durable, before any worker is rented.
- The deployment runs on a thread that the host owns. One operation owns one
  cloud: while it runs, `Start`, `Resume`, `Deploy` and the restore path of that
  card refuse, and a second operation for the same repository in one workspace
  is refused. A card the person removes before the worker is rented fails the
  operation instead, and a cancel before that point rents nothing: the operation
  never replaces a token that is already cancelled.
- The outcome reaches the agent before the card's presentation watch starts, so
  a ready cloud is done while that watch keeps running on the deployment thread
  for as long as the cloud is live. A ready cloud holds no operation token: the
  card's own paths work again, a cancel answers `cloud_create_done`, and the
  card stops the cloud.
- A worker that asks for a GitHub sign-in gets the existing non-UI answer
  `github::skip` only when the policy allows it. Otherwise the operation fails
  with `cloud_create_needs_person` and cancels. No sign-in prompt waits for a
  person on the card of a cloud that nobody watched being created.
- The status answer reads this Horizon's own operation and the card's output
  ring. A cancel asks an operation that is not ready to stop, and answers that
  the worker may already exist.
- A cloud that never rented a worker leaves no card behind: a refusal or a
  cancel before allocation removes the card the operation registered, so the
  person has nothing to remove later and may submit the repository again.

## What this code does not guarantee

- A cancel cannot recall a provider request that already left this computer.
  Only `cloud_list` and the cloud's card show and stop a rented worker.
- The policy is checked once, before the record exists. A policy that changes
  while a deployment runs does not stop that deployment. A `status` poll after
  such an edit reads no new policy.
- The price is the provider's own quote at that moment. Storage and other
  provider charges are outside `max_hourly`.
- The placement of the admitted offer is a request, not a reservation. A provider
  that has no stock of that server type or location at that moment fails the
  deployment; Horizon does not rent another worker instead. The rate that the
  worker bills can still differ from the quote, because a price can change after
  Horizon reads it.
- The operation table is in memory. After a Horizon restart, the cloud is on its
  card, and its operation ID answers `cloud_create_unknown_operation`.
- This version gives an agent no worker choice, no plan token, no siblings, no
  publishing selection and no CLI plan entry.
