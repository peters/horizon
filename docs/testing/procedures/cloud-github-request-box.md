---
procedure: cloud-github-request-box
feature: GitHub access request box on a cloud that an agent rented
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings, GitHub sign-in of this computer]
owner: peters
---

# GitHub access request box test procedure

## 1. Purpose

This procedure shows that the card of a cloud that an agent rented shows the GitHub
access requests of the cloud's agents, although this Horizon window never saw a
sign-in prompt for that cloud. It also shows that a worker without the GitHub
service costs no repeated asks.

## 2. Applicability

- Candidate: a Horizon build from this checkout.
- Platforms: Linux.
- This procedure does not test the decision of a request, which
  `cloud-create-tool.md` and the GitHub card procedure cover.

## 3. Safety

> **CAUTION:** EVERY SUBMITTED CLOUD RENTS A PAID WORKER. Stop and delete the cloud
> that this procedure creates before you end the run.

> **CAUTION:** THE REQUESTS OF THIS PROCEDURE NAME A PRIVATE REPOSITORY. Keep the
> screenshots, the logs and the request ids out of the repository.

## 4. Equipment and preconditions

- One Horizon desktop with an agent panel in a saved session.
- A GitHub sign-in of this computer in Cloud settings.
- An `agent_lifecycle` section and a `spending_envelope` section in the cloud
  settings, as `cloud-create-tool.md` sets them.
- A Git repository with a committed revision and no `.horizon/cloud.yml` in that
  commit.

## 5. Setup

1. Start Horizon from the candidate build, and open the workspace with the agent
   panel.

   Result: The agent panel is live.

2. Note that this window shows no sign-in prompt for the cloud of step 6.1: the
   agent that rents the cloud installs this computer's GitHub chain without a
   prompt of its own.

   Result: The card of that cloud has no sign-in line.

## 6. Tasks

### 6.1 R1 — An agent's request reaches the card

1. Call `cloud_create` with `operation submit`, a lowercase UUID `operation_id`,
   the absolute `repository` and `quick_start: true`. Poll `status` until `done`
   is true.

   Result: `phase` is `ready`, and the cloud has a card in the agent's workspace.

2. In a session of that cloud's worker, ask its GitHub service for access to one
   more repository, as the agent tool of the worker does.

   Result: The worker answers that the request waits for a person.

3. Wait up to one minute, and draw one frame of the cloud card, for example with a
   pointer movement over it.

   Result: The card shows the request, with the repository and its controls. The
   request appears although this window never showed a sign-in prompt for this
   cloud.

4. Decide the request with **Deny**.

   Result: The request leaves the card, and the worker reports it as denied.

### 6.2 R2 — A worker without the GitHub service is asked five times

1. Stop the cloud of R1, and read its journal.

   Result: The card shows no request. The log holds at most five asks for
   requests, spread over a minute, and then none until the stage of the cloud
   changes.

## 7. Cleanup

1. Stop and delete every cloud that this procedure created, and wait for each
   deletion.

   Result: `cloud_list` shows none of them.

2. Remove the `agent_lifecycle` section and the `spending_envelope` section that
   the setup added, or put the copy of the settings file back.

   Result: The cloud settings of this computer hold the sections it had before.
