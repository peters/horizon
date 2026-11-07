---
procedure: cloud-lifecycle-tool
feature: cloud lifecycle MCP tool
platforms: [linux]
cost: rents compute
destructive: yes
secrets: [provider API key in Cloud settings]
owner: peters
---

# Cloud lifecycle tool test procedure

## 1. Purpose

This procedure shows that the `cloud_lifecycle` MCP tool stops and resumes the
worker of one cloud without a person, that it acts only on a cloud that an agent
panel of the same actor created, and that each answer states the phase it
reached.

## 2. Applicability

- Candidate: a Horizon build with the `cloud_lifecycle` MCP tool, from
  `cargo build` on this checkout.
- Platforms: Linux. The provider is the one the profile of the cloud names.
- This procedure does not test:
  - the delete of the cloud. Use the cloud delete procedure.
  - a stop or a resume from the cloud card. That path is the cloud card
    procedure.
  - the agent session of the worker after a resume. Use the cloud agent session
    procedure.
  - a cloud that a person created. Use the cloud card procedure.

## 3. Safety

> **CAUTION:** EVERY CLOUD THAT THIS PROCEDURE CREATES BILLS THE PROVIDER UNTIL YOU
> DELETE IT. A stop ends compute, but a provider can still bill storage. Delete
> every cloud that this procedure creates before you finish.

> **CAUTION:** THE TEST SENDS NO PROVIDER CREDENTIAL TO AN AGENT. Horizon reads the
> provider API key in Cloud settings itself. Do not write the key, a provider
> token or a worker ID in a report, a pull request or a screenshot.

## 4. Equipment and preconditions

- One Horizon desktop with an agent panel in a workspace.
- A Git repository on this computer with a committed revision and no
  `.horizon/cloud.yml` in that commit.
- A provider API key in Cloud settings, as `runpod_key_file`, and an
  `agent_lifecycle` section that allows that provider, that repository and the
  hourly rate of the profile.
- An approved spending envelope in Cloud settings, so that `cloud_create` admits
  a rent.
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
   lowercase UUID `operation_id`, the absolute `repository`, `quick_start: true`
   and `hours: 1`.

   Result: The answer holds `phase: submitted` and one `cloud` ID. Note the ID as
   cloud C.

3. Poll `cloud_create` with `operation status` and the same `operation_id`.

   Result: `done` becomes true and `phase` is `ready`. Cloud C is on a card in the
   workspace of the first panel, and its stage is `Ready`.

## 6. Tasks

### 6.1 L1 — A cloud of another agent is refused

1. From the second agent panel, call `cloud_lifecycle` with `operation submit`, a
   new lowercase UUID `operation_id`, `action: stop` and `cloud` C.

   Result: The tool answers `phase: submitted` and `action: stop`.

2. Call `cloud_lifecycle` with `operation status` and that `operation_id`.

   Result: `phase` is `failed`, `done` is true, and `error` is
   `cloud_lifecycle_refused: not an agent's cloud; a person stops this cloud on
   its card`. No provider request is sent, and cloud C stays `Ready`.

### 6.2 L2 — A request with the wrong shape is refused

1. Call `cloud_lifecycle` with `operation submit`, an `operation_id` and
   `cloud` C, and no `action`.

   Result: The tool answers `cloud_lifecycle_invalid_request: operation_id is one
   lowercase UUID; submit needs action stop or resume and one cloud ID`.

2. Call `cloud_lifecycle` with `operation submit`, `action: delete` and `cloud`
   C.

   Result: The request is refused. A delete is `cloud_delete`, which also proves
   the absence of the provider resources.

3. Call `cloud_lifecycle` with `operation submit`, `action: stop` and the `cloud`
   value `cloud-unknown`.

   Result: The tool answers `cloud_lifecycle_unknown_cloud: no cloud with this ID
   in your workspace; run cloud_list first`.

4. Call `cloud_lifecycle` with `operation submit`, `action: resume` and `cloud` C,
   while cloud C is `Ready`.

   Result: `phase` is `failed` and `error` starts with
   `cloud_lifecycle_refused: this cloud is not stopped`.

