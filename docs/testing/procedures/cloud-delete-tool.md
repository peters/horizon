---
procedure: cloud-delete-tool
feature: cloud delete MCP tool
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud delete tool test procedure

## 1. Purpose

This procedure shows that the `cloud_delete` MCP tool deletes one cloud's provider
resources without a person, that it deletes only a cloud that an agent panel of the
same actor created, and that it reports what the provider shows.

## 2. Applicability

- Candidate: a Horizon build with the `cloud_delete` MCP tool, from `cargo build` on
  this checkout.
- Platforms: Linux. The provider is the one the profile of the cloud names.
- This procedure does not test:
  - the removal of a cloud card from the list. The person removes a deleted cloud
    with the removal flow of the cloud card procedure.
  - a credential, a tailnet node or a browser session of the worker. Use the cloud
    card procedure and the tailnet procedure.
  - a delete from the cloud card. That path is the cloud card procedure.
  - a cloud that a person created. Use the cloud card procedure.

## 3. Safety

> **CAUTION:** EVERY DELETE THAT THIS PROCEDURE STARTS IS PERMANENT. A provider
> request that Horizon sent cannot be recalled. Use only clouds that this
> procedure creates, and read `cloud_list` after each step.

> **CAUTION:** THE TEST SENDS NO PROVIDER CREDENTIAL TO AN AGENT. Horizon reads
> the provider API key in Cloud settings itself. Do not write the key, a provider
> token or a worker ID in a report, a pull request or a screenshot.

## 4. Equipment and preconditions

- One Horizon desktop with an agent panel in a workspace.
- A Git repository on this computer with a committed revision and no
  `.horizon/cloud.yml` in that commit.
- A provider API key in Cloud settings, as `runpod_key_file`, and an
  `agent_lifecycle` section that allows that provider, that repository and the
  hourly rate of the profile.