### 6.3 L3 — The creator stops its own cloud

1. From the first agent panel, call `cloud_lifecycle` with `operation submit`, a
   new lowercase UUID `operation_id`, `action: stop` and `cloud` C.

   Result: The answer holds `phase: submitted`, `action: stop`, `done: false` and
   an `in_progress` sentence that says an action that cannot be recalled may
   already be in progress.

2. Call `cloud_lifecycle` with `operation status` and that `operation_id` every
   10 seconds.

   Result: `phase` becomes `stopping`, and then `stopped` with `done: true`,
   `stage: stopped` and an `error` of `null`.

3. Read `kept` in the answer.

   Result: `kept` names what the stop kept, such as the workspace volume. The card
   of cloud C shows the stopped cloud, and `cloud_list` reports the cloud without
   a running worker.

4. Call `cloud_list` in the workspace.

   Result: The cloud row shows the stopped cloud and its saved session, and the
   card offers **Resume**.

### 6.4 L4 — A stop of a stopped cloud sends no provider request

1. Call `cloud_lifecycle` with `operation submit`, a new `operation_id`,
   `action: stop` and `cloud` C.

2. Call `cloud_lifecycle` with `operation status` and that `operation_id` until
   `done` is true.

   Result: `phase` is `stopped` at once, `stage` is `stopped` and `error` is
   `null`. No provider stop request is sent, because the record already says
   `Stopped`.

3. Read the provider's own API for the worker of cloud C with your own
   credential.

   Result: The worker reports the stopped state, and the provider bills no compute
   for it.

### 6.5 L5 — The creator resumes the cloud it stopped

1. Call `cloud_lifecycle` with `operation submit`, a new `operation_id`,
   `action: resume` and `cloud` C.

2. Call `cloud_lifecycle` with `operation status` and that `operation_id` every
   10 seconds until `done` is true.

   Result: `phase` becomes `resuming`, and then `resumed` with `stage: ready` and
   `error` of `null`. The card of cloud C shows the running cloud again.

3. Open a panel of cloud C from its card.

   Result: The workspace of the worker holds the same files as before the stop.

### 6.6 L6 — A lost answer is read and never sent twice

1. Call `cloud_lifecycle` with `operation submit`, one `operation_id`,
   `action: stop` and `cloud` C, and read the answer.

2. Call `cloud_lifecycle` with `operation submit`, the same `operation_id`,
   `action: stop` and `cloud` C.

   Result: The tool answers the same operation, with the same `phase`, and no
   second provider request is sent.

3. Call `cloud_lifecycle` with `operation status` and an `operation_id` that no
   operation of this session used.

   Result: The tool answers `cloud_lifecycle_unknown_operation: no operation of
   yours has this ID`.

### 6.7 L7 — A stopped cloud costs no compute

1. Read the worker's state in the provider's own view after cloud C is stopped.

   Result: The provider reports the worker stopped, and its billing view adds no
   compute charge for the stopped time.

2. Resume cloud C, as in L5, and read the provider's view again.

   Result: The provider reports the worker running, and its billing view adds the
   compute charge of the running time.

## 7. Cleanup

1. From the first agent panel, call `cloud_delete` with `operation submit`, a new
   lowercase UUID `operation_id` and `cloud` C, and poll it until `done` is true.

   Result: `absent` is true, and the answer states that the record and every
   provider journal confirm that nothing cloud C created remains.

2. Confirm with the provider's own API that no worker and no volume of cloud C
   remains.

   Result: Both answers are absent from the account.

3. Close the test Horizon through the fixture, and stop only the fixture-owned
   processes and state.

   Result: The candidate process exits, and your own Horizon keeps running.

## 8. Pass criteria

- L1 to L7 pass with the answers that the tasks name.
- No cloud that this procedure created remains in the provider account.
- The card of each cloud shows the phase that the tool reported.

## 9. Report

Record the run in a report under `docs/testing/reports/` from the report
template, or put the results in the pull request. State the candidate commit, the
provider, the phases that each task reached and every difference from this
procedure.