- A saved Horizon session for the workspace of the agent panel.
- An MCP client for the Horizon browser MCP tools.
- Two agent panels in the same workspace, to test the lease in both directions.
- A test Horizon that you start only through the local smoke fixture with
  `--native-view`, and watch live in a native VNC Device panel, as
  [the fixture guide](../../../scripts/device-smoke/README.md#horizon-inside-a-native-vnc-device-panel)
  describes.

## 5. Setup

1. Start the candidate build through the fixture, and open the workspace with the
   two agent panels. Watch the desktop live in the native VNC Device panel.

   Result: Both agent panels are live, and the cloud list of that workspace is
   visible in the Device panel.

2. From the first agent panel, call `cloud_create` with `operation submit`, a
   lowercase UUID `operation_id`, the absolute `repository` and
   `quick_start: true`.

   Result: The answer holds `phase: submitted` and one `cloud` ID. Note the ID as
   cloud C.

3. Poll `cloud_create` with `operation status` and the same `operation_id`.

   Result: `done` becomes true and `phase` is `ready`. Cloud C is on a card in
   the workspace of the first panel.

## 6. Tasks

### 6.1 D1 — A cloud of another agent is refused

1. From the second agent panel, call `cloud_delete` with `operation submit`, a new
   lowercase UUID `operation_id`, and `cloud` C.

   Result: The tool answers `phase: submitted`.

2. Call `cloud_delete` with `operation status` and that `operation_id`.

   Result: `phase` is `failed`, `absent` is false, and `error` is
   `cloud_delete_refused: not an agent's cloud; a person deletes this cloud on its
   card`. No provider request is sent, and the card of cloud C does not change.

### 6.2 D2 — A request with the wrong shape is refused

1. Call `cloud_delete` with `operation submit` and no `cloud`.

   Result: The tool answers `cloud_delete_invalid_request: operation_id is one
   lowercase UUID; submit needs one cloud ID`.

2. Call `cloud_delete` with `operation submit`, a `cloud` of a cloud that is not
   in the workspace, and a new `operation_id`. Use `cloud-unknown` as the value.

   Result: The tool answers `cloud_delete_unknown_cloud: no cloud with this ID in
   your workspace; run cloud_list first`.

### 6.3 D3 — The creator deletes its own cloud

1. From the first agent panel, call `cloud_delete` with `operation submit`, a new
   lowercase UUID `operation_id`, and `cloud` C.

   Result: The answer holds `phase: submitted`, `cloud` C, the `provider` of the
   profile, and an `in_progress` sentence that says a delete may already be in
   progress and cannot be recalled.

2. Call `cloud_delete` with `operation status` and that `operation_id` every 15
   seconds.

   Result: `phase` is `deleting` and `stage` names the current deletion step, such
   as `Delete worker`. The card of cloud C shows the deletion.

3. Wait until `done` is true.

   Result: `phase` is `done`, `absent` is true, and `absence` states that the
   record and every provider journal confirm that nothing the cloud created
   remains.

4. Call `cloud_list` for the workspace.

   Result: Cloud C is in the list without a worker, and its card offers **Remove
   cloud**.

### 6.4 D4 — The provider reports absence with its own API

1. Read the worker ID and the volume ID from the record of cloud C. Do not write
   them in the report.

   Result: You have both IDs.

2. With your own provider credential, and never with a credential of Horizon, ask
   the provider for the worker and for the volume of cloud C.

   Result: Both answers are `404`, the account lists no pod or volume for cloud C,
   and the provider's billing view lists no resource for it.

### 6.5 D5 — A second delete of the same cloud sends no provider request

1. Call `cloud_delete` with `operation submit`, a new `operation_id`, and `cloud`
   C.

   Result: The answer holds `phase: submitted` and `stage` is `null`. Horizon
   checks the record on its own thread, as it does for the first delete.

2. Call `cloud_delete` with `operation status` and the same `operation_id`, until
   `done` is true.

   Result: `phase` is `done`, `absent` is true, and `stage` is `Worker deleted`.
   No provider delete request is sent, because the record already says `Deleted`.

3. Call `cloud_delete` with `operation submit` again with the `operation_id` of
   D3.

   Result: The tool answers the same operation. No second delete starts.

### 6.6 D6 — A cancel cannot recall a delete

1. Create a second cloud with `cloud_create`, as in section 5, and note its ID as
   cloud D.

2. Call `cloud_delete` with `operation submit` and `cloud` D, then call
   `cloud_delete` with `operation cancel` and the same `operation_id` at once.

   Result: The answer holds `phase: cancelled` and `cloud_delete_cancelled: the
   delete may already have started`.

3. Call `cloud_delete` with `operation status` and the same `operation_id`, until
   `done` is true.

   Result: `phase` stays `cancelled`, and `absent` and `stage` state what the
   record and the provider journal prove. When the delete had already gone past
   recall, `absent` is true and `absence` names the cancel and the recall. When it
   stopped before a provider request, `absent` is false and `absence` names the
   cancel and the step it reached.

4. Read `cloud_list`, and read the card of cloud D.

   Result: You know whether cloud D still exists. If it does, delete it again with
   a new `operation_id`, as in D3.

### 6.7 D7 — A cloud that no agent created is refused

1. Create a cloud with **New cloud** on the card, as a person, and note its ID as
   cloud E.

2. From the first agent panel, call `cloud_delete` with `operation submit` and
   `cloud` E.

   Result: The answer's `error` is `cloud_delete_refused: not an agent's cloud; a
   person deletes this cloud on its card`. Cloud E and its card do not change.

### 6.8 D8 — A cloud without an agent panel is refused

1. Close the agent panel of the first agent, and call `cloud_delete` from that
   client.

   Result: The tool answers `cloud_delete_unavailable: requires a Horizon agent
   panel`.

## 7. Pass criteria

- D1, D2 and D7 send no provider request, and their clouds stay as they were.
- D1, D7 and D8 are refusals: the lease and the agent panel decide them.
- D3 deletes cloud C, and `absent` becomes true only when the record and the
  provider journal confirm absence.
- D4 shows the same absence with the provider's own API, with the credential of
  the person.
- D5 starts no second delete: its first `submit` answers `submitted`, and its
  poll shows the record already deleted.
- D6 answers the cancel sentence and states that the delete may already have
  started.
- `cloud_list` shows every cloud that this run created, deleted or not.

## 8. Cleanup

1. Delete every cloud that this run created and that still exists: call
   `cloud_delete` from its creator, or choose **Delete** on its card.

   Result: Each one answers `absent` true, or the provider API reports absence by
   the rule of D4.

2. Remove the cards of the clouds that this run deleted, with **Remove cloud**.

   Result: `cloud_list` of the workspace lists no cloud of this run.

3. Stop any cloud of this run that still has a worker, and close Horizon and the
   MCP client.

   Result: No worker of this run stays billed, and no Horizon process of this run
   stays alive.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Give each task ID of section 6 a result. Keep worker IDs, volume
IDs, credentials and paths of this computer out of the repository.
